//! Storage-agnostic repository ports.
//!
//! The application never touches SQL. An implementation of [`Repository`]
//! (e.g. the SQLite one in `forgepost-infrastructure`) is injected at the
//! edges so routes and use cases never depend on a concrete backend (§5, §5.4).

use forgepost_analytics::{ArticleStats, BandReach};
use forgepost_content::{Block, BlockId, BlockVersion, DocumentId, ParsedBlock, VersionId};
use forgepost_domain::model::{
    AnalyticsEvent, Comment, DashboardMetric, DocumentSummary, ExperimentCounts,
    ExperimentDecision, ExperimentRecord, FullDocument, Media, NewExperiment, PublishedPost,
    SearchHit, Session, SiteSettings, User,
};
use forgepost_experiments::{ExperimentId, VariantId};

/// Resolve third-party video metadata (oEmbed) for newly pasted video blocks.
/// Application services need the enrichment; the infrastructure crate provides
/// the real HTTP-backed implementation.
#[async_trait::async_trait]
pub trait OEmbedResolver: Send + Sync {
    async fn enrich_video_metadata(&self, parsed: &mut [ParsedBlock]);
}

// ---------------------------------------------------------------------------
// Sub-traits: each service depends on the narrowest trait it needs.
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait UserRepo: Send + Sync {
    async fn is_setup_complete(&self) -> Result<bool, RepositoryError>;
    async fn create_first_user(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<User, RepositoryError>;
    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, RepositoryError>;
    async fn find_user_by_id(&self, id: uuid::Uuid) -> Result<Option<User>, RepositoryError>;
}

#[async_trait::async_trait]
pub trait SessionRepo: Send + Sync {
    async fn create_session(&self, user_id: uuid::Uuid) -> Result<Session, RepositoryError>;
    async fn session_by_token(&self, token: &str) -> Result<Option<Session>, RepositoryError>;
    async fn delete_session(&self, token: &str) -> Result<(), RepositoryError>;
}

#[async_trait::async_trait]
pub trait SettingsRepo: Send + Sync {
    async fn get_setting(&self, key: &str) -> Result<Option<String>, RepositoryError>;
    async fn set_setting(&self, key: &str, value: &str) -> Result<(), RepositoryError>;
    /// Read the blog-wide settings, applying the defaults for any unset keys.
    async fn site_settings(&self) -> Result<SiteSettings, RepositoryError>;
}

#[async_trait::async_trait]
pub trait DocumentRepo: Send + Sync {
    async fn list_documents(
        &self,
        owner_id: uuid::Uuid,
    ) -> Result<Vec<DocumentSummary>, RepositoryError>;
    async fn get_document(&self, id: DocumentId) -> Result<Option<FullDocument>, RepositoryError>;
    async fn create_document(
        &self,
        owner_id: uuid::Uuid,
        title: &str,
    ) -> Result<FullDocument, RepositoryError>;
    async fn update_document_title(
        &self,
        id: DocumentId,
        title: &str,
    ) -> Result<(), RepositoryError>;
    /// Regenerate the slug from `title` while the document is still a draft;
    /// once published the slug is stable, so this is a no-op for published
    /// documents. Returns true when the slug changed.
    async fn regenerate_draft_slug(
        &self,
        id: DocumentId,
        title: &str,
    ) -> Result<bool, RepositoryError>;
    async fn save_document_blocks(
        &self,
        id: DocumentId,
        blocks: &[Block],
        versions: &[BlockVersion],
    ) -> Result<(), RepositoryError>;
    async fn publish_document(&self, id: DocumentId) -> Result<(), RepositoryError>;
    /// Permanently remove a document. Cascades clear its blocks, block
    /// versions, tags, comments, experiments, and search index rows; analytics
    /// events survive with their `document_id` set to NULL.
    async fn delete_document(&self, id: DocumentId) -> Result<(), RepositoryError>;
    async fn get_published_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<FullDocument>, RepositoryError>;
    async fn list_published(&self) -> Result<Vec<DocumentSummary>, RepositoryError>;
    /// Published documents with their tags, newest first (blog home page).
    async fn list_published_with_tags(&self) -> Result<Vec<PublishedPost>, RepositoryError>;
    /// Published documents tagged `tag`, newest first, with their tags
    /// (per-tag listing page).
    async fn list_published_with_tag(
        &self,
        tag: &str,
    ) -> Result<Vec<PublishedPost>, RepositoryError>;
    /// All non-deleted documents regardless of status (used by `export`).
    async fn list_all_documents(&self) -> Result<Vec<DocumentSummary>, RepositoryError>;
    async fn set_document_tags(
        &self,
        id: DocumentId,
        tags: &[String],
    ) -> Result<(), RepositoryError>;
    async fn document_tags(&self, id: DocumentId) -> Result<Vec<String>, RepositoryError>;
}

#[async_trait::async_trait]
pub trait CommentRepo: Send + Sync {
    async fn create_comment(
        &self,
        document_id: DocumentId,
        author_name: &str,
        body: &str,
    ) -> Result<Comment, RepositoryError>;
    async fn comments_for_document(
        &self,
        document_id: DocumentId,
        status: Option<&str>,
    ) -> Result<Vec<Comment>, RepositoryError>;
    async fn pending_comments(&self) -> Result<Vec<Comment>, RepositoryError>;
    async fn set_comment_status(&self, id: uuid::Uuid, status: &str)
    -> Result<(), RepositoryError>;
}

#[async_trait::async_trait]
pub trait AnalyticsRepo: Send + Sync {
    async fn record_analytics_event(&self, event: &AnalyticsEvent) -> Result<(), RepositoryError>;
    async fn article_stats(&self, document_id: DocumentId)
    -> Result<ArticleStats, RepositoryError>;
    /// Distinct pageviews whose deepest scroll reached each band (cumulative).
    async fn band_reach(&self, document_id: DocumentId) -> Result<Vec<BandReach>, RepositoryError>;
    /// Distinct pageviews that rendered each block.
    async fn block_impressions(
        &self,
        document_id: DocumentId,
    ) -> Result<std::collections::HashMap<uuid::Uuid, i64>, RepositoryError>;
    /// Per-referrer distinct view pageviews for a document (Stats "traffic
    /// sources" table). Referrers are bucketed in `analytics::bucket_traffic_sources`.
    async fn referrer_counts(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<(Option<String>, i64)>, RepositoryError>;
    /// Per-document dashboard metrics for the last two 7-day windows plus
    /// lifetime views and completions. `now_ms` pins the window boundary so
    /// tests can drive results deterministically.
    async fn dashboard_metrics(&self, now_ms: i64)
    -> Result<Vec<DashboardMetric>, RepositoryError>;
}

#[async_trait::async_trait]
pub trait ExperimentRepo: Send + Sync {
    /// Create an experiment as an overlay on a block. Control is the block's
    /// current version; each variant writes a fresh immutable version to the
    /// shared pool without touching the block's canonical `current_version_id`.
    async fn create_experiment(
        &self,
        document_id: DocumentId,
        block_id: BlockId,
        new: &NewExperiment,
    ) -> Result<ExperimentRecord, RepositoryError>;
    async fn experiment(
        &self,
        id: ExperimentId,
    ) -> Result<Option<ExperimentRecord>, RepositoryError>;
    async fn experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<ExperimentRecord>, RepositoryError>;
    /// Experiments currently running for a document (article render overlay).
    async fn running_experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<ExperimentRecord>, RepositoryError>;
    /// All experiments with status `running` across every document (auto-decider).
    async fn running_experiments(&self) -> Result<Vec<ExperimentRecord>, RepositoryError>;
    async fn start_experiment(&self, id: ExperimentId) -> Result<(), RepositoryError>;
    async fn stop_experiment(&self, id: ExperimentId) -> Result<(), RepositoryError>;
    /// Repoint the block to `version_id` (promotion). Canonical content changes
    /// only here; the version pool itself is never mutated.
    async fn promote_block_version(
        &self,
        block_id: BlockId,
        version_id: VersionId,
    ) -> Result<(), RepositoryError>;
    /// Append a decision row and update the experiment status atomically.
    async fn conclude_experiment(
        &self,
        id: ExperimentId,
        decision: &str,
        winning_variant_id: Option<VariantId>,
        promoted_version_id: Option<VersionId>,
        stats: &ExperimentDecision,
    ) -> Result<(), RepositoryError>;
    /// Per-variant sample counts for a running experiment (deduped by visitor).
    async fn experiment_counts(
        &self,
        id: ExperimentId,
    ) -> Result<Vec<ExperimentCounts>, RepositoryError>;
    async fn experiment_decisions(
        &self,
        id: ExperimentId,
    ) -> Result<Vec<ExperimentDecision>, RepositoryError>;
    /// Confirm an experiment is running and that `variant_id` belongs to it.
    async fn experiment_variant_belongs(
        &self,
        id: ExperimentId,
        variant_id: VariantId,
    ) -> Result<bool, RepositoryError>;
}

#[async_trait::async_trait]
pub trait SearchRepo: Send + Sync {
    /// Search published documents, ranked by BM25. `query` is a plain string;
    /// the last token is treated as a prefix (as-you-type matching).
    async fn search_documents(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<SearchHit>, RepositoryError>;
    /// Re-index a single document (published only; drafts drop out of search).
    /// Safe to call any time; no-ops for missing documents.
    async fn refresh_search_index(&self, document_id: DocumentId) -> Result<(), RepositoryError>;
    /// Rebuild the index from scratch for every published document.
    async fn rebuild_search_index_all(&self) -> Result<(), RepositoryError>;
}

#[async_trait::async_trait]
pub trait MediaRepo: Send + Sync {
    /// Record an uploaded file. The caller writes the bytes to the media
    /// directory itself; this only persists the metadata row.
    async fn insert_media(&self, media: &Media) -> Result<(), RepositoryError>;
    /// Fetch media metadata by the on-disk name (e.g. `<uuid>.png`).
    async fn media_by_disk_name(&self, disk_name: &str) -> Result<Option<Media>, RepositoryError>;
}

#[async_trait::async_trait]
pub trait ExportRepo: Send + Sync {
    async fn export_json(&self) -> Result<serde_json::Value, RepositoryError>;
}

// ---------------------------------------------------------------------------
// Composite trait: the full storage surface used by AppState and route
// handlers that touch multiple domains. Services should depend on the
// narrowest sub-trait instead.
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait Repository:
    UserRepo
    + SessionRepo
    + SettingsRepo
    + DocumentRepo
    + CommentRepo
    + AnalyticsRepo
    + ExperimentRepo
    + SearchRepo
    + MediaRepo
    + ExportRepo
    + Send
    + Sync
{
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid uuid: {0}")]
    Uuid(#[from] uuid::Error),
    #[error("rate limited")]
    RateLimited,
}
