//! M3: experiment decision engine wiring.
//!
//! Turns stored sample counts into a live Bayesian report (via the pure
//! `openpublish-experiments` engine) and applies decisions: promote the winner
//! by repointing the block to the winning immutable version, or conclude
//! "no improvement". Shared by the background auto-decider and the admin
//! routes.

use openpublish_content::now_ms;
use openpublish_experiments::{
    EngineConfig, ExperimentReport, Recommendation, VariantStats, analyze,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{ExperimentCounts, ExperimentDecision, ExperimentRecord};
use crate::repository::{Repository, RepositoryError};

// ---------------------------------------------------------------------------
// Response DTOs (mirror of `ExperimentRecord` + live report + decisions).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentVariantView {
    pub id: Uuid,
    pub block_id: Uuid,
    pub version_id: Uuid,
    pub weight: f64,
    pub is_control: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentDecisionView {
    pub id: Uuid,
    pub decided_at_ms: i64,
    pub decision: String,
    pub winner_variant_id: Option<Uuid>,
    pub promoted_version_id: Option<Uuid>,
    pub effect_size: Option<f64>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentView {
    pub id: Uuid,
    pub document_id: Uuid,
    pub block_id: Uuid,
    pub name: String,
    pub status: String,
    pub goal: String,
    pub traffic_weight: f64,
    pub confidence_threshold: f64,
    pub min_sample_per_variant: i64,
    pub no_winner_prob: f64,
    pub max_duration_ms: i64,
    pub started_at_ms: Option<i64>,
    pub decided_at_ms: Option<i64>,
    pub decision: Option<String>,
    pub winning_variant_id: Option<Uuid>,
    pub created_at_ms: i64,
    pub variants: Vec<ExperimentVariantView>,
    /// Live Bayesian report for running experiments (None once decided).
    pub report: Option<ExperimentReport>,
    pub decisions: Vec<ExperimentDecisionView>,
}

/// The result of applying a decision (or a manual override).
#[derive(Debug, Clone, Serialize)]
pub struct DecisionOutcome {
    pub experiment_id: Uuid,
    pub decision: String,
    pub winner_variant_id: Option<Uuid>,
    pub promoted_version_id: Option<Uuid>,
    pub confidence: Option<f64>,
    pub effect_size: Option<f64>,
}

pub fn config_from(exp: &ExperimentRecord) -> EngineConfig {
    EngineConfig {
        confidence_threshold: exp.confidence_threshold,
        min_sample_per_variant: exp.min_sample_per_variant.max(0) as u64,
        no_winner_prob: exp.no_winner_prob,
        max_duration_ms: exp.max_duration_ms,
    }
}

/// Build the full admin view for one experiment: record, live report, decisions.
pub async fn experiment_view(
    repo: &dyn Repository,
    exp: &ExperimentRecord,
) -> Result<ExperimentView, RepositoryError> {
    let report = if exp.status == "running" {
        let counts = repo.experiment_counts(exp.id).await?;
        Some(live_report(exp, &counts))
    } else {
        None
    };
    let decisions = repo
        .experiment_decisions(exp.id)
        .await?
        .into_iter()
        .map(|d| ExperimentDecisionView {
            id: d.id,
            decided_at_ms: d.decided_at_ms,
            decision: d.decision,
            winner_variant_id: d.winner_variant_id,
            promoted_version_id: d.promoted_version_id,
            effect_size: d.effect_size,
            confidence: d.confidence,
        })
        .collect();
    Ok(ExperimentView {
        id: exp.id,
        document_id: exp.document_id,
        block_id: exp.block_id,
        name: exp.name.clone(),
        status: exp.status.clone(),
        goal: exp.goal.clone(),
        traffic_weight: exp.traffic_weight,
        confidence_threshold: exp.confidence_threshold,
        min_sample_per_variant: exp.min_sample_per_variant,
        no_winner_prob: exp.no_winner_prob,
        max_duration_ms: exp.max_duration_ms,
        started_at_ms: exp.started_at_ms,
        decided_at_ms: exp.decided_at_ms,
        decision: exp.decision.clone(),
        winning_variant_id: exp.winning_variant_id,
        created_at_ms: exp.created_at_ms,
        variants: exp
            .variants
            .iter()
            .map(|v| ExperimentVariantView {
                id: v.id,
                block_id: v.block_id,
                version_id: v.version_id,
                weight: v.weight,
                is_control: v.is_control,
            })
            .collect(),
        report,
        decisions,
    })
}

fn count_of(counts: &[ExperimentCounts], id: Uuid) -> ExperimentCounts {
    counts
        .iter()
        .find(|c| c.variant_id == id)
        .cloned()
        .unwrap_or(ExperimentCounts {
            variant_id: id,
            impressions: 0,
            conversions: 0,
        })
}

/// Current Bayesian report for an experiment from its stored samples.
pub fn live_report(exp: &ExperimentRecord, counts: &[ExperimentCounts]) -> ExperimentReport {
    let control = exp
        .variants
        .iter()
        .find(|v| v.is_control)
        .map(|v| count_of(counts, v.id))
        .unwrap_or(ExperimentCounts {
            variant_id: exp.control_version_id,
            impressions: 0,
            conversions: 0,
        });
    let variants: Vec<VariantStats> = exp
        .variants
        .iter()
        .filter(|v| !v.is_control)
        .map(|v| {
            let c = count_of(counts, v.id);
            VariantStats {
                variant_id: v.id,
                impressions: c.impressions.max(0) as u64,
                conversions: c.conversions.max(0) as u64,
            }
        })
        .collect();
    let elapsed_ms = exp
        .started_at_ms
        .map(|s| (now_ms() - s).max(0))
        .unwrap_or(0);
    analyze(
        VariantStats {
            variant_id: control.variant_id,
            impressions: control.impressions.max(0) as u64,
            conversions: control.conversions.max(0) as u64,
        },
        &variants,
        config_from(exp),
        elapsed_ms,
    )
}

/// Run the sequential-test decision rules for one experiment. Applies the
/// engine's recommendation (promote winner / conclude no-improvement) and
/// records the append-only decision row. Returns the outcome, or `None` when
/// the engine says to keep collecting (or the experiment is not running).
pub async fn decide_experiment(
    repo: &dyn Repository,
    id: Uuid,
) -> Result<Option<DecisionOutcome>, RepositoryError> {
    let exp = repo
        .experiment(id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound("experiment".into()))?;
    if exp.status != "running" {
        return Ok(None);
    }
    let counts = repo.experiment_counts(id).await?;
    let report = live_report(&exp, &counts);

    let outcome = match &report.recommendation {
        Recommendation::Promote {
            variant_id,
            confidence,
        } => winner_outcome(&exp, &counts, variant_id, Some(*confidence)),
        Recommendation::NoWinner => Some(DecisionOutcome {
            experiment_id: id,
            decision: "no_improvement".into(),
            winner_variant_id: None,
            promoted_version_id: None,
            confidence: None,
            effect_size: None,
        }),
        Recommendation::Continue => None,
    };

    match outcome {
        Some(o) => {
            record_decision(repo, &exp, &counts, &o).await?;
            Ok(Some(o))
        }
        None => Ok(None),
    }
}

/// Manual "ship it": promote the current best variant, whatever the threshold.
pub async fn promote_experiment(
    repo: &dyn Repository,
    id: Uuid,
) -> Result<DecisionOutcome, RepositoryError> {
    let exp = repo
        .experiment(id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound("experiment".into()))?;
    if exp.status != "running" {
        return Err(RepositoryError::Conflict(
            "only running experiments can be promoted".into(),
        ));
    }
    let counts = repo.experiment_counts(id).await?;
    let report = live_report(&exp, &counts);
    let best = report
        .variants
        .iter()
        .filter(|v| !v.is_control)
        .max_by(|a, b| {
            a.prob_beats_control
                .unwrap_or(0.0)
                .partial_cmp(&b.prob_beats_control.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .or_else(|| {
            report
                .variants
                .iter()
                .filter(|v| !v.is_control)
                .max_by(|a, b| {
                    a.posterior_mean
                        .partial_cmp(&b.posterior_mean)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
    let Some(best) = best else {
        return Err(RepositoryError::InvalidInput(
            "experiment has no non-control variants".into(),
        ));
    };
    let outcome = winner_outcome(&exp, &counts, &best.variant_id, best.prob_beats_control)
        .ok_or_else(|| RepositoryError::InvalidInput("could not resolve winning variant".into()))?;
    record_decision(repo, &exp, &counts, &outcome).await?;
    Ok(outcome)
}

/// Manual "no improvement": conclude without promoting anything.
pub async fn conclude_no_winner(
    repo: &dyn Repository,
    id: Uuid,
) -> Result<DecisionOutcome, RepositoryError> {
    let exp = repo
        .experiment(id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound("experiment".into()))?;
    if exp.status != "running" {
        return Err(RepositoryError::Conflict(
            "only running experiments can be concluded".into(),
        ));
    }
    let counts = repo.experiment_counts(id).await?;
    let outcome = DecisionOutcome {
        experiment_id: id,
        decision: "no_improvement".into(),
        winner_variant_id: None,
        promoted_version_id: None,
        confidence: None,
        effect_size: None,
    };
    record_decision(repo, &exp, &counts, &outcome).await?;
    Ok(outcome)
}

fn winner_outcome(
    exp: &ExperimentRecord,
    counts: &[ExperimentCounts],
    winner_variant_id: &Uuid,
    confidence: Option<f64>,
) -> Option<DecisionOutcome> {
    let variant = exp.variants.iter().find(|v| v.id == *winner_variant_id)?;
    let control = exp.variants.iter().find(|v| v.is_control)?;
    let c = count_of(counts, control.id);
    let w = count_of(counts, variant.id);
    let control_rate = c.impressions.max(1) as f64;
    let effect_size = Some(
        w.conversions as f64 / w.impressions.max(1) as f64 - c.conversions as f64 / control_rate,
    );
    Some(DecisionOutcome {
        experiment_id: exp.id,
        decision: "winner".into(),
        winner_variant_id: Some(*winner_variant_id),
        promoted_version_id: Some(variant.version_id),
        confidence,
        effect_size,
    })
}

async fn record_decision(
    repo: &dyn Repository,
    exp: &ExperimentRecord,
    counts: &[ExperimentCounts],
    outcome: &DecisionOutcome,
) -> Result<(), RepositoryError> {
    let control = count_of(counts, exp.control_version_id);
    let (winner_impressions, winner_conversions) = match outcome.winner_variant_id {
        Some(wid) => {
            let w = count_of(counts, wid);
            (Some(w.impressions), Some(w.conversions))
        }
        None => (None, None),
    };
    let decision = ExperimentDecision {
        id: Uuid::new_v4(),
        experiment_id: exp.id,
        decided_at_ms: now_ms(),
        decision: outcome.decision.clone(),
        winner_variant_id: outcome.winner_variant_id,
        promoted_version_id: outcome.promoted_version_id,
        effect_size: outcome.effect_size,
        confidence: outcome.confidence,
        control_impressions: Some(control.impressions),
        control_conversions: Some(control.conversions),
        variant_impressions: winner_impressions,
        variant_conversions: winner_conversions,
    };
    repo.conclude_experiment(
        exp.id,
        &outcome.decision,
        outcome.winner_variant_id,
        outcome.promoted_version_id,
        &decision,
    )
    .await
}
