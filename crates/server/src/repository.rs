//! Storage-agnostic repository layer.
//!
//! The domain never touches SQL. A Postgres implementation of [`Repository`]
//! can be added later without touching routes or domain logic (§5, §5.4).

use crate::analytics::{ArticleStats, BandReach};
use crate::auth::{SESSION_TTL_MS, sha256_hex};
use crate::model::{AnalyticsEvent, Comment, DocumentSummary, FullDocument, Session, User};
use async_trait::async_trait;
use openpublish_content::{
    Block, BlockContent, BlockId, BlockKind, BlockVersion, Document, DocumentId, VersionId, now_ms,
};
use openpublish_experiments::{ExperimentId, VariantId};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

#[async_trait]
pub trait Repository: Send + Sync {
    // Setup / users
    async fn is_setup_complete(&self) -> Result<bool, RepositoryError>;
    async fn create_first_user(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<User, RepositoryError>;
    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, RepositoryError>;
    async fn find_user_by_id(&self, id: Uuid) -> Result<Option<User>, RepositoryError>;

    // Sessions
    async fn create_session(&self, user_id: Uuid) -> Result<Session, RepositoryError>;
    async fn session_by_token(&self, token: &str) -> Result<Option<Session>, RepositoryError>;
    async fn delete_session(&self, token: &str) -> Result<(), RepositoryError>;

    // Documents
    async fn list_documents(&self, owner_id: Uuid)
    -> Result<Vec<DocumentSummary>, RepositoryError>;
    async fn get_document(&self, id: DocumentId) -> Result<Option<FullDocument>, RepositoryError>;
    async fn create_document(
        &self,
        owner_id: Uuid,
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
    async fn get_published_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<FullDocument>, RepositoryError>;
    async fn list_published(&self) -> Result<Vec<DocumentSummary>, RepositoryError>;
    /// All non-deleted documents regardless of status (used by `export`).
    async fn list_all_documents(&self) -> Result<Vec<DocumentSummary>, RepositoryError>;
    // Tags
    async fn set_document_tags(
        &self,
        id: DocumentId,
        tags: &[String],
    ) -> Result<(), RepositoryError>;
    async fn document_tags(&self, id: DocumentId) -> Result<Vec<String>, RepositoryError>;

    // Comments
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
    async fn set_comment_status(&self, id: Uuid, status: &str) -> Result<(), RepositoryError>;

    // Export
    async fn export_json(&self) -> Result<serde_json::Value, RepositoryError>;

    // Analytics (M2)
    async fn record_analytics_event(&self, event: &AnalyticsEvent) -> Result<(), RepositoryError>;
    async fn article_stats(&self, document_id: DocumentId)
    -> Result<ArticleStats, RepositoryError>;
    /// Distinct pageviews whose deepest scroll reached each band (cumulative).
    async fn band_reach(&self, document_id: DocumentId) -> Result<Vec<BandReach>, RepositoryError>;
    /// Distinct pageviews that rendered each block.
    async fn block_impressions(
        &self,
        document_id: DocumentId,
    ) -> Result<std::collections::HashMap<Uuid, i64>, RepositoryError>;

    // Experiments (M3)
    /// Create an experiment as an overlay on a block. Control is the block's
    /// current version; each variant writes a fresh immutable version to the
    /// shared pool without touching the block's canonical `current_version_id`.
    async fn create_experiment(
        &self,
        document_id: DocumentId,
        block_id: BlockId,
        new: &crate::model::NewExperiment,
    ) -> Result<crate::model::ExperimentRecord, RepositoryError>;
    async fn experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<Option<crate::model::ExperimentRecord>, RepositoryError>;
    async fn experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<crate::model::ExperimentRecord>, RepositoryError>;
    /// Experiments currently running for a document (article render overlay).
    async fn running_experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<crate::model::ExperimentRecord>, RepositoryError>;
    /// All experiments with status `running` across every document (auto-decider).
    async fn running_experiments(
        &self,
    ) -> Result<Vec<crate::model::ExperimentRecord>, RepositoryError>;
    async fn start_experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<(), RepositoryError>;
    async fn stop_experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<(), RepositoryError>;
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
        id: openpublish_experiments::ExperimentId,
        decision: &str,
        winning_variant_id: Option<openpublish_experiments::VariantId>,
        promoted_version_id: Option<VersionId>,
        stats: &crate::model::ExperimentDecision,
    ) -> Result<(), RepositoryError>;
    /// Per-variant sample counts for a running experiment (deduped by visitor).
    async fn experiment_counts(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<Vec<crate::model::ExperimentCounts>, RepositoryError>;
    async fn experiment_decisions(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<Vec<crate::model::ExperimentDecision>, RepositoryError>;
    /// Confirm an experiment is running and that `variant_id` belongs to it.
    async fn experiment_variant_belongs(
        &self,
        id: openpublish_experiments::ExperimentId,
        variant_id: openpublish_experiments::VariantId,
    ) -> Result<bool, RepositoryError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
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

/// SQLite-backed repository (solo mode, the only MVP distribution).
pub struct SqliteRepository {
    pool: SqlitePool,
}

impl SqliteRepository {
    /// Connect to a SQLite database, creating the file if needed.
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Run pending migrations from the workspace `migrations/` directory.
    pub async fn migrate(&self) -> Result<(), RepositoryError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: Uuid::from_str(&row.get::<String, _>("id")).unwrap_or_default(),
        email: row.get("email"),
        display_name: row.get("display_name"),
        role: row.get("role"),
        created_at_ms: row.get("created_at_ms"),
        password_hash: row.get("password_hash"),
    }
}

fn row_to_document_summary(row: &sqlx::sqlite::SqliteRow) -> DocumentSummary {
    DocumentSummary {
        id: Uuid::from_str(&row.get::<String, _>("id")).unwrap_or_default(),
        title: row.get("title"),
        slug: row.get("slug"),
        status: row.get("status"),
        published_at_ms: row.get("published_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
    }
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for c in title.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug
    }
}

async fn next_slug(
    conn: &mut Transaction<'_, sqlx::sqlite::Sqlite>,
    owner_id: Uuid,
    base: &str,
) -> Result<String, RepositoryError> {
    let candidate = base.to_string();
    let exists: i64 = sqlx::query("SELECT COUNT(*) FROM documents WHERE owner_id = ? AND slug = ?")
        .bind(owner_id.to_string())
        .bind(&candidate)
        .fetch_one(&mut **conn)
        .await?
        .get(0);
    if exists == 0 {
        return Ok(candidate);
    }
    for i in 2..1000 {
        let c = format!("{base}-{i}");
        let exists: i64 =
            sqlx::query("SELECT COUNT(*) FROM documents WHERE owner_id = ? AND slug = ?")
                .bind(owner_id.to_string())
                .bind(&c)
                .fetch_one(&mut **conn)
                .await?
                .get(0);
        if exists == 0 {
            return Ok(c);
        }
    }
    Err(RepositoryError::Conflict(
        "could not generate a unique slug".into(),
    ))
}

#[async_trait]
impl Repository for SqliteRepository {
    async fn is_setup_complete(&self) -> Result<bool, RepositoryError> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = 'setup.complete'")
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => r.get::<String, _>("value") == "1",
            None => false,
        })
    }

