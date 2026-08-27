//! Server-facing domain records: posts, comments, events, experiments, media.
//! These are pure value objects — no SQL, Axum, or filesystem imports.

use forgepost_content::{BlockContent, BlockId, Document, VersionId};
use forgepost_experiments::{ExperimentId, VariantId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Domain identifiers
// ---------------------------------------------------------------------------

/// Identifier of a blog post (the same key as a [`Document`] row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PostId(pub Uuid);

impl From<Uuid> for PostId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<PostId> for Uuid {
    fn from(id: PostId) -> Self {
        id.0
    }
}

impl std::fmt::Display for PostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifier of an anonymous visitor (the `opv` cookie value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VisitorId(pub Uuid);

impl From<Uuid> for VisitorId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<VisitorId> for Uuid {
    fn from(id: VisitorId) -> Self {
        id.0
    }
}

impl std::fmt::Display for VisitorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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

/// Blog-wide appearance settings read from the generic `settings` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteSettings {
    /// Displayed in the header brand, page titles, and the RSS feed title.
    pub name: String,
    /// `system` | `light` | `dark` | `sepia` | `solarized`.
    pub theme: String,
    /// Public base URL (e.g. `https://example.com`) used for canonical links,
    /// Open Graph, sitemap, robots, and RSS links. Empty until configured.
    pub url: String,
    /// One-line site description used as the home page meta description.
    pub tagline: String,
    /// Default social-preview image (absolute or base-relative URL) used as
    /// the Open Graph / Twitter card image for the home page and for any
    /// published post that contains no image. Empty until configured.
    pub image: String,
    /// Whether readers can leave comments. Disabled by default.
    pub comments_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentSummary {
    pub id: PostId,
    pub title: String,
    pub slug: String,
    pub status: String,
    pub published_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// A published document plus its tags, as listed on the blog home page.
#[derive(Debug, Clone)]
pub struct PublishedPost {
    pub id: PostId,
    pub title: String,
    pub slug: String,
    pub published_at_ms: Option<i64>,
    pub tags: Vec<String>,
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
    pub document_id: PostId,
    pub author_name: String,
    pub body: String,
    pub status: String,
    pub created_at_ms: i64,
}

/// A single row of the append-only analytics event stream.
#[derive(Debug, Clone)]
pub struct AnalyticsEvent {
    pub id: Uuid,
    pub document_id: PostId,
    /// Wire event type: `view` | `banded_scroll` | `article_read` |
    /// `block_impression` | `experiment_impression` | `experiment_conversion` |
    /// `recommendation_impression` | `recommendation_click` | `share_click`.
    pub event_type: String,
    pub band: Option<i64>,
    pub block_id: Option<Uuid>,
    /// Client-generated, one per page load (a "session" of events).
    pub pageview_id: Uuid,
    /// Anonymous visitor from the `opv` cookie; used for unique-reader counts.
    pub visitor_id: VisitorId,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub read_time_ms: Option<i64>,
    pub experiment_id: Option<Uuid>,
    pub variant_id: Option<Uuid>,
    /// Slug of the article shown/clicked in "Keep reading"
    /// (`recommendation_impression` / `recommendation_click` only).
    pub recommended_slug: Option<String>,
    pub created_at_ms: i64,
}

/// Per-document dashboard metrics for the game-feel "last 7 days" view.
#[derive(Debug, Clone, Default)]
pub struct DashboardMetric {
    pub document_id: PostId,
    /// Distinct view pageviews in the last 7 days.
    pub views_7d: i64,
    /// Distinct view pageviews in the 7 days before that.
    pub views_prev_7d: i64,
    /// Lifetime distinct view pageviews.
    pub views_total: i64,
    /// Lifetime pageviews that scrolled to 100% (band 100).
    pub completed: i64,
}

/// A row of the `experiments` table plus its variant rows.
#[derive(Debug, Clone)]
pub struct ExperimentRecord {
    pub id: ExperimentId,
    pub document_id: PostId,
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

/// One uploaded media file. The bytes live on disk in the media directory;
/// `disk_name` is always the generated UUID plus a canonical extension, never
/// the client's original filename.
#[derive(Debug, Clone)]
pub struct Media {
    pub id: Uuid,
    pub disk_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub created_at_ms: i64,
}

/// A full-text search hit against the FTS5 index.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub document_id: PostId,
    pub slug: String,
    pub title: String,
    pub published_at_ms: Option<i64>,
    pub tags: Vec<String>,
    /// HTML fragment from `snippet()` with `<mark>` around matching terms.
    pub snippet: String,
}
