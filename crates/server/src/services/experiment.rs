//! Experiment service: create, start/stop, decide, promote.

use std::sync::Arc;

use uuid::Uuid;

use crate::model::{ExperimentRecord, ExperimentVariantInput, NewExperiment};
use crate::repository::{DocumentRepo, ExperimentRepo, Repository};
use crate::services::ServiceError;

pub struct ExperimentService {
    exp_repo: Arc<dyn ExperimentRepo>,
    doc_repo: Arc<dyn DocumentRepo>,
}

impl ExperimentService {
    pub fn new(repo: Arc<dyn Repository>) -> Self {
        Self {
            exp_repo: repo.clone(),
            doc_repo: repo,
        }
    }

    /// All experiments for a document, with live reports.
    pub async fn list_for_document(
        &self,
        document_id: Uuid,
        owner_id: Uuid,
    ) -> Result<Vec<ExperimentRecord>, ServiceError> {
        let _full = self.verify_document_owner(document_id, owner_id).await?;
        Ok(self.exp_repo.experiments_for_document(document_id).await?)
    }

    /// Create an experiment overlay on a block.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        document_id: Uuid,
        block_id: Uuid,
        owner_id: Uuid,
        name: &str,
        goal: &str,
        traffic_weight: f64,
        confidence_threshold: f64,
        min_sample_per_variant: u64,
        no_winner_prob: f64,
        max_duration_ms: i64,
        variants: Vec<ExperimentVariantInput>,
    ) -> Result<ExperimentRecord, ServiceError> {
        let full = self.verify_document_owner(document_id, owner_id).await?;
        let block = full
            .document
            .block(block_id)
            .ok_or_else(|| ServiceError::Validation("block not found in document".into()))?;
        if !block.kind.is_experimentable() {
            return Err(ServiceError::Validation(
                "this block kind cannot be tested (use a heading, paragraph, image, or CTA)".into(),
            ));
        }
        if variants.is_empty() {
            return Err(ServiceError::Validation(
                "at least one variant is required".into(),
            ));
        }
        if variants.iter().any(|v| v.weight <= 0.0) {
            return Err(ServiceError::Validation(
                "variant weights must be positive".into(),
            ));
        }
        if !(0.0..=100.0).contains(&traffic_weight) {
            return Err(ServiceError::Validation(
                "traffic weight must be 0–100".into(),
            ));
        }
        let new = NewExperiment {
            name: name.trim().to_string(),
            goal: goal.to_string(),
            traffic_weight,
            confidence_threshold,
            min_sample_per_variant,
            no_winner_prob,
            max_duration_ms,
            variants,
        };
        Ok(self
            .exp_repo
            .create_experiment(document_id, block_id, &new)
            .await?)
    }

    /// Create an experiment from the form-based page input (3 hardcoded variant slots).
    pub async fn create_from_form(
        &self,
        document_id: Uuid,
        owner_id: Uuid,
        name: &str,
        block_id: Uuid,
        traffic_weight: f64,
        variant_contents: &[(&str, f64)],
    ) -> Result<ExperimentRecord, ServiceError> {
        let variants: Vec<ExperimentVariantInput> = variant_contents
            .iter()
            .filter(|(content, _)| !content.trim().is_empty())
            .map(|(content, weight)| ExperimentVariantInput {
                content: serde_json::json!({ "text": content.trim() }),
                weight: *weight,
            })
            .collect();
        if variants.is_empty() {
            return Err(ServiceError::Validation("variant_required".into()));
        }
        self.create(
            document_id,
            block_id,
            owner_id,
            name,
            "completion",
            traffic_weight,
            0.95,
            100,
            0.05,
            30 * 24 * 60 * 60 * 1000,
            variants,
        )
        .await
    }

    /// Start a draft experiment.
    pub async fn start(&self, id: Uuid, owner_id: Uuid) -> Result<(), ServiceError> {
        let exp = self.verify_experiment_owner(id, owner_id).await?;
        tracing::info!(experiment_id = %exp.id, "starting experiment");
        self.exp_repo.start_experiment(exp.id).await?;
        Ok(())
    }

    /// Stop a running experiment.
    pub async fn stop(&self, id: Uuid, owner_id: Uuid) -> Result<(), ServiceError> {
        let exp = self.verify_experiment_owner(id, owner_id).await?;
        tracing::info!(experiment_id = %exp.id, "stopping experiment");
        self.exp_repo.stop_experiment(exp.id).await?;
        Ok(())
    }

    /// Run the sequential-test rules (auto or manual decide).
    pub async fn decide(
        &self,
        id: Uuid,
        owner_id: Uuid,
    ) -> Result<Option<crate::experiments::DecisionOutcome>, ServiceError> {
        let exp = self.verify_experiment_owner(id, owner_id).await?;
        tracing::info!(experiment_id = %exp.id, "deciding experiment");
        Ok(crate::experiments::decide_experiment(&*self.exp_repo, exp.id).await?)
    }

    /// Manual override: promote the current best variant.
    pub async fn promote(
        &self,
        id: Uuid,
        owner_id: Uuid,
    ) -> Result<crate::experiments::DecisionOutcome, ServiceError> {
        let exp = self.verify_experiment_owner(id, owner_id).await?;
        Ok(crate::experiments::promote_experiment(&*self.exp_repo, exp.id).await?)
    }

    /// Manual override: conclude "no improvement".
    pub async fn conclude_no_winner(
        &self,
        id: Uuid,
        owner_id: Uuid,
    ) -> Result<crate::experiments::DecisionOutcome, ServiceError> {
        let exp = self.verify_experiment_owner(id, owner_id).await?;
        Ok(crate::experiments::conclude_no_winner(&*self.exp_repo, exp.id).await?)
    }

    // ------------------------------------------------------------------
    // Ownership verification helpers
    // ------------------------------------------------------------------

    /// Verify the document exists and belongs to `owner_id`.
    async fn verify_document_owner(
        &self,
        document_id: Uuid,
        owner_id: Uuid,
    ) -> Result<crate::model::FullDocument, ServiceError> {
        let full = self
            .doc_repo
            .get_document(document_id)
            .await?
            .ok_or_else(|| ServiceError::Validation("document not found".into()))?;
        if full.owner_id != owner_id {
            return Err(ServiceError::Forbidden);
        }
        Ok(full)
    }

    /// Verify the experiment exists and its document belongs to `owner_id`.
    async fn verify_experiment_owner(
        &self,
        experiment_id: Uuid,
        owner_id: Uuid,
    ) -> Result<ExperimentRecord, ServiceError> {
        let exp = self
            .exp_repo
            .experiment(experiment_id)
            .await?
            .ok_or_else(|| ServiceError::Validation("experiment not found".into()))?;
        self.verify_document_owner(exp.document_id, owner_id)
            .await?;
        Ok(exp)
    }
}
