//! Server-side domain records (users, sessions, comments, documents).

use openpublish_content::{BlockContent, BlockId, Document, VersionId};
use openpublish_experiments::{ExperimentId, VariantId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub created_at_ms: i64,
    /// Argon2 hash. Never serialized or exposed through the API.
    #[serde(skip)]
    pub password_hash: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    /// Raw bearer token stored in the cookie; only its SHA-256 is persisted.
    pub token: String,
    pub csrf: String,
    pub user_id: Uuid,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentSummary {
    pub id: Uuid,
    pub title: String,
    pub slug: String,
    pub status: String,
    pub published_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

/// A document plus its row-level metadata (owner, slug, status, publish time).
#[derive(Debug, Clone)]
pub struct FullDocument {
    pub document: Document,
    pub owner_id: Uuid,
    pub slug: String,
    pub status: String,
    pub published_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: Uuid,
    pub document_id: Uuid,
    pub author_name: String,
    pub body: String,
    pub status: String,
    pub created_at_ms: i64,
}

/// A single row of the append-only analytics event stream.
#[derive(Debug, Clone)]
pub struct AnalyticsEvent {
    pub id: Uuid,
    pub document_id: Uuid,
    /// Wire event type: `view` | `banded_scroll` | `article_read` |
    /// `block_impression` | `experiment_impression` | `experiment_conversion`.
    pub event_type: String,
    pub band: Option<i64>,
    pub block_id: Option<Uuid>,
    /// Client-generated, one per page load (a "session" of events).
    pub pageview_id: Uuid,
    /// Anonymous visitor from the `opv` cookie; used for unique-reader counts.
    pub visitor_id: Uuid,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub read_time_ms: Option<i64>,
    pub experiment_id: Option<Uuid>,
    pub variant_id: Option<Uuid>,
    pub created_at_ms: i64,
}

/// A row of the `experiments` table plus its variant rows.
#[derive(Debug, Clone)]
pub struct ExperimentRecord {
    pub id: ExperimentId,
    pub document_id: Uuid,
    pub block_id: BlockId,
    pub name: String,
    pub status: String,
    pub control_version_id: VersionId,
    pub goal: String,
    pub traffic_weight: f64,
    pub confidence_threshold: f64,
    pub min_sample_per_variant: i64,
    pub no_winner_prob: f64,
    pub max_duration_ms: i64,
    pub started_at_ms: Option<i64>,
    pub decided_at_ms: Option<i64>,
    pub decision: Option<String>,
    pub winning_variant_id: Option<VariantId>,
    pub created_at_ms: i64,
    pub variants: Vec<ExperimentVariantRecord>,
}

#[derive(Debug, Clone)]
pub struct ExperimentVariantRecord {
    pub id: VariantId,
    pub block_id: BlockId,
    pub version_id: VersionId,
    pub weight: f64,
    pub is_control: bool,
}

/// An experiment variant being created: its new content (a fresh immutable
/// version is written to the shared version pool) and its relative weight.
#[derive(Debug, Clone)]
pub struct ExperimentVariantInput {
    pub content: BlockContent,
    pub weight: f64,
}

/// Configuration + variants for creating an experiment.
#[derive(Debug, Clone)]
pub struct NewExperiment {
    pub name: String,
    pub goal: String,
    pub traffic_weight: f64,
    pub confidence_threshold: f64,
    pub min_sample_per_variant: u64,
    pub no_winner_prob: f64,
    pub max_duration_ms: i64,
    pub variants: Vec<ExperimentVariantInput>,
}

/// Per-variant sample counts for one experiment (deduped by visitor).
#[derive(Debug, Clone)]
pub struct ExperimentCounts {
    pub variant_id: VariantId,
    pub impressions: i64,
    pub conversions: i64,
}

/// One append-only conclusion row for an experiment.
#[derive(Debug, Clone)]
pub struct ExperimentDecision {
    pub id: Uuid,
    pub experiment_id: ExperimentId,
    pub decided_at_ms: i64,
    pub decision: String,
    pub winner_variant_id: Option<VariantId>,
    pub promoted_version_id: Option<VersionId>,
    pub effect_size: Option<f64>,
    pub confidence: Option<f64>,
    pub control_impressions: Option<i64>,
    pub control_conversions: Option<i64>,
    pub variant_impressions: Option<i64>,
    pub variant_conversions: Option<i64>,
}
