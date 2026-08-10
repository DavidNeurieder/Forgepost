//! A/B experiment overlay engine (Bayesian + sequential testing).
//!
//! Experiments are overlays over the immutable block-version pool (§5.1): each
//! variant points at an existing version; promoting a winner repoints the block
//! to that version and records an append-only `experiment_decisions` row.
//!
//! Analysis is Bayesian: every conversion rate is a Beta(1 + conversions,
//! 1 + impressions - conversions) posterior, and we report the probability that
//! a variant beats control, not a p-value. Because we watch continuously we
//! use a sequential testing correction: the confidence threshold is tightened
//! by the number of interim looks taken so far (spending-bound adjusted).

use forgepost_content::{BlockId, DocumentId, VersionId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use uuid::Uuid;

pub type ExperimentId = Uuid;
pub type VariantId = Uuid;

/// Beta(1, 1) = uniform prior over conversion rates.
pub const PRIOR_ALPHA: u64 = 1;
pub const PRIOR_BETA: u64 = 1;

/// Fraction of a block's traffic that is allowed to hit an experiment
/// before its name is shown. Anything above 50% defeats the purpose of
/// a "thin slice" of readers.
pub const MAX_TRAFFIC_WEIGHT: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExperimentStatus {
    Draft,
    Running,
    Decided,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperimentVariant {
    pub id: VariantId,
    pub block_id: BlockId,
    /// The immutable version this variant shows; may be shared with control.
    pub version_id: VersionId,
    /// Relative traffic weight (>= 0).
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub id: ExperimentId,
    pub document_id: DocumentId,
    pub status: ExperimentStatus,
    pub variants: Vec<ExperimentVariant>,
    pub created_at_ms: i64,
}

impl Experiment {
    /// Sum of variant weights; used to normalize traffic split.
    pub fn total_weight(&self) -> f64 {
        self.variants.iter().map(|v| v.weight).sum()
    }
}

/// Stopping rules for an experiment.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Posterior probability required (after sequential correction) to call a
    /// winner. Default 0.95.
    pub confidence_threshold: f64,
    /// Minimum impressions per variant (control included) before any decision.
    pub min_sample_per_variant: u64,
    /// Below this posterior probability of beating control, conclude
    /// "no improvement". Default 0.05.
    pub no_winner_prob: f64,
    /// Hard stop: if the experiment has run this long with no winner it is
    /// concluded "no improvement". Default 30 days in ms.
    pub max_duration_ms: i64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.95,
            min_sample_per_variant: 100,
            no_winner_prob: 0.05,
            max_duration_ms: 30 * 24 * 60 * 60 * 1000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VariantStats {
    pub variant_id: VariantId,
    pub impressions: u64,
    pub conversions: u64,
}

/// What the engine recommends right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Recommendation {
    /// Keep collecting data.
    Continue,
    /// Promote `variant_id` (posterior confidence in `confidence`).
    Promote {
        variant_id: VariantId,
        confidence: f64,
    },
    /// The variant is (near-)certain to not beat control: conclude.
    NoWinner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantReport {
    pub variant_id: VariantId,
    pub is_control: bool,
    pub impressions: u64,
    pub conversions: u64,
    pub conversion_rate: f64,
    /// Posterior mean conversion rate.
    pub posterior_mean: f64,
    /// Equal-tailed 95% credible interval.
    pub credible_interval: (f64, f64),
    /// P(this variant beats control). None for control.
    pub prob_beats_control: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentReport {
    pub variants: Vec<VariantReport>,
    pub recommendation: Recommendation,
    /// Number of interim looks taken so far (1 + observations / min_sample).
    pub n_looks: u64,
    /// Confidence threshold after the spending-bound correction.
    pub adjusted_confidence_threshold: f64,
    /// Milliseconds the experiment has been running.
    pub elapsed_ms: i64,
}

/// Probability that X ~ Beta(ax, bx) is strictly less than Y ~ Beta(ay, by),
/// with all four parameters positive integers. Derived from the identity
///
///   P(X < Y) = sum_{j=0}^{ay-1}
///       B(ax + j, bx + by) / ((by + j) * B(1 + j, by) * B(ax, bx))
///
/// where B is the beta function. Verified against direct numeric integration
/// in `prob_beta_lt_matches_numeric`.
pub fn prob_beta_lt(ax: u64, bx: u64, ay: u64, by: u64) -> f64 {
    if ax == 0 || bx == 0 || ay == 0 || by == 0 {
        return 0.0;
    }
    let mut total = 0.0;
    for j in 0..ay {
        let log_num = log_beta((ax + j) as f64, (bx + by) as f64);
        let log_den = ((by + j) as f64).ln()
            + log_beta((1 + j) as f64, by as f64)
            + log_beta(ax as f64, bx as f64);
        total += (log_num - log_den).exp();
    }
    // Clamp to the unit interval to absorb floating point noise.
    total.clamp(0.0, 1.0)
}

/// Probability that variant X beats control C: P(X > C) = P(C < X).
pub fn prob_variant_beats_control(c_alpha: u64, c_beta: u64, x_alpha: u64, x_beta: u64) -> f64 {
    prob_beta_lt(c_alpha, c_beta, x_alpha, x_beta)
}

/// Beta(a, b) with a, b integers: `I_x(a, b) = P(Bin(a+b-1, x) >= a)`.
/// Computed with the log-space modal recurrence so it stays accurate for
/// large sample sizes (where starting from the k=0 tail underflows).
pub fn beta_cdf(x: f64, a: u64, b: u64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let n = a + b - 1;
    let mode = ((n as f64 + 1.0) * x).floor() as usize;
    let ln = |k: u64| {
        log_gamma(n as f64 + 1.0) - log_gamma(k as f64 + 1.0) - log_gamma((n - k) as f64 + 1.0)
            + k as f64 * x.ln()
            + (n - k) as f64 * (1.0 - x).ln()
    };
    let mut total = 0.0;
    let mut t = ln(mode as u64).exp();
    total += t;
    // Walk down from the mode: P(k-1) = P(k) * k/(n-k+1) * (1-x)/x
    for k in (1..=mode).rev() {
        t *= k as f64 / (n - k as u64 + 1) as f64 * (1.0 - x) / x;
        total += t;
    }
    // Walk up from the mode: P(k+1) = P(k) * (n-k)/(k+1) * x/(1-x)
    t = ln(mode as u64).exp();
    for k in mode..n as usize {
        t *= (n - k as u64) as f64 / (k as f64 + 1.0) * x / (1.0 - x);
        total += t;
    }
    // Tail from `a` up is the beta CDF value.
    let sum_lt_a: f64 = (0..a).map(|k| ln(k).exp()).sum();
    (total - sum_lt_a).clamp(0.0, 1.0)
}

/// Equal-tailed credible interval covering `mass` of the Beta(a, b) posterior.
/// Find the x such that the CDF equals the lower tail via bisection.
pub fn credible_interval(a: u64, b: u64, mass: f64) -> (f64, f64) {
    let lo = invert_cdf(a, b, (1.0 - mass) / 2.0);
    let hi = invert_cdf(a, b, 1.0 - (1.0 - mass) / 2.0);
    (lo, hi)
}

fn invert_cdf(a: u64, b: u64, p: f64) -> f64 {
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if beta_cdf(mid, a, b) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Analyze a running experiment and recommend what to do next.
///
/// `control` and `variants` are indexed by variant id; `control.is_control`
/// should be true for exactly one. If the engine is called before any data
/// arrives it returns `Continue` (no premature decisions).
pub fn analyze(
    control: VariantStats,
    variants: &[VariantStats],
    config: EngineConfig,
    elapsed_ms: i64,
) -> ExperimentReport {
    let total_obs: u64 = variants.iter().map(|v| v.impressions).sum::<u64>() + control.impressions;
    let n_looks = if config.min_sample_per_variant > 0 {
        (total_obs / config.min_sample_per_variant).max(1)
    } else {
        1
    };
    let adjusted_confidence = 1.0 - (1.0 - config.confidence_threshold) / n_looks as f64;

    let mut reports = Vec::with_capacity(variants.len() + 1);
    reports.push(build_report(
        control.variant_id,
        true,
        control.impressions,
        control.conversions,
    ));

    for v in variants {
        let mut rep = build_report(v.variant_id, false, v.impressions, v.conversions);
        let c_alpha = PRIOR_ALPHA + control.conversions;
        let c_beta = PRIOR_BETA + control.impressions - control.conversions;
        let v_alpha = PRIOR_ALPHA + v.conversions;
        let v_beta = PRIOR_BETA + v.impressions - v.conversions;
        rep.prob_beats_control = Some(prob_variant_beats_control(c_alpha, c_beta, v_alpha, v_beta));
        reports.push(rep);
    }

    let recommendation = decide(
        &reports,
        control.impressions,
        config,
        adjusted_confidence,
        elapsed_ms,
    );

    ExperimentReport {
        variants: reports,
        recommendation,
        n_looks,
        adjusted_confidence_threshold: adjusted_confidence,
        elapsed_ms,
    }
}

fn build_report(
    variant_id: VariantId,
    is_control: bool,
    impressions: u64,
    conversions: u64,
) -> VariantReport {
    let alpha = PRIOR_ALPHA + conversions;
    let beta = PRIOR_BETA + impressions.saturating_sub(conversions);
    VariantReport {
        variant_id,
        is_control,
        impressions,
        conversions,
        conversion_rate: if impressions == 0 {
            0.0
        } else {
            conversions as f64 / impressions as f64
        },
        posterior_mean: alpha as f64 / (alpha + beta) as f64,
        credible_interval: credible_interval(alpha, beta, 0.95),
        prob_beats_control: None,
    }
}

fn decide(
    reports: &[VariantReport],
    control_impressions: u64,
    config: EngineConfig,
    adjusted_confidence: f64,
    elapsed_ms: i64,
) -> Recommendation {
    let best = reports.iter().filter(|r| !r.is_control).max_by(|a, b| {
        a.prob_beats_control
            .partial_cmp(&b.prob_beats_control)
            .unwrap_or(Ordering::Equal)
    });

    let Some(best) = best else {
        return Recommendation::Continue;
    };
    let prob = best.prob_beats_control.unwrap_or(0.0);

    if config.min_sample_per_variant > 0
        && best.impressions < config.min_sample_per_variant
        && control_impressions < config.min_sample_per_variant
    {
        // Not enough data to be confident either way yet.
        if elapsed_ms > config.max_duration_ms {
            return Recommendation::NoWinner;
        }
        return Recommendation::Continue;
    }

    if prob >= adjusted_confidence {
        return Recommendation::Promote {
            variant_id: best.variant_id,
            confidence: prob,
        };
    }

    if elapsed_ms > config.max_duration_ms {
        return Recommendation::NoWinner;
    }

    if prob < config.no_winner_prob {
        return Recommendation::NoWinner;
    }

    Recommendation::Continue
}

/// Deterministic traffic-split assignment.
///
/// Hash `visitor_id` and `experiment_id` into [0, 1) so a visitor is assigned
/// the same variant across requests (stable assignment). `control_share` is the
/// probability of seeing control (derived from the experiment's
/// `traffic_weight`: 1 - traffic_weight/100). `variants` are the non-control
/// variants with relative weights; the remainder of the experiment bucket is
/// split proportionally.
pub fn assign_variant(
    experiment_id: &ExperimentId,
    visitor_id: &Uuid,
    control_id: VariantId,
    control_share: f64,
    variants: &[(VariantId, f64)],
) -> VariantId {
    let frac = hash_unit(experiment_id, visitor_id);
    if frac < control_share.clamp(0.0, 1.0) {
        return control_id;
    }
    let total: f64 = variants.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return control_id;
    }
    let mut cum = 0.0;
    for (id, w) in variants {
        cum += w / total;
        if frac < control_share + (1.0 - control_share) * cum {
            return *id;
        }
    }
    control_id
}

/// Map (experiment_id, visitor_id) into [0, 1) via SHA-256.
fn hash_unit(experiment_id: &ExperimentId, visitor_id: &Uuid) -> f64 {
    let mut hasher = Sha256::new();
    hasher.update(experiment_id.as_bytes());
    hasher.update(visitor_id.as_bytes());
    let out = hasher.finalize();
    let hi = u64::from_be_bytes(out[..8].try_into().expect("8 bytes"));
    hi as f64 / u64::MAX as f64
}

// ---------------------------------------------------------------------------
// Numerics: log-gamma (Lanczos) and log-beta.
// ---------------------------------------------------------------------------

const LANCZOS_G: f64 = 7.0;
// Standard Lanczos coefficients; the extra digits are the canonical published
// values (kept as-is for fidelity, f64 rounds them internally).
#[allow(clippy::excessive_precision)]
const LANCZOS_COEFFS: [f64; 9] = [
    0.99999999999980993,
    676.5203681218851,
    -1259.1392167224028,
    771.32342877765313,
    -176.61502916214059,
    12.507343278686905,
    -0.13857109526572012,
    9.9843695780195716e-6,
    1.5056327351493116e-7,
];

/// ln Gamma(x) for x > 0, Lanczos approximation (accurate to ~1e-13).
pub fn log_gamma(x: f64) -> f64 {
    debug_assert!(x > 0.0);
    if x < 0.5 {
        // Reflection formula for small x (never needed here, params >= 1).
        let s = std::f64::consts::PI / (x * (std::f64::consts::PI * x).sin());
        (std::f64::consts::PI / s).ln() - log_gamma(1.0 - x)
    } else {
        let z = x - 1.0;
        let t = z + LANCZOS_G + 0.5;
        let mut a = LANCZOS_COEFFS[0];
        for (i, c) in LANCZOS_COEFFS[1..].iter().enumerate() {
            a += c / (z + (i + 1) as f64);
        }
        0.5 * (std::f64::consts::LN_2 + std::f64::consts::PI.ln()) + (z + 0.5) * t.ln() - t + a.ln()
    }
}

/// ln B(a, b) for a, b > 0.
pub fn log_beta(a: f64, b: f64) -> f64 {
    log_gamma(a) + log_gamma(b) - log_gamma(a + b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: u64, impressions: u64, conversions: u64) -> VariantStats {
        // A deterministic id so reports are comparable.
        VariantStats {
            variant_id: VariantId::from_u128(id as u128),
            impressions,
            conversions,
        }
    }

    #[test]
    fn total_weight_sums_variants() {
        let exp = Experiment {
            id: ExperimentId::new_v4(),
            document_id: DocumentId::new_v4(),
            status: ExperimentStatus::Running,
            variants: vec![
                ExperimentVariant {
                    id: VariantId::new_v4(),
                    block_id: BlockId::new_v4(),
                    version_id: VersionId::new_v4(),
                    weight: 50.0,
                },
                ExperimentVariant {
                    id: VariantId::new_v4(),
                    block_id: BlockId::new_v4(),
                    version_id: VersionId::new_v4(),
                    weight: 50.0,
                },
            ],
            created_at_ms: 1,
        };
        assert_eq!(exp.total_weight(), 100.0);
    }

    #[test]
    fn prob_beta_lt_uniform_vs_uniform_is_half() {
        // X ~ Beta(1,1), Y ~ Beta(1,1): P(X < Y) = 1/2 by symmetry.
        let p = prob_beta_lt(1, 1, 1, 1);
        assert!((p - 0.5).abs() < 1e-9, "got {p}");
    }

    #[test]
    fn prob_beta_lt_matches_hand_computed() {
        // P(X < Y) where X ~ Beta(2,1) (mean 2/3), Y ~ Beta(1,1) (mean 1/2).
        // By symmetry this is 1 - P(Y < X); P(Y < X) with Y ~ U(0,1) is
        // E[P(Y < X)] = E[X] = 2/3, so P(X < Y) = 1/3.
        let p = prob_beta_lt(2, 1, 1, 1);
        assert!((p - 1.0 / 3.0).abs() < 1e-9, "got {p}");
    }

    #[test]
    fn prob_beta_lt_symmetric() {
        // P(X < Y) + P(Y < X) = 1 for non-atomic distributions.
        let p1 = prob_beta_lt(3, 5, 7, 2);
        let p2 = prob_beta_lt(7, 2, 3, 5);
        assert!((p1 + p2 - 1.0).abs() < 1e-9, "got {p1} + {p2}");
    }

    #[test]
    fn prob_beta_lt_matches_numeric() {
        // Numeric integration of the joint density over x < y.
        let numeric = |ax: f64, bx: f64, ay: f64, by: f64| {
            let n = 2000;
            let h = 1.0 / n as f64;
            let mut acc = 0.0;
            for i in 0..n {
                let x = (i as f64 + 0.5) * h;
                let fx = x.powf(ax - 1.0) * (1.0 - x).powf(bx - 1.0);
                let mut inner = 0.0;
                for j in 0..n {
                    let y = (j as f64 + 0.5) * h;
                    if y > x {
                        inner += y.powf(ay - 1.0) * (1.0 - y).powf(by - 1.0) * h;
                    }
                }
                acc += fx * inner * h;
            }
            let bx_b = |a: f64, b: f64| log_beta(a, b).exp();
            acc / (bx_b(ax, bx) * bx_b(ay, by))
        };
        let cases = [(2, 3, 4, 1), (5, 2, 2, 5), (10, 10, 1, 1)];
        for (ax, bx, ay, by) in cases {
            let exact = prob_beta_lt(ax, bx, ay, by);
            let approx = numeric(ax as f64, bx as f64, ay as f64, by as f64);
            assert!(
                (exact - approx).abs() < 1e-3,
                "prob_beta_lt({ax},{bx},{ay},{by}) = {exact}, numeric = {approx}"
            );
        }
    }

    #[test]
    fn beats_control_points_the_right_way() {
        // Control conversion 10/100 (10%), variant 30/100 (30%): variant is
        // overwhelmingly likely to beat control.
        let p = prob_variant_beats_control(1 + 10, 1 + 90, 1 + 30, 1 + 70);
        assert!(p > 0.999, "got {p}");
        // And the mirror image is near zero.
        let q = prob_variant_beats_control(1 + 30, 1 + 70, 1 + 10, 1 + 90);
        assert!(q < 0.001, "got {q}");
    }

    #[test]
    fn beta_cdf_known_values() {
        // Beta(1,1) is uniform: CDF(x) = x.
        assert!((beta_cdf(0.3, 1, 1) - 0.3).abs() < 1e-9);
        assert!((beta_cdf(0.7, 1, 1) - 0.7).abs() < 1e-9);
        // Beta(2,1): CDF(x) = x^2.
        assert!((beta_cdf(0.5, 2, 1) - 0.25).abs() < 1e-9);
        // Degenerate-ish: Beta(a,a) median is 0.5.
        assert!((beta_cdf(0.5, 10, 10) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn credible_interval_contains_posterior_mean_and_narrows() {
        let (a, b) = (1 + 100, 1 + 300); // observed 25% conversion
        let mean = a as f64 / (a + b) as f64;
        let (lo, hi) = credible_interval(a, b, 0.95);
        assert!(lo < mean && mean < hi, "mean {mean} not in ({lo}, {hi})");
        // More data => tighter interval.
        let (lo2, hi2) = credible_interval(1 + 1000, 1 + 3000, 0.95);
        assert!(hi2 - lo2 < hi - lo);
    }

    #[test]
    fn analyze_continues_with_no_data() {
        let report = analyze(v(0, 0, 0), &[v(1, 0, 0)], EngineConfig::default(), 0);
        assert_eq!(report.recommendation, Recommendation::Continue);
        assert_eq!(report.n_looks, 1);
        assert_eq!(report.adjusted_confidence_threshold, 0.95);
    }

    #[test]
    fn analyze_promotes_clear_winner() {
        let report = analyze(
            v(0, 200, 20),    // 10%
            &[v(1, 200, 80)], // 40%
            EngineConfig {
                min_sample_per_variant: 50,
                ..Default::default()
            },
            0,
        );
        match report.recommendation {
            Recommendation::Promote {
                variant_id,
                confidence,
            } => {
                assert_eq!(variant_id, VariantId::from_u128(1));
                assert!(confidence > 0.95);
            }
            other => panic!("expected Promote, got {other:?}"),
        }
    }

    #[test]
    fn analyze_waits_for_min_sample() {
        // Huge-looking effect but only 10 impressions: must not conclude.
        let report = analyze(v(0, 10, 1), &[v(1, 10, 9)], EngineConfig::default(), 0);
        assert_eq!(report.recommendation, Recommendation::Continue);
    }

    #[test]
    fn analyze_concludes_no_winner_when_below_no_winner_prob() {
        // Large samples, variant clearly worse than control (60% vs 20%).
        let report = analyze(
            v(0, 500, 300),
            &[v(1, 500, 100)],
            EngineConfig {
                min_sample_per_variant: 50,
                no_winner_prob: 0.05,
                ..Default::default()
            },
            0,
        );
        assert_eq!(report.recommendation, Recommendation::NoWinner);
    }

    #[test]
    fn analyze_no_winner_after_max_duration() {
        // Even with a modest positive signal, the hard stop fires.
        let report = analyze(
            v(0, 200, 60),
            &[v(1, 200, 70)],
            EngineConfig::default(),
            EngineConfig::default().max_duration_ms + 1,
        );
        assert_eq!(report.recommendation, Recommendation::NoWinner);
    }

    #[test]
    fn sequential_correction_tightens_threshold() {
        // More looks -> adjusted threshold rises above 0.95.
        let config = EngineConfig {
            min_sample_per_variant: 100,
            ..Default::default()
        };
        let r1 = analyze(v(0, 100, 10), &[v(1, 100, 20)], config, 0);
        let r2 = analyze(v(0, 1000, 100), &[v(1, 1000, 200)], config, 0);
        assert!(r1.adjusted_confidence_threshold < r2.adjusted_confidence_threshold);
        assert!(r2.adjusted_confidence_threshold > 0.95);
    }

    #[test]
    fn assignment_is_deterministic_and_respects_split() {
        let exp = ExperimentId::from_u128(7);
        let control = VariantId::from_u128(1);
        let a = VariantId::from_u128(2);
        let b = VariantId::from_u128(3);
        let mut control_count = 0u32;
        let mut a_count = 0u32;
        let mut b_count = 0u32;
        for i in 0..50_000u32 {
            let visitor = Uuid::from_u128(i as u128 + 1);
            let chosen = assign_variant(&exp, &visitor, control, 0.5, &[(a, 50.0), (b, 50.0)]);
            match chosen {
                id if id == control => control_count += 1,
                id if id == a => a_count += 1,
                id if id == b => b_count += 1,
                other => panic!("unexpected {other}"),
            }
            // Same visitor must get the same variant every time.
            let again = assign_variant(&exp, &visitor, control, 0.5, &[(a, 50.0), (b, 50.0)]);
            assert_eq!(chosen, again);
        }
        let total = (control_count + a_count + b_count) as f64;
        assert!(
            ((control_count as f64 / total) - 0.5).abs() < 0.02,
            "control {control_count}"
        );
        assert!(
            ((a_count as f64 / total) - 0.25).abs() < 0.02,
            "a {a_count}"
        );
        assert!(
            ((b_count as f64 / total) - 0.25).abs() < 0.02,
            "b {b_count}"
        );
    }

    #[test]
    fn assignment_with_zero_traffic_stays_on_control() {
        let exp = ExperimentId::from_u128(7);
        let control = VariantId::from_u128(1);
        for i in 0..100u32 {
            let visitor = Uuid::from_u128(i as u128 + 1);
            assert_eq!(assign_variant(&exp, &visitor, control, 1.0, &[]), control);
        }
    }

    #[test]
    fn log_gamma_sanity() {
        // Known: ln(5!) = ln(120).
        assert!((log_gamma(6.0) - 120.0f64.ln()).abs() < 1e-12);
        // ln(1) = 0.
        assert!(log_gamma(1.0).abs() < 1e-12);
        // ln(1/2!) = ln(sqrt(pi)/2)
        assert!((log_gamma(1.5) - (0.5 * std::f64::consts::PI.sqrt()).ln()).abs() < 1e-9);
    }
}