    async fn create_first_user(
        &self,
        email: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<User, RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, display_name, role) VALUES (?, ?, ?, ?, 'owner')",
        )
        .bind(id.to_string())
        .bind(email)
        .bind(password_hash)
        .bind(display_name)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('setup.complete', '1')
             ON CONFLICT(key) DO UPDATE SET value = '1'",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(User {
            id,
            email: email.to_string(),
            display_name: display_name.to_string(),
            role: "owner".into(),
            created_at_ms: now_ms(),
            password_hash: password_hash.to_string(),
        })
    }

    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, email, password_hash, display_name, role, created_at_ms FROM users WHERE email = ?",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| row_to_user(&r)))
    }

    async fn find_user_by_id(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, email, password_hash, display_name, role, created_at_ms FROM users WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| row_to_user(&r)))
    }

    async fn create_session(&self, user_id: Uuid) -> Result<Session, RepositoryError> {
        let token = Uuid::new_v4().to_string();
        let csrf = Uuid::new_v4().to_string();
        let token_hash = sha256_hex(&token);
        let expires_at_ms = now_ms() + SESSION_TTL_MS;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, csrf_token, expires_at_ms) VALUES (?, ?, ?, ?)")
            .bind(token_hash)
            .bind(user_id.to_string())
            .bind(&csrf)
            .bind(expires_at_ms)
            .execute(&self.pool)
            .await?;
        Ok(Session {
            token,
            csrf,
            user_id,
            expires_at_ms,
        })
    }

    async fn session_by_token(&self, token: &str) -> Result<Option<Session>, RepositoryError> {
        let token_hash = sha256_hex(token);
        let now = now_ms();
        let row = sqlx::query(
            "SELECT token_hash, user_id, csrf_token, expires_at_ms FROM sessions WHERE token_hash = ? AND expires_at_ms > ?",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Session {
            token: token.to_string(),
            user_id: Uuid::from_str(&r.get::<String, _>("user_id")).unwrap_or_default(),
            csrf: r.get("csrf_token"),
            expires_at_ms: r.get("expires_at_ms"),
        }))
    }

    async fn delete_session(&self, token: &str) -> Result<(), RepositoryError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(sha256_hex(token))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_documents(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<DocumentSummary>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, slug, status, published_at_ms, updated_at_ms
             FROM documents WHERE owner_id = ? AND deleted_at_ms IS NULL
             ORDER BY updated_at_ms DESC",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_document_summary).collect())
    }

    async fn list_published(&self) -> Result<Vec<DocumentSummary>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, slug, status, published_at_ms, updated_at_ms
             FROM documents WHERE status = 'published' AND deleted_at_ms IS NULL
             ORDER BY published_at_ms DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_document_summary).collect())
    }

    async fn list_all_documents(&self) -> Result<Vec<DocumentSummary>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, slug, status, published_at_ms, updated_at_ms
             FROM documents WHERE deleted_at_ms IS NULL
             ORDER BY updated_at_ms DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_document_summary).collect())
    }

    async fn get_document(&self, id: DocumentId) -> Result<Option<FullDocument>, RepositoryError> {
        let doc_row = sqlx::query(
            "SELECT id, owner_id, title, slug, status, published_at_ms, deleted_at_ms, created_at_ms, updated_at_ms
             FROM documents WHERE id = ? AND deleted_at_ms IS NULL",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some(doc_row) = doc_row else {
            return Ok(None);
        };
        let owner_id = Uuid::from_str(&doc_row.get::<String, _>("owner_id")).unwrap_or_default();
        let title: String = doc_row.get("title");
        let slug: String = doc_row.get("slug");
        let status: String = doc_row.get("status");
        let published_at_ms: Option<i64> = doc_row.get("published_at_ms");
        let created_at_ms: i64 = doc_row.get("created_at_ms");
        let updated_at_ms: i64 = doc_row.get("updated_at_ms");

        let block_rows = sqlx::query(
            "SELECT id, kind, position, current_version_id, created_at_ms, updated_at_ms
             FROM blocks WHERE document_id = ? ORDER BY position",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let blocks: Vec<Block> = block_rows
            .iter()
            .map(|r| Block {
                id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
                kind: serde_json::from_str(&r.get::<String, _>("kind"))
                    .unwrap_or(BlockKind::Paragraph),
                version_id: Uuid::from_str(&r.get::<String, _>("current_version_id"))
                    .unwrap_or_default(),
                position: r.get("position"),
                created_at_ms: r.get("created_at_ms"),
                updated_at_ms: r.get("updated_at_ms"),
            })
            .collect();

        let version_rows = sqlx::query(
            "SELECT v.id, v.block_id, v.content_json, v.created_at_ms
             FROM block_versions v JOIN blocks b ON b.id = v.block_id
             WHERE b.document_id = ? ORDER BY v.created_at_ms",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let versions: Vec<BlockVersion> = version_rows
            .iter()
            .map(|r| BlockVersion {
                id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
                block_id: Uuid::from_str(&r.get::<String, _>("block_id")).unwrap_or_default(),
                content: serde_json::from_str(&r.get::<String, _>("content_json"))
                    .unwrap_or(BlockContent::Null),
                created_at_ms: r.get("created_at_ms"),
            })
            .collect();

        Ok(Some(FullDocument {
            document: Document {
                id,
                title,
                blocks,
                versions,
                created_at_ms,
                updated_at_ms,
            },
            owner_id,
            slug,
            status,
            published_at_ms,
        }))
    }

    async fn get_published_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<FullDocument>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id FROM documents WHERE slug = ? AND status = 'published' AND deleted_at_ms IS NULL LIMIT 1",
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: String = row.get("id");
        self.get_document(Uuid::from_str(&id).unwrap_or_default())
            .await
    }

    async fn create_document(
        &self,
        owner_id: Uuid,
        title: &str,
    ) -> Result<FullDocument, RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let id = Uuid::new_v4();
        let slug = next_slug(&mut tx, owner_id, &slugify(title)).await?;
        let now = now_ms();
        sqlx::query(
            "INSERT INTO documents (id, owner_id, title, slug, status, created_at_ms, updated_at_ms)
             VALUES (?, ?, ?, ?, 'draft', ?, ?)",
        )
        .bind(id.to_string())
        .bind(owner_id.to_string())
        .bind(title)
        .bind(&slug)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(FullDocument {
            document: Document {
                id,
                title: title.to_string(),
                blocks: Vec::new(),
                versions: Vec::new(),
                created_at_ms: now,
                updated_at_ms: now,
            },
            owner_id,
            slug,
            status: "draft".into(),
            published_at_ms: None,
        })
    }

    async fn update_document_title(
        &self,
        id: DocumentId,
        title: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE documents SET title = ?, updated_at_ms = ? WHERE id = ?")
            .bind(title)
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn regenerate_draft_slug(
        &self,
        id: DocumentId,
        title: &str,
    ) -> Result<bool, RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let (owner_id, status): (String, String) =
            sqlx::query_as("SELECT owner_id, status FROM documents WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if status != "draft" {
            return Ok(false);
        }
        let owner_id = Uuid::parse_str(&owner_id)
            .map_err(|_| RepositoryError::Conflict("invalid owner_id in documents row".into()))?;
        let slug = next_slug(&mut tx, owner_id, &slugify(title)).await?;
        sqlx::query("UPDATE documents SET slug = ?, updated_at_ms = ? WHERE id = ?")
            .bind(&slug)
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn save_document_blocks(
        &self,
        id: DocumentId,
        blocks: &[Block],
        versions: &[BlockVersion],
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        // Park existing blocks out of the unique (document_id, position) space
        // first, so renumbering (e.g. inserting at position 0) cannot collide
        // transiently while other rows still hold the old positions.
        sqlx::query("UPDATE blocks SET position = -(position + 1) WHERE document_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        for block in blocks {
            let kind = serde_json::to_string(&block.kind).unwrap_or_default();
            let updated: Option<(String,)> =
                sqlx::query_as("SELECT current_version_id FROM blocks WHERE id = ?")
                    .bind(block.id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?;
            match updated {
                Some(_) => {
                    sqlx::query(
                        "UPDATE blocks SET kind = ?, position = ?, current_version_id = ?, updated_at_ms = ? WHERE id = ?",
                    )
                    .bind(&kind)
                    .bind(block.position)
                    .bind(block.version_id.to_string())
                    .bind(block.updated_at_ms)
                    .bind(block.id.to_string())
                    .execute(&mut *tx)
                    .await?;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO blocks (id, document_id, kind, position, current_version_id, created_at_ms, updated_at_ms)
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(block.id.to_string())
                    .bind(id.to_string())
                    .bind(&kind)
                    .bind(block.position)
                    .bind(block.version_id.to_string())
                    .bind(block.created_at_ms)
                    .bind(block.updated_at_ms)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        for version in versions {
            sqlx::query(
                "INSERT INTO block_versions (id, block_id, content_json, created_at_ms) VALUES (?, ?, ?, ?)",
            )
            .bind(version.id.to_string())
            .bind(version.block_id.to_string())
            .bind(version.content.to_string())
            .bind(version.created_at_ms)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn publish_document(&self, id: DocumentId) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE documents SET status = 'published', published_at_ms = ?, updated_at_ms = ? WHERE id = ?")
            .bind(now_ms())
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_document_tags(
        &self,
        id: DocumentId,
        tags: &[String],
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM document_tags WHERE document_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        for slug in tags {
            let slug = slug.trim().trim_start_matches('#').to_lowercase();
            if slug.is_empty() {
                continue;
            }
            sqlx::query("INSERT INTO tags (id, slug) VALUES (?, ?) ON CONFLICT(slug) DO NOTHING")
                .bind(Uuid::new_v4().to_string())
                .bind(&slug)
                .execute(&mut *tx)
                .await?;
            let tag_row = sqlx::query("SELECT id FROM tags WHERE slug = ?")
                .bind(&slug)
                .fetch_one(&mut *tx)
                .await?;
            let tag_id: String = tag_row.get("id");
            sqlx::query("INSERT INTO document_tags (document_id, tag_id) VALUES (?, ?) ON CONFLICT DO NOTHING")
                .bind(id.to_string())
                .bind(&tag_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn document_tags(&self, id: DocumentId) -> Result<Vec<String>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT t.slug FROM tags t JOIN document_tags dt ON dt.tag_id = t.id WHERE dt.document_id = ? ORDER BY t.slug",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| r.get::<String, _>("slug")).collect())
    }

    async fn create_comment(
        &self,
        document_id: DocumentId,
        author_name: &str,
        body: &str,
    ) -> Result<Comment, RepositoryError> {
        let id = Uuid::new_v4();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO comments (id, document_id, author_name, body, status, created_at_ms) VALUES (?, ?, ?, ?, 'pending', ?)",
        )
        .bind(id.to_string())
        .bind(document_id.to_string())
        .bind(author_name)
        .bind(body)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(Comment {
            id,
            document_id,
            author_name: author_name.to_string(),
            body: body.to_string(),
            status: "pending".into(),
            created_at_ms: now,
        })
    }

    async fn comments_for_document(
        &self,
        document_id: DocumentId,
        status: Option<&str>,
    ) -> Result<Vec<Comment>, RepositoryError> {
        let rows =
            match status {
                Some(s) => sqlx::query(
                    "SELECT id, document_id, author_name, body, status, created_at_ms FROM comments
                     WHERE document_id = ? AND status = ? ORDER BY created_at_ms",
                )
                .bind(document_id.to_string())
                .bind(s)
                .fetch_all(&self.pool)
                .await?,
                None => sqlx::query(
                    "SELECT id, document_id, author_name, body, status, created_at_ms FROM comments
                     WHERE document_id = ? ORDER BY created_at_ms",
                )
                .bind(document_id.to_string())
                .fetch_all(&self.pool)
                .await?,
            };
        Ok(rows
            .iter()
            .map(|r| Comment {
                id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
                document_id: Uuid::from_str(&r.get::<String, _>("document_id")).unwrap_or_default(),
                author_name: r.get("author_name"),
                body: r.get("body"),
                status: r.get("status"),
                created_at_ms: r.get("created_at_ms"),
            })
            .collect())
    }

    async fn set_comment_status(&self, id: Uuid, status: &str) -> Result<(), RepositoryError> {
        let result = sqlx::query("UPDATE comments SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound("comment".into()));
        }
        Ok(())
    }

    async fn pending_comments(&self) -> Result<Vec<Comment>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, document_id, author_name, body, status, created_at_ms FROM comments
             WHERE status = 'pending' ORDER BY created_at_ms ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| Comment {
                id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
                document_id: Uuid::from_str(&r.get::<String, _>("document_id")).unwrap_or_default(),
                author_name: r.get("author_name"),
                body: r.get("body"),
                status: r.get("status"),
                created_at_ms: r.get("created_at_ms"),
            })
            .collect())
    }

    async fn export_json(&self) -> Result<serde_json::Value, RepositoryError> {
        let settings: Vec<(String, String, i64)> =
            sqlx::query_as("SELECT key, value, updated_at FROM settings")
                .fetch_all(&self.pool)
                .await?;
        let users: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT id, email, password_hash, display_name, role, created_at_ms FROM users",
        )
        .fetch_all(&self.pool)
        .await?;
        let comments: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT id, document_id, author_name, body, status, created_at_ms FROM comments",
        )
        .fetch_all(&self.pool)
        .await?;

        let summaries = self.list_all_documents().await?;
        let mut documents = Vec::new();
        for summary in summaries {
            if let Some(full) = self.get_document(summary.id).await? {
                let doc = &full.document;
                let tags = self.document_tags(doc.id).await?;
                let blocks: Vec<serde_json::Value> = doc
                    .blocks
                    .iter()
                    .map(|b| {
                        let content = doc
                            .versions
                            .iter()
                            .find(|v| v.id == b.version_id)
                            .map(|v| v.content.clone())
                            .unwrap_or(BlockContent::Null);
                        json!({
                            "id": b.id,
                            "kind": b.kind,
                            "position": b.position,
                            "content": content,
                        })
                    })
                    .collect();
                documents.push(json!({
                    "id": doc.id,
                    "title": doc.title,
                    "slug": summary.slug,
                    "status": summary.status,
                    "published_at_ms": summary.published_at_ms,
                    "tags": tags,
                    "blocks": blocks,
                }));
            }
        }

        let experiments: Vec<(
            String,
            String,
            String,
            String,
            String,
            String,
            f64,
            f64,
            i64,
            f64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT id, document_id, block_id, name, status, goal, traffic_weight,
                    confidence_threshold, min_sample_per_variant, no_winner_prob,
                    max_duration_ms, started_at_ms, decided_at_ms, decision, winning_variant_id
             FROM experiments",
        )
        .fetch_all(&self.pool)
        .await?;
        let experiment_variants: Vec<(String, String, String, String, f64, i64)> = sqlx::query_as(
            "SELECT id, experiment_id, block_id, version_id, weight, is_control
             FROM experiment_variants",
        )
        .fetch_all(&self.pool)
        .await?;
        let experiment_decisions: Vec<(String, String, i64, String, Option<String>, Option<String>, Option<f64>, Option<f64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT id, experiment_id, decided_at_ms, decision, winner_variant_id, promoted_version_id,
                    effect_size, confidence, control_impressions, control_conversions,
                    variant_impressions, variant_conversions
             FROM experiment_decisions",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(json!({
            "version": 1,
            "exported_at_ms": now_ms(),
            "settings": settings,
            "users": users,
            "documents": documents,
            "comments": comments,
            "experiments": experiments,
            "experiment_variants": experiment_variants,
            "experiment_decisions": experiment_decisions,
        }))
    }

    async fn record_analytics_event(&self, event: &AnalyticsEvent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO analytics_events
                (id, document_id, event_type, band, block_id, pageview_id, visitor_id,
                 referrer, user_agent, read_time_ms, experiment_id, variant_id, created_at_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.id.to_string())
        .bind(event.document_id.to_string())
        .bind(&event.event_type)
        .bind(event.band)
        .bind(event.block_id.map(|b| b.to_string()))
        .bind(event.pageview_id.to_string())
        .bind(event.visitor_id.to_string())
        .bind(&event.referrer)
        .bind(&event.user_agent)
        .bind(event.read_time_ms)
        .bind(event.experiment_id.map(|e| e.to_string()))
        .bind(event.variant_id.map(|v| v.to_string()))
        .bind(event.created_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn article_stats(
        &self,
        document_id: DocumentId,
    ) -> Result<ArticleStats, RepositoryError> {
        let row = sqlx::query(
            "SELECT
                (SELECT COUNT(DISTINCT pageview_id) FROM analytics_events
                    WHERE document_id = ? AND event_type = 'view') AS views,
                (SELECT COUNT(DISTINCT visitor_id) FROM analytics_events
                    WHERE document_id = ? AND event_type = 'view') AS readers,
                (SELECT COUNT(*) FROM analytics_events
                    WHERE document_id = ? AND event_type = 'article_read') AS reads,
                (SELECT AVG(read_time_ms) FROM analytics_events
                    WHERE document_id = ? AND event_type = 'article_read'
                      AND read_time_ms IS NOT NULL) AS avg_read",
        )
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        let views: i64 = row.get("views");
        let unique_readers: i64 = row.get("readers");
        let read_events: i64 = row.get("reads");
        let avg_read_time_ms: Option<f64> = row.get("avg_read");
        Ok(ArticleStats {
            views,
            unique_readers,
            avg_read_time_ms: avg_read_time_ms.map(|v| v.round() as i64),
            read_events,
            completion: None,
            band_reach: Vec::new(),
        })
    }

    async fn band_reach(&self, document_id: DocumentId) -> Result<Vec<BandReach>, RepositoryError> {
        // Cumulative distinct pageviews per band: for each threshold band B,
        // count pageviews whose deepest scroll reached at least B.
        let rows = sqlx::query(
            "WITH depth AS (
                SELECT pageview_id, MAX(band) AS max_band
                FROM analytics_events
                WHERE document_id = ? AND event_type = 'banded_scroll'
                GROUP BY pageview_id
             )
             SELECT b.band AS band, COUNT(d.pageview_id) AS pvs
             FROM (SELECT 25 AS band UNION ALL SELECT 50 UNION ALL SELECT 75 UNION ALL SELECT 100) b
             LEFT JOIN depth d ON b.band <= d.max_band
             GROUP BY b.band
             ORDER BY b.band ASC",
        )
        .bind(document_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| BandReach {
                band: r.get("band"),
                pageviews: r.get("pvs"),
            })
            .collect())
    }

    async fn block_impressions(
        &self,
        document_id: DocumentId,
    ) -> Result<std::collections::HashMap<Uuid, i64>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT block_id, COUNT(DISTINCT pageview_id) AS pvs
             FROM analytics_events
             WHERE document_id = ? AND event_type = 'block_impression' AND block_id IS NOT NULL
             GROUP BY block_id",
        )
        .bind(document_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let block_id: String = r.get("block_id");
                Uuid::from_str(&block_id)
                    .ok()
                    .map(|id| (id, r.get::<i64, _>("pvs")))
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // Experiments (M3)
    // -----------------------------------------------------------------------

    async fn create_experiment(
        &self,
        document_id: DocumentId,
        block_id: BlockId,
        new: &crate::model::NewExperiment,
    ) -> Result<crate::model::ExperimentRecord, RepositoryError> {
        let mut tx = self.pool.begin().await?;

        let control_row =
            sqlx::query("SELECT current_version_id FROM blocks WHERE id = ? AND document_id = ?")
                .bind(block_id.to_string())
                .bind(document_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        let Some(control_row) = control_row else {
            return Err(RepositoryError::NotFound(
                "block not found for experiment".into(),
            ));
        };
        let control_version_id =
            Uuid::from_str(&control_row.get::<String, _>("current_version_id"))
                .map_err(|_| RepositoryError::InvalidInput("bad control version".into()))?;

        let id = openpublish_experiments::ExperimentId::new_v4();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO experiments
                (id, document_id, block_id, name, status, control_version_id, goal,
                 traffic_weight, confidence_threshold, min_sample_per_variant,
                 no_winner_prob, max_duration_ms, created_at_ms)
             VALUES (?, ?, ?, ?, 'draft', ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(document_id.to_string())
        .bind(block_id.to_string())
        .bind(&new.name)
        .bind(control_version_id.to_string())
        .bind(&new.goal)
        .bind(new.traffic_weight)
        .bind(new.confidence_threshold)
        .bind(new.min_sample_per_variant as i64)
        .bind(new.no_winner_prob)
        .bind(new.max_duration_ms)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        // Control variant row points at the immutable control version.
        let control_variant_id = VariantId::new_v4();
        sqlx::query(
            "INSERT INTO experiment_variants (id, experiment_id, block_id, version_id, weight, is_control)
             VALUES (?, ?, ?, ?, ?, 1)",
        )
        .bind(control_variant_id.to_string())
        .bind(id.to_string())
        .bind(block_id.to_string())
        .bind(control_version_id.to_string())
        .bind(new.traffic_weight)
        .execute(&mut *tx)
        .await?;

        // Each non-control variant writes a NEW immutable version to the shared
        // pool. The block's canonical current_version_id is left untouched.
        let mut variant_rows = Vec::new();
        for input in &new.variants {
            let variant_id = VariantId::new_v4();
            let version_id = VersionId::new_v4();
            sqlx::query(
                "INSERT INTO block_versions (id, block_id, content_json, created_at_ms)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(version_id.to_string())
            .bind(block_id.to_string())
            .bind(input.content.to_string())
            .bind(now)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO experiment_variants (id, experiment_id, block_id, version_id, weight, is_control)
                 VALUES (?, ?, ?, ?, ?, 0)",
            )
            .bind(variant_id.to_string())
            .bind(id.to_string())
            .bind(block_id.to_string())
            .bind(version_id.to_string())
            .bind(input.weight)
            .execute(&mut *tx)
            .await?;
            variant_rows.push(crate::model::ExperimentVariantRecord {
                id: variant_id,
                block_id,
                version_id,
                weight: input.weight,
                is_control: false,
            });
        }

        tx.commit().await?;

        let mut variants_all = vec![crate::model::ExperimentVariantRecord {
            id: control_variant_id,
            block_id,
            version_id: control_version_id,
            weight: new.traffic_weight,
            is_control: true,
        }];
        variants_all.extend(variant_rows);
        Ok(crate::model::ExperimentRecord {
            id,
            document_id,
            block_id,
            name: new.name.clone(),
            status: "draft".into(),
            control_version_id,
            goal: new.goal.clone(),
            traffic_weight: new.traffic_weight,
            confidence_threshold: new.confidence_threshold,
            min_sample_per_variant: new.min_sample_per_variant as i64,
            no_winner_prob: new.no_winner_prob,
            max_duration_ms: new.max_duration_ms,
            started_at_ms: None,
            decided_at_ms: None,
            decision: None,
            winning_variant_id: None,
            created_at_ms: now,
            variants: variants_all,
        })
    }

    async fn experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<Option<crate::model::ExperimentRecord>, RepositoryError> {
        load_experiment(&self.pool, &id).await
    }

    async fn experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<crate::model::ExperimentRecord>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id FROM experiments WHERE document_id = ? ORDER BY created_at_ms DESC",
        )
        .bind(document_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            if let Some(exp) = load_experiment(&self.pool, &Uuid::parse_str(&id)?).await? {
                out.push(exp);
            }
        }
        Ok(out)
    }

    async fn running_experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<crate::model::ExperimentRecord>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id FROM experiments
             WHERE document_id = ? AND status = 'running' ORDER BY created_at_ms ASC",
        )
        .bind(document_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            if let Some(exp) = load_experiment(&self.pool, &Uuid::parse_str(&id)?).await? {
                out.push(exp);
            }
        }
        Ok(out)
    }

    async fn running_experiments(
        &self,
    ) -> Result<Vec<crate::model::ExperimentRecord>, RepositoryError> {
        let rows = sqlx::query("SELECT id FROM experiments WHERE status = 'running'")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            if let Some(exp) = load_experiment(&self.pool, &Uuid::parse_str(&id)?).await? {
                out.push(exp);
            }
        }
        Ok(out)
    }

    async fn start_experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<(), RepositoryError> {
        let now = now_ms();
        let result = sqlx::query(
            "UPDATE experiments SET status = 'running', started_at_ms = ?, decided_at_ms = NULL, decision = NULL, winning_variant_id = NULL
             WHERE id = ? AND status = 'draft'",
        )
        .bind(now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::Conflict(
                "only draft experiments can start".into(),
            ));
        }
        Ok(())
    }

    async fn stop_experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "UPDATE experiments SET status = 'stopped', decided_at_ms = ?, decision = 'stopped' WHERE id = ?",
        )
        .bind(now_ms())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(RepositoryError::NotFound("experiment".into()));
        }
        Ok(())
    }

    async fn promote_block_version(
        &self,
        block_id: BlockId,
        version_id: VersionId,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE blocks SET current_version_id = ?, updated_at_ms = ?
             WHERE id = ?",
        )
        .bind(version_id.to_string())
        .bind(now_ms())
        .bind(block_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn conclude_experiment(
        &self,
        id: openpublish_experiments::ExperimentId,
        decision: &str,
        winning_variant_id: Option<openpublish_experiments::VariantId>,
        promoted_version_id: Option<VersionId>,
        stats: &crate::model::ExperimentDecision,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO experiment_decisions
                (id, experiment_id, decided_at_ms, decision, winner_variant_id, promoted_version_id,
                 effect_size, confidence, control_impressions, control_conversions,
                 variant_impressions, variant_conversions)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(stats.id.to_string())
        .bind(id.to_string())
        .bind(stats.decided_at_ms)
        .bind(decision)
        .bind(stats.winner_variant_id.map(|v| v.to_string()))
        .bind(stats.promoted_version_id.map(|v| v.to_string()))
        .bind(stats.effect_size)
        .bind(stats.confidence)
        .bind(stats.control_impressions)
        .bind(stats.control_conversions)
        .bind(stats.variant_impressions)
        .bind(stats.variant_conversions)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE experiments SET status = 'decided', decided_at_ms = ?, decision = ?, winning_variant_id = ?
             WHERE id = ? AND status = 'running'",
        )
        .bind(stats.decided_at_ms)
        .bind(decision)
        .bind(winning_variant_id.map(|v| v.to_string()))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        if let Some(pid) = promoted_version_id {
            let exp = load_experiment_tx(&mut tx, &id)
                .await?
                .ok_or_else(|| RepositoryError::NotFound("experiment".into()))?;
            sqlx::query("UPDATE blocks SET current_version_id = ?, updated_at_ms = ? WHERE id = ?")
                .bind(pid.to_string())
                .bind(now_ms())
                .bind(exp.block_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn experiment_counts(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<Vec<crate::model::ExperimentCounts>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT variant_id,
                    COUNT(DISTINCT CASE WHEN event_type = 'experiment_impression' THEN visitor_id END) AS impressions,
                    COUNT(DISTINCT CASE WHEN event_type = 'experiment_conversion' THEN visitor_id END) AS conversions
             FROM analytics_events
             WHERE experiment_id = ?
             GROUP BY variant_id",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| crate::model::ExperimentCounts {
                variant_id: Uuid::from_str(&r.get::<String, _>("variant_id")).unwrap_or_default(),
                impressions: r.get("impressions"),
                conversions: r.get("conversions"),
            })
            .collect())
    }

    async fn experiment_decisions(
        &self,
        id: openpublish_experiments::ExperimentId,
    ) -> Result<Vec<crate::model::ExperimentDecision>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, experiment_id, decided_at_ms, decision, winner_variant_id, promoted_version_id,
                    effect_size, confidence, control_impressions, control_conversions,
                    variant_impressions, variant_conversions
             FROM experiment_decisions WHERE experiment_id = ? ORDER BY decided_at_ms ASC",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| crate::model::ExperimentDecision {
                id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
                experiment_id: id,
                decided_at_ms: r.get("decided_at_ms"),
                decision: r.get("decision"),
                winner_variant_id: r
                    .get::<Option<String>, _>("winner_variant_id")
                    .and_then(|s| Uuid::parse_str(&s).ok()),
                promoted_version_id: r
                    .get::<Option<String>, _>("promoted_version_id")
                    .and_then(|s| Uuid::parse_str(&s).ok()),
                effect_size: r.get("effect_size"),
                confidence: r.get("confidence"),
                control_impressions: r.get("control_impressions"),
                control_conversions: r.get("control_conversions"),
                variant_impressions: r.get("variant_impressions"),
                variant_conversions: r.get("variant_conversions"),
            })
            .collect())
    }

    async fn experiment_variant_belongs(
        &self,
        id: openpublish_experiments::ExperimentId,
        variant_id: openpublish_experiments::VariantId,
    ) -> Result<bool, RepositoryError> {
        let row = sqlx::query(
            "SELECT 1 AS one FROM experiment_variants WHERE experiment_id = ? AND id = ? LIMIT 1",
        )
        .bind(id.to_string())
        .bind(variant_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }
}

