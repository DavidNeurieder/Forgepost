//! A/B experiment overlay over immutable block versions.
//!
//! Experiments are overlays: each variant points at an immutable version in the
//! shared version pool (§5.1). Scaffolded in M0; the Bayesian + sequential-test
//! engine, traffic split, no-winner rule, and `experiment_decisions` land in M3.

use openpublish_content::{BlockId, DocumentId, VersionId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type ExperimentId = Uuid;
pub type VariantId = Uuid;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
