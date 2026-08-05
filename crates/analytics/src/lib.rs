//! Event collection + per-block metrics.
//!
//! Scaffolded in M0 with the `EventSink` boundary (§5.4): analytics writes go
//! through an interface so a bulk store (ClickHouse) can replace the SQLite
//! writer later without touching the domain. The full event pipeline and
//! per-block aggregations land in M2.

use async_trait::async_trait;
use openpublish_content::{BlockId, DocumentId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type SessionId = Uuid;
pub type EventId = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    View,
    BandedScroll,
    BlockImpression,
    BlockCompletion,
    ArticleRead,
    ExperimentImpression,
    ExperimentConversion,
    Referral,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_id: EventId,
    pub session_id: SessionId,
    pub document_id: DocumentId,
    pub block_id: Option<BlockId>,
    pub kind: EventKind,
    /// Unix timestamp in milliseconds.
    pub occurred_at_ms: i64,
    /// Kind-specific payload (band, referral source, experiment id, ...).
    pub payload: serde_json::Value,
}

/// Boundary for analytics writes (§5.4). Storage behind this trait can be
/// swapped without touching domain logic.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn record(&self, event: RawEvent) -> Result<(), AnalyticsError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("invalid event: {0}")]
    InvalidEvent(String),
}