/// Load an experiment with its variants (read path: fresh snapshot).
async fn load_experiment(
    pool: &SqlitePool,
    id: &ExperimentId,
) -> Result<Option<crate::model::ExperimentRecord>, RepositoryError> {
    let mut tx = pool.begin().await?;
    load_experiment_tx(&mut tx, id).await
}

/// Load an experiment with its variants inside a live transaction (promotion
/// needs a consistent snapshot).
async fn load_experiment_tx(
    tx: &mut Transaction<'_, sqlx::sqlite::Sqlite>,
    id: &ExperimentId,
) -> Result<Option<crate::model::ExperimentRecord>, RepositoryError> {
    let row = sqlx::query(
        "SELECT id, document_id, block_id, name, status, control_version_id, goal,
                traffic_weight, confidence_threshold, min_sample_per_variant,
                no_winner_prob, max_duration_ms, started_at_ms, decided_at_ms,
                decision, winning_variant_id, created_at_ms
         FROM experiments WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };

    let variant_rows = sqlx::query(
        "SELECT id, block_id, version_id, weight, is_control
         FROM experiment_variants WHERE experiment_id = ?",
    )
    .bind(id.to_string())
    .fetch_all(&mut **tx)
    .await?;

    let variants = variant_rows
        .iter()
        .map(|r| crate::model::ExperimentVariantRecord {
            id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
            block_id: Uuid::from_str(&r.get::<String, _>("block_id")).unwrap_or_default(),
            version_id: Uuid::from_str(&r.get::<String, _>("version_id")).unwrap_or_default(),
            weight: r.get("weight"),
            is_control: r.get::<i64, _>("is_control") != 0,
        })
        .collect();

    Ok(Some(crate::model::ExperimentRecord {
        id: *id,
        document_id: Uuid::from_str(&row.get::<String, _>("document_id")).unwrap_or_default(),
        block_id: Uuid::from_str(&row.get::<String, _>("block_id")).unwrap_or_default(),
        name: row.get("name"),
        status: row.get("status"),
        control_version_id: Uuid::from_str(&row.get::<String, _>("control_version_id"))
            .unwrap_or_default(),
        goal: row.get("goal"),
        traffic_weight: row.get("traffic_weight"),
        confidence_threshold: row.get("confidence_threshold"),
        min_sample_per_variant: row.get("min_sample_per_variant"),
        no_winner_prob: row.get("no_winner_prob"),
        max_duration_ms: row.get("max_duration_ms"),
        started_at_ms: row.get("started_at_ms"),
        decided_at_ms: row.get("decided_at_ms"),
        decision: row.get("decision"),
        winning_variant_id: row
            .get::<Option<String>, _>("winning_variant_id")
            .and_then(|s| Uuid::parse_str(&s).ok()),
        created_at_ms: row.get("created_at_ms"),
        variants,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> SqliteRepository {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory pool");
        let repo = SqliteRepository::from_pool(pool);
        repo.migrate().await.expect("migrations apply");
        repo
    }

    async fn seed_user(repo: &SqliteRepository) -> User {
        repo.create_first_user("a@b.com", "Alice", "hash")
            .await
            .expect("first user")
    }

    #[tokio::test]
    async fn first_user_marks_setup_complete() {
        let repo = repo().await;
        assert!(!repo.is_setup_complete().await.unwrap());
        seed_user(&repo).await;
        assert!(repo.is_setup_complete().await.unwrap());
    }

    #[tokio::test]
    async fn session_roundtrip_and_expiry_check() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let session = repo.create_session(user.id).await.unwrap();
        let found = repo
            .session_by_token(&session.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.csrf, session.csrf);
        assert_eq!(found.user_id, user.id);
        repo.delete_session(&session.token).await.unwrap();
        assert!(
            repo.session_by_token(&session.token)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn create_save_publish_and_read_document() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Hello World").await.unwrap();
        let doc_id = full.document.id;
        assert_eq!(full.document.blocks.len(), 0);
        assert_eq!(full.slug, "hello-world");

        let parsed = openpublish_content::parse_markdown("# Hello\n\nbody text");
        let merged = openpublish_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        repo.publish_document(doc_id).await.unwrap();

        let loaded = repo.get_document(doc_id).await.unwrap().unwrap();
        assert_eq!(loaded.document.blocks.len(), 2);
        assert_eq!(loaded.status, "published");

        let published = repo
            .get_published_by_slug("hello-world")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(published.document.id, doc_id);
        assert_eq!(
            published
                .document
                .current_content(published.document.blocks[0].id),
            Some(&json!({ "text": "Hello" }))
        );
    }

    #[tokio::test]
    async fn slug_uniqueness_per_owner() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let d1 = repo.create_document(user.id, "Same Title").await.unwrap();
        let d2 = repo.create_document(user.id, "Same Title").await.unwrap();
        assert_ne!(d1.slug, d2.slug);
        let s1 = repo.list_documents(user.id).await.unwrap();
        assert_eq!(s1.len(), 2);
        assert_ne!(s1[0].slug, s1[1].slug);
    }

    #[tokio::test]
    async fn tags_and_comments() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Tagged").await.unwrap();
        let doc_id = full.document.id;
        repo.set_document_tags(doc_id, &["news".into(), "tech".into()])
            .await
            .unwrap();
        let tags = repo.document_tags(doc_id).await.unwrap();
        assert_eq!(tags, vec!["news", "tech"]);

        let c = repo
            .create_comment(doc_id, "Reader", "Nice post")
            .await
            .unwrap();
        assert_eq!(c.status, "pending");
        let pending = repo
            .comments_for_document(doc_id, Some("pending"))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        repo.set_comment_status(c.id, "approved").await.unwrap();
        let approved = repo
            .comments_for_document(doc_id, Some("approved"))
            .await
            .unwrap();
        assert_eq!(approved.len(), 1);
    }

    #[tokio::test]
    async fn export_dumps_documents() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Exported").await.unwrap();
        let doc_id = full.document.id;
        let parsed = openpublish_content::parse_markdown("Some text");
        let merged = openpublish_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        repo.publish_document(doc_id).await.unwrap();

        let dump = repo.export_json().await.unwrap();
        assert_eq!(dump["documents"].as_array().unwrap().len(), 1);
        assert_eq!(dump["documents"][0]["title"], "Exported");
    }

    #[tokio::test]
    async fn export_includes_unpublished_drafts() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Draft only").await.unwrap();
        let doc_id = full.document.id;
        let parsed = openpublish_content::parse_markdown("unpublished body");
        let merged = openpublish_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();

        let dump = repo.export_json().await.unwrap();
        let docs = dump["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 1, "drafts must be part of backups");
        assert_eq!(docs[0]["title"], "Draft only");
        assert_eq!(docs[0]["status"], "draft");
    }
}
