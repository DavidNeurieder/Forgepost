//! SQLite-backed implementation of the [`forgepost_application::ports`]
//! repository traits (solo mode, the only MVP distribution).
//!
//! The port traits are implemented here; migrations for the schema live in the
//! workspace `migrations/` directory. The domain and application layers never
//! touch SQL (§5, §5.4).

pub use forgepost_application::ports::*;

use async_trait::async_trait;
use forgepost_analytics::{ArticleStats, BandReach};
use forgepost_content::{
    Block, BlockContent, BlockId, BlockKind, BlockVersion, Document, DocumentId, VersionId,
    html_escape, now_ms,
};
use forgepost_domain::model::{
    AnalyticsEvent, Comment, DashboardMetric, DocumentSummary, FullDocument, Media, PostId,
    Session, SiteSettings, User,
};
use forgepost_domain::security::{SESSION_TTL_MS, sha256_hex};
use forgepost_experiments::{ExperimentId, VariantId};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

/// One week in milliseconds; the dashboard's "last 7 days" window.
const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

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
        id: PostId(Uuid::from_str(&row.get::<String, _>("id")).unwrap_or_default()),
        title: row.get("title"),
        slug: row.get("slug"),
        status: row.get("status"),
        published_at_ms: row.get("published_at_ms"),
        created_at_ms: row.get("created_at_ms"),
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
impl Repository for SqliteRepository {}

#[async_trait]
impl UserRepo for SqliteRepository {
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
}

#[async_trait]
impl SessionRepo for SqliteRepository {
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
}

#[async_trait]
impl SettingsRepo for SqliteRepository {
    async fn get_setting(&self, key: &str) -> Result<Option<String>, RepositoryError> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<String, _>("value")))
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, strftime('%s','now') * 1000)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                             updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn site_settings(&self) -> Result<SiteSettings, RepositoryError> {
        let name = self
            .get_setting("site.name")
            .await?
            .unwrap_or_else(|| "Forgepost".into());
        let theme = self
            .get_setting("theme")
            .await?
            .unwrap_or_else(|| "system".into());
        let url = self.get_setting("site.url").await?.unwrap_or_default();
        let tagline = self.get_setting("site.tagline").await?.unwrap_or_default();
        let image = self.get_setting("site.image").await?.unwrap_or_default();
        let comments_enabled = self
            .get_setting("comments.enabled")
            .await?
            .map(|v| v == "1")
            .unwrap_or(false);
        Ok(SiteSettings {
            name,
            theme,
            url,
            tagline,
            image,
            comments_enabled,
        })
    }
}

#[async_trait]
impl DocumentRepo for SqliteRepository {
    async fn list_documents(
        &self,
        owner_id: Uuid,
    ) -> Result<Vec<DocumentSummary>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, slug, status, published_at_ms, created_at_ms, updated_at_ms
             FROM documents WHERE owner_id = ? AND deleted_at_ms IS NULL
             ORDER BY created_at_ms DESC, id DESC",
        )
        .bind(owner_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_document_summary).collect())
    }

    async fn list_published(&self) -> Result<Vec<DocumentSummary>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, slug, status, published_at_ms, created_at_ms, updated_at_ms
             FROM documents WHERE status = 'published' AND deleted_at_ms IS NULL
             ORDER BY published_at_ms DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_document_summary).collect())
    }

    async fn list_published_with_tags(
        &self,
    ) -> Result<Vec<forgepost_domain::model::PublishedPost>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT d.id, d.title, d.slug, d.published_at_ms,
                    (SELECT json_group_array(t.slug) FROM tags t
                       JOIN document_tags dt ON dt.tag_id = t.id
                      WHERE dt.document_id = d.id) AS tags
             FROM documents d
             WHERE d.status = 'published' AND d.deleted_at_ms IS NULL
             ORDER BY d.published_at_ms DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| forgepost_domain::model::PublishedPost {
                id: PostId(Uuid::from_str(&row.get::<String, _>("id")).unwrap_or_default()),
                title: row.get("title"),
                slug: row.get("slug"),
                published_at_ms: row.get("published_at_ms"),
                tags: row
                    .get::<Option<String>, _>("tags")
                    .map(|s| serde_json::from_str(&s).unwrap_or_default())
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn list_published_with_tag(
        &self,
        tag: &str,
    ) -> Result<Vec<forgepost_domain::model::PublishedPost>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT d.id, d.title, d.slug, d.published_at_ms,
                    (SELECT json_group_array(t2.slug) FROM tags t2
                       JOIN document_tags dt2 ON dt2.tag_id = t2.id
                      WHERE dt2.document_id = d.id) AS tags
             FROM documents d
             JOIN document_tags dt ON dt.document_id = d.id
             JOIN tags t ON t.id = dt.tag_id
             WHERE d.status = 'published' AND d.deleted_at_ms IS NULL AND t.slug = ?
             ORDER BY d.published_at_ms DESC",
        )
        .bind(tag)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| forgepost_domain::model::PublishedPost {
                id: PostId(Uuid::from_str(&row.get::<String, _>("id")).unwrap_or_default()),
                title: row.get("title"),
                slug: row.get("slug"),
                published_at_ms: row.get("published_at_ms"),
                tags: row
                    .get::<Option<String>, _>("tags")
                    .map(|s| serde_json::from_str(&s).unwrap_or_default())
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn list_all_documents(&self) -> Result<Vec<DocumentSummary>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, title, slug, status, published_at_ms, created_at_ms, updated_at_ms
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
             FROM blocks WHERE document_id = ? AND position >= 0 ORDER BY position",
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
        self.refresh_search_index(id).await?;
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
        // Move all current rows out of the live 0..n position space first, so
        // renumbering (e.g. inserting at position 0) cannot collide transiently
        // while other rows still hold the old positions.
        //
        // Rows are parked in two steps so the bands stay disjoint:
        //   1. already-parked (dropped) rows from previous saves are pushed one
        //      generation deeper, to `orig - (k+1)*1e9` (<= -2e9);
        //   2. live rows are moved into the freshly-freed first generation band
        //      `pos - 1e9` (in [-1e9, -1e9+n)).
        // A single combined UPDATE could transiently violate UNIQUE
        // (document_id, position): a live row updated *before* a parked row that
        // already sits at that exact destination (e.g. position 20 parks to
        // -999999980 while a previously-dropped block still holds -999999980),
        // because SQLite checks the constraint per row as the statement runs.
        sqlx::query("UPDATE blocks SET position = position - 1000000000 WHERE document_id = ? AND position < 0")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE blocks SET position = position - 1000000000 WHERE document_id = ? AND position >= 0")
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
        self.refresh_search_index(id).await?;
        Ok(())
    }

    async fn publish_document(&self, id: DocumentId) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE documents SET status = 'published', published_at_ms = ?, updated_at_ms = ? WHERE id = ?")
            .bind(now_ms())
            .bind(now_ms())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        self.refresh_search_index(id).await?;
        Ok(())
    }

    async fn delete_document(&self, id: DocumentId) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        // Clear the FTS5 rows first: `search_rows` (needed for the lookup)
        // cascades away once the document row is gone, leaving garbage in the
        // virtual table.
        remove_search_row(&mut tx, id).await?;
        let res = sqlx::query("DELETE FROM documents WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        if res.rows_affected() == 0 {
            return Err(RepositoryError::NotFound(id.to_string()));
        }
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
        self.refresh_search_index(id).await?;
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
}

#[async_trait]
impl CommentRepo for SqliteRepository {
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
            document_id: PostId::from(document_id),
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
                document_id: PostId(
                    Uuid::from_str(&r.get::<String, _>("document_id")).unwrap_or_default(),
                ),
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
                document_id: PostId(
                    Uuid::from_str(&r.get::<String, _>("document_id")).unwrap_or_default(),
                ),
                author_name: r.get("author_name"),
                body: r.get("body"),
                status: r.get("status"),
                created_at_ms: r.get("created_at_ms"),
            })
            .collect())
    }
}

#[async_trait]
impl ExportRepo for SqliteRepository {
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
            if let Some(full) = self.get_document(summary.id.0).await? {
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
}

#[async_trait]
impl AnalyticsRepo for SqliteRepository {
    async fn record_analytics_event(&self, event: &AnalyticsEvent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO analytics_events
                (id, document_id, event_type, band, block_id, pageview_id, visitor_id,
                 referrer, user_agent, read_time_ms, experiment_id, variant_id,
                 recommended_slug, created_at_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&event.recommended_slug)
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
                (SELECT COUNT(DISTINCT pageview_id) FROM analytics_events
                    WHERE document_id = ? AND event_type = 'share_click') AS shares,
                (SELECT AVG(read_time_ms) FROM analytics_events
                    WHERE document_id = ? AND event_type = 'article_read'
                      AND read_time_ms IS NOT NULL) AS avg_read",
        )
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .bind(document_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        let views: i64 = row.get("views");
        let unique_readers: i64 = row.get("readers");
        let read_events: i64 = row.get("reads");
        let shares: i64 = row.get("shares");
        let avg_read_time_ms: Option<f64> = row.get("avg_read");
        Ok(ArticleStats {
            views,
            unique_readers,
            avg_read_time_ms: avg_read_time_ms.map(|v| v.round() as i64),
            read_events,
            shares,
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

    async fn referrer_counts(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<(Option<String>, i64)>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT referrer, COUNT(DISTINCT pageview_id) AS pvs
             FROM analytics_events
             WHERE document_id = ? AND event_type = 'view'
             GROUP BY referrer",
        )
        .bind(document_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let referrer: Option<String> = r.get("referrer");
                (referrer, r.get::<i64, _>("pvs"))
            })
            .collect())
    }

    async fn dashboard_metrics(
        &self,
        now_ms: i64,
    ) -> Result<Vec<DashboardMetric>, RepositoryError> {
        // Views in the current and previous 7-day windows, per document.
        let views_rows = sqlx::query(
            "SELECT document_id,
                    COUNT(DISTINCT CASE WHEN created_at_ms >= ? THEN pageview_id END) AS cur,
                    COUNT(DISTINCT CASE WHEN created_at_ms < ? AND created_at_ms >= ? THEN pageview_id END) AS prev
             FROM analytics_events
             WHERE event_type = 'view' AND created_at_ms >= ?
             GROUP BY document_id",
        )
        .bind(now_ms - SEVEN_DAYS_MS)
        .bind(now_ms - SEVEN_DAYS_MS)
        .bind(now_ms - SEVEN_DAYS_MS - SEVEN_DAYS_MS)
        .bind(now_ms - SEVEN_DAYS_MS - SEVEN_DAYS_MS)
        .fetch_all(&self.pool)
        .await?;

        // Lifetime views and completed pageviews (reached band 100), per document.
        let total_rows = sqlx::query(
            "SELECT document_id,
                    COUNT(DISTINCT pageview_id) AS views,
                    (SELECT COUNT(DISTINCT c.pageview_id) FROM analytics_events c
                     WHERE c.document_id = e.document_id
                       AND c.event_type = 'banded_scroll' AND c.band = 100) AS completed
             FROM analytics_events e
             WHERE e.event_type = 'view'
             GROUP BY e.document_id",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut totals: std::collections::HashMap<Uuid, (i64, i64)> = total_rows
            .iter()
            .filter_map(|r| {
                let id: String = r.get("document_id");
                Uuid::from_str(&id)
                    .ok()
                    .map(|id| (id, (r.get::<i64, _>("views"), r.get::<i64, _>("completed"))))
            })
            .collect();

        Ok(views_rows
            .iter()
            .filter_map(|r| {
                let id: String = r.get("document_id");
                let id = Uuid::from_str(&id).ok()?;
                let (views_total, completed) = totals.remove(&id).unwrap_or((0, 0));
                Some(DashboardMetric {
                    document_id: PostId::from(id),
                    views_7d: r.get("cur"),
                    views_prev_7d: r.get("prev"),
                    views_total,
                    completed,
                })
            })
            .collect())
    }
}

#[async_trait]
impl ExperimentRepo for SqliteRepository {
    async fn create_experiment(
        &self,
        document_id: DocumentId,
        block_id: BlockId,
        new: &forgepost_domain::model::NewExperiment,
    ) -> Result<forgepost_domain::model::ExperimentRecord, RepositoryError> {
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

        let id = forgepost_experiments::ExperimentId::new_v4();
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
            variant_rows.push(forgepost_domain::model::ExperimentVariantRecord {
                id: variant_id,
                block_id,
                version_id,
                weight: input.weight,
                is_control: false,
            });
        }

        tx.commit().await?;

        let mut variants_all = vec![forgepost_domain::model::ExperimentVariantRecord {
            id: control_variant_id,
            block_id,
            version_id: control_version_id,
            weight: new.traffic_weight,
            is_control: true,
        }];
        variants_all.extend(variant_rows);
        Ok(forgepost_domain::model::ExperimentRecord {
            id,
            document_id: PostId::from(document_id),
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
        id: forgepost_experiments::ExperimentId,
    ) -> Result<Option<forgepost_domain::model::ExperimentRecord>, RepositoryError> {
        load_experiment(&self.pool, &id).await
    }

    async fn experiments_for_document(
        &self,
        document_id: DocumentId,
    ) -> Result<Vec<forgepost_domain::model::ExperimentRecord>, RepositoryError> {
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
    ) -> Result<Vec<forgepost_domain::model::ExperimentRecord>, RepositoryError> {
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
    ) -> Result<Vec<forgepost_domain::model::ExperimentRecord>, RepositoryError> {
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
        id: forgepost_experiments::ExperimentId,
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
        id: forgepost_experiments::ExperimentId,
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
        // Promotion changes the canonical content, so the search index follows.
        let row = sqlx::query("SELECT document_id FROM blocks WHERE id = ?")
            .bind(block_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = row {
            let document_id: String = row.get("document_id");
            if let Ok(id) = Uuid::from_str(&document_id) {
                self.refresh_search_index(id).await?;
            }
        }
        Ok(())
    }

    async fn conclude_experiment(
        &self,
        id: forgepost_experiments::ExperimentId,
        decision: &str,
        winning_variant_id: Option<forgepost_experiments::VariantId>,
        promoted_version_id: Option<VersionId>,
        stats: &forgepost_domain::model::ExperimentDecision,
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
        id: forgepost_experiments::ExperimentId,
    ) -> Result<Vec<forgepost_domain::model::ExperimentCounts>, RepositoryError> {
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
            .map(|r| forgepost_domain::model::ExperimentCounts {
                variant_id: Uuid::from_str(&r.get::<String, _>("variant_id")).unwrap_or_default(),
                impressions: r.get("impressions"),
                conversions: r.get("conversions"),
            })
            .collect())
    }

    async fn experiment_decisions(
        &self,
        id: forgepost_experiments::ExperimentId,
    ) -> Result<Vec<forgepost_domain::model::ExperimentDecision>, RepositoryError> {
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
            .map(|r| forgepost_domain::model::ExperimentDecision {
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
        id: forgepost_experiments::ExperimentId,
        variant_id: forgepost_experiments::VariantId,
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

#[async_trait]
impl SearchRepo for SqliteRepository {
    async fn search_documents(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<forgepost_domain::model::SearchHit>, RepositoryError> {
        let match_expr = fts_match_expr(query);
        if match_expr.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT d.id AS document_id, d.slug, d.title, d.published_at_ms,
                    (SELECT group_concat(t.slug, ',') FROM document_tags dt
                       JOIN tags t ON t.id = dt.tag_id
                      WHERE dt.document_id = d.id) AS tag_csv,
                    snippet(document_search, 2, '<mark>', '</mark>', '…', 30) AS snippet
             FROM document_search
             JOIN search_rows r ON r.fts_rowid = document_search.rowid
             JOIN documents d ON d.id = r.document_id
             WHERE document_search MATCH ?
               AND d.status = 'published'
               AND d.deleted_at_ms IS NULL
             ORDER BY bm25(document_search, 6.0, 1.0, 2.0)
             LIMIT ?",
        )
        .bind(match_expr)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| forgepost_domain::model::SearchHit {
                document_id: PostId(
                    Uuid::from_str(&r.get::<String, _>("document_id")).unwrap_or_default(),
                ),
                slug: r.get("slug"),
                title: r.get("title"),
                published_at_ms: r.get("published_at_ms"),
                tags: r
                    .get::<Option<String>, _>("tag_csv")
                    .map(|csv| csv.split(',').map(|s| s.to_string()).collect())
                    .unwrap_or_default(),
                snippet: r
                    .get::<Option<String>, _>("snippet")
                    .map(|s| escape_snippet(&s))
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn refresh_search_index(&self, document_id: DocumentId) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT title, status, deleted_at_ms FROM documents WHERE id = ?")
            .bind(document_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(());
        };
        let title: String = row.get("title");
        let status: String = row.get("status");
        let deleted_at_ms: Option<i64> = row.get("deleted_at_ms");
        remove_search_row(&mut tx, document_id).await?;
        if status != "published" || deleted_at_ms.is_some() {
            tx.commit().await?;
            return Ok(());
        }

        let block_rows = sqlx::query(
            "SELECT b.kind, v.content_json
             FROM blocks b JOIN block_versions v ON v.id = b.current_version_id
             WHERE b.document_id = ? AND b.position >= 0 ORDER BY b.position",
        )
        .bind(document_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let body = block_rows
            .iter()
            .filter_map(|r| {
                let kind: BlockKind = serde_json::from_str(&r.get::<String, _>("kind"))
                    .unwrap_or(BlockKind::Paragraph);
                let content: BlockContent =
                    serde_json::from_str(&r.get::<String, _>("content_json"))
                        .unwrap_or(serde_json::Value::Null);
                let text = forgepost_content::markdown::block_search_text(&kind, &content);
                (!text.trim().is_empty()).then_some(text)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let tag_rows = sqlx::query(
            "SELECT t.slug FROM tags t JOIN document_tags dt ON dt.tag_id = t.id
             WHERE dt.document_id = ? ORDER BY t.slug",
        )
        .bind(document_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let tags = tag_rows
            .iter()
            .map(|r| r.get::<String, _>("slug"))
            .collect::<Vec<_>>()
            .join(" ");

        sqlx::query(
            "INSERT INTO document_search (document_id, title, body, tags) VALUES (?, ?, ?, ?)",
        )
        .bind(document_id.to_string())
        .bind(title)
        .bind(body)
        .bind(tags)
        .execute(&mut *tx)
        .await?;
        let rid_row = sqlx::query("SELECT last_insert_rowid() AS rid")
            .fetch_one(&mut *tx)
            .await?;
        let rid: i64 = rid_row.get("rid");
        sqlx::query("INSERT OR REPLACE INTO search_rows (document_id, fts_rowid) VALUES (?, ?)")
            .bind(document_id.to_string())
            .bind(rid)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn rebuild_search_index_all(&self) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM search_rows")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM document_search")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let published = self.list_published().await?;
        for doc in published {
            self.refresh_search_index(doc.id.0).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl MediaRepo for SqliteRepository {
    async fn insert_media(&self, media: &Media) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO media (id, disk_name, content_type, size_bytes, sha256, created_at_ms)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(media.id.to_string())
        .bind(&media.disk_name)
        .bind(&media.content_type)
        .bind(media.size_bytes)
        .bind(&media.sha256)
        .bind(media.created_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn media_by_disk_name(&self, disk_name: &str) -> Result<Option<Media>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, disk_name, content_type, size_bytes, sha256, created_at_ms FROM media
             WHERE disk_name = ?",
        )
        .bind(disk_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Media {
            id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
            disk_name: r.get("disk_name"),
            content_type: r.get("content_type"),
            size_bytes: r.get("size_bytes"),
            sha256: r.get("sha256"),
            created_at_ms: r.get("created_at_ms"),
        }))
    }
}

/// Populate the search index once after migrations (existing databases get
/// indexed at startup). No-op when the index is already populated.
pub async fn backfill_search_index(repo: &SqliteRepository) -> Result<(), RepositoryError> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM search_rows")
        .fetch_one(repo.pool())
        .await?;
    let n: i64 = row.get("n");
    if n == 0 {
        repo.rebuild_search_index_all().await?;
    }
    Ok(())
}

/// Delete a document from the FTS5 index and its rowid mapping (idempotent).
async fn remove_search_row(
    conn: &mut sqlx::sqlite::SqliteConnection,
    document_id: DocumentId,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "DELETE FROM document_search WHERE rowid IN
            (SELECT fts_rowid FROM search_rows WHERE document_id = ?)",
    )
    .bind(document_id.to_string())
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM search_rows WHERE document_id = ?")
        .bind(document_id.to_string())
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Escape a raw FTS5 `snippet()` string for safe HTML output while keeping the
/// `<mark>…</mark>` highlight markers. The FTS index stores un-escaped body
/// text, so without this the snippet would render author content as markup.
fn escape_snippet(snippet: &str) -> String {
    const MARK_OPEN: &str = "<mark>";
    const MARK_CLOSE: &str = "</mark>";
    let mut out = String::with_capacity(snippet.len() + 32);
    let mut rest = snippet;
    loop {
        let Some(open) = rest.find(MARK_OPEN) else {
            out.push_str(&html_escape(rest));
            break;
        };
        out.push_str(&html_escape(&rest[..open]));
        out.push_str(MARK_OPEN);
        rest = &rest[open + MARK_OPEN.len()..];
        let Some(close) = rest.find(MARK_CLOSE) else {
            out.push_str(&html_escape(rest));
            break;
        };
        out.push_str(&html_escape(&rest[..close]));
        out.push_str(MARK_CLOSE);
        rest = &rest[close + MARK_CLOSE.len()..];
    }
    out
}

/// Build a safe FTS5 match expression from a plain query string. Tokens are
/// double-quoted (literal) and the last token gets a prefix star so typing
/// "hello wor" matches "hello world". Returns empty for a blank query.
fn fts_match_expr(query: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    for raw in query.split_whitespace() {
        let token: String = raw
            .chars()
            .filter(|c| !c.is_control() && *c != '"')
            .collect();
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        tokens.push(token.to_string());
    }
    if tokens.is_empty() {
        return String::new();
    }
    let last = tokens.len() - 1;
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push('"');
        out.push_str(token);
        out.push('"');
        if i == last {
            out.push('*');
        }
    }
    out
}

/// Load an experiment with its variants (read path: fresh snapshot).
async fn load_experiment(
    pool: &SqlitePool,
    id: &ExperimentId,
) -> Result<Option<forgepost_domain::model::ExperimentRecord>, RepositoryError> {
    let mut tx = pool.begin().await?;
    load_experiment_tx(&mut tx, id).await
}

/// Load an experiment with its variants inside a live transaction (promotion
/// needs a consistent snapshot).
async fn load_experiment_tx(
    tx: &mut Transaction<'_, sqlx::sqlite::Sqlite>,
    id: &ExperimentId,
) -> Result<Option<forgepost_domain::model::ExperimentRecord>, RepositoryError> {
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
        .map(|r| forgepost_domain::model::ExperimentVariantRecord {
            id: Uuid::from_str(&r.get::<String, _>("id")).unwrap_or_default(),
            block_id: Uuid::from_str(&r.get::<String, _>("block_id")).unwrap_or_default(),
            version_id: Uuid::from_str(&r.get::<String, _>("version_id")).unwrap_or_default(),
            weight: r.get("weight"),
            is_control: r.get::<i64, _>("is_control") != 0,
        })
        .collect();

    Ok(Some(forgepost_domain::model::ExperimentRecord {
        id: *id,
        document_id: PostId(
            Uuid::from_str(&row.get::<String, _>("document_id")).unwrap_or_default(),
        ),
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
    use forgepost_domain::model::VisitorId;

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
    async fn settings_roundtrip_and_defaults() {
        let repo = repo().await;

        // Fresh database: defaults apply.
        let site = repo.site_settings().await.unwrap();
        assert_eq!(site.name, "Forgepost");
        assert_eq!(site.theme, "system");
        assert_eq!(site.url, "");
        assert_eq!(site.tagline, "");
        assert_eq!(site.image, "", "no default image until configured");
        assert!(!site.comments_enabled, "comments must default to disabled");
        assert!(repo.get_setting("site.name").await.unwrap().is_none());

        // Roundtrip an explicit value.
        repo.set_setting("site.name", "My Blog").await.unwrap();
        repo.set_setting("theme", "dark").await.unwrap();
        repo.set_setting("site.url", "https://example.com")
            .await
            .unwrap();
        repo.set_setting("site.tagline", "Notes on things.")
            .await
            .unwrap();
        repo.set_setting("site.image", "https://example.com/og.png")
            .await
            .unwrap();
        assert_eq!(
            repo.get_setting("site.name").await.unwrap().unwrap(),
            "My Blog"
        );
        let site = repo.site_settings().await.unwrap();
        assert_eq!(site.name, "My Blog");
        assert_eq!(site.theme, "dark");
        assert_eq!(site.url, "https://example.com");
        assert_eq!(site.tagline, "Notes on things.");
        assert_eq!(site.image, "https://example.com/og.png");

        // Comments are opt-in: explicitly enabling flips the default.
        repo.set_setting("comments.enabled", "1").await.unwrap();
        assert!(repo.site_settings().await.unwrap().comments_enabled);
        repo.set_setting("comments.enabled", "0").await.unwrap();
        assert!(!repo.site_settings().await.unwrap().comments_enabled);

        // Overwrite and confirm only one row per key.
        repo.set_setting("theme", "sepia").await.unwrap();
        let site = repo.site_settings().await.unwrap();
        assert_eq!(site.theme, "sepia");
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM settings WHERE key IN ('site.name', 'theme')")
                .fetch_all(&repo.pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn media_roundtrip() {
        let repo = repo().await;
        let media = Media {
            id: Uuid::new_v4(),
            disk_name: "abc123.png".into(),
            content_type: "image/png".into(),
            size_bytes: 42,
            sha256: "deadbeef".into(),
            created_at_ms: now_ms(),
        };
        assert!(
            repo.media_by_disk_name(&media.disk_name)
                .await
                .unwrap()
                .is_none()
        );
        repo.insert_media(&media).await.unwrap();
        let got = repo
            .media_by_disk_name(&media.disk_name)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, media.id);
        assert_eq!(got.disk_name, "abc123.png");
        assert_eq!(got.content_type, "image/png");
        assert_eq!(got.size_bytes, 42);
        assert_eq!(got.sha256, "deadbeef");
        assert_eq!(got.created_at_ms, media.created_at_ms);
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

        let parsed = forgepost_content::parse_markdown("# Hello\n\nbody text");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
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
        let parsed = forgepost_content::parse_markdown("Some text");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
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
        let parsed = forgepost_content::parse_markdown("unpublished body");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();

        let dump = repo.export_json().await.unwrap();
        let docs = dump["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 1, "drafts must be part of backups");
        assert_eq!(docs[0]["title"], "Draft only");
        assert_eq!(docs[0]["status"], "draft");
    }

    #[tokio::test]
    async fn search_indexes_published_content_and_tracks_edits() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo
            .create_document(user.id, "Rust Async Notes")
            .await
            .unwrap();
        let doc_id = full.document.id;

        // Drafts are not searchable.
        let parsed = forgepost_content::parse_markdown("explains tokio channels");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        assert!(
            repo.search_documents("tokio", 10).await.unwrap().is_empty(),
            "drafts must not be indexed"
        );

        // Published content is searchable, with prefix (as-you-type) matching.
        repo.publish_document(doc_id).await.unwrap();
        let hits = repo.search_documents("tok", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "rust-async-notes");
        assert!(
            hits[0].snippet.contains("tokio"),
            "snippet should surface the matched text"
        );

        // Editing a published post re-indexes (deleted blocks drop out too).
        let parsed = forgepost_content::parse_markdown("now about actix actors");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        assert!(repo.search_documents("tokio", 10).await.unwrap().is_empty());
        assert_eq!(repo.search_documents("actix", 10).await.unwrap().len(), 1);

        // Title edits are reflected.
        repo.update_document_title(doc_id, "Async in Rust")
            .await
            .unwrap();
        let hits = repo.search_documents("async", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Async in Rust");
    }

    #[tokio::test]
    async fn search_matches_tags_and_returns_them_on_hits() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Some Post").await.unwrap();
        let doc_id = full.document.id;
        repo.set_document_tags(doc_id, &["rust".into(), "async".into()])
            .await
            .unwrap();
        repo.publish_document(doc_id).await.unwrap();

        let hits = repo.search_documents("async", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].tags.contains(&"rust".to_string()));
        assert!(hits[0].tags.contains(&"async".to_string()));
    }

    #[tokio::test]
    async fn search_snippet_escapes_body_markup() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Escaped").await.unwrap();
        let doc_id = full.document.id;
        let parsed = forgepost_content::parse_markdown(
            "needle <script>alert(1)</script> and <mark>more</mark> text",
        );
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        repo.publish_document(doc_id).await.unwrap();

        let hits = repo.search_documents("needle", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            !hits[0].snippet.contains("<script"),
            "raw markup must not reach the page: {}",
            hits[0].snippet
        );
        assert!(
            hits[0].snippet.contains("&lt;script&gt;"),
            "markup should be HTML-escaped: {}",
            hits[0].snippet
        );
        assert!(
            hits[0].snippet.contains("<mark>needle</mark>"),
            "match highlighting must survive escaping: {}",
            hits[0].snippet
        );
    }

    #[tokio::test]
    async fn search_handles_edge_queries() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Edge Cases").await.unwrap();
        let doc_id = full.document.id;
        let parsed = forgepost_content::parse_markdown("needle in a haystack");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        repo.publish_document(doc_id).await.unwrap();

        // Empty / whitespace-only queries never error and return nothing.
        assert!(repo.search_documents("", 10).await.unwrap().is_empty());
        assert!(repo.search_documents("   ", 10).await.unwrap().is_empty());
        // Quotes are neutralized rather than fatal.
        assert_eq!(
            repo.search_documents("needle\"", 10).await.unwrap().len(),
            1
        );
        // A bare asterisk must not break the MATCH parser.
        assert!(repo.search_documents("*", 10).await.is_ok());
    }

    #[tokio::test]
    async fn rebuild_search_index_all_is_idempotent() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Rebuild Me").await.unwrap();
        let doc_id = full.document.id;
        let parsed = forgepost_content::parse_markdown("rebuildable body");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        repo.publish_document(doc_id).await.unwrap();

        repo.rebuild_search_index_all().await.unwrap();
        assert_eq!(
            repo.search_documents("rebuildable", 10)
                .await
                .unwrap()
                .len(),
            1
        );
        // Rebuilding again must not duplicate rows.
        repo.rebuild_search_index_all().await.unwrap();
        assert_eq!(
            repo.search_documents("rebuildable", 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn fts_match_expr_builds_safe_expressions() {
        assert_eq!(fts_match_expr("hello world"), "\"hello\" \"world\"*");
        assert_eq!(fts_match_expr("async"), "\"async\"*");
        assert_eq!(fts_match_expr(""), "");
        assert_eq!(fts_match_expr("   "), "");
        assert_eq!(fts_match_expr("foo\"bar"), "\"foobar\"*");
    }

    #[tokio::test]
    async fn delete_document_removes_post_and_cascades_but_keeps_media_and_events() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Doomed Post").await.unwrap();
        let doc_id = full.document.id;

        // Blocks + versions, tags, a comment, and an experiment on a block.
        let parsed = forgepost_content::parse_markdown("some doomed body");
        let merged = forgepost_content::merge_blocks(&[], &[], parsed, now_ms());
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();
        let block_id = merged.blocks[0].id;
        repo.set_document_tags(doc_id, &["doomed".to_string()])
            .await
            .unwrap();
        repo.create_comment(doc_id, "Reader", "hi").await.unwrap();
        repo.create_experiment(
            doc_id,
            block_id,
            &forgepost_domain::model::NewExperiment {
                name: "Headline test".into(),
                goal: "completion".into(),
                traffic_weight: 50.0,
                confidence_threshold: 0.95,
                min_sample_per_variant: 100,
                no_winner_prob: 0.1,
                max_duration_ms: 7 * 86_400_000,
                variants: vec![forgepost_domain::model::ExperimentVariantInput {
                    content: json!("new headline"),
                    weight: 50.0,
                }],
            },
        )
        .await
        .unwrap();

        // Analytics event (document-scoped) and media (document-independent).
        repo.record_analytics_event(&AnalyticsEvent {
            id: Uuid::new_v4(),
            document_id: PostId(doc_id),
            event_type: "view".into(),
            band: None,
            block_id: Some(block_id),
            pageview_id: Uuid::new_v4(),
            visitor_id: VisitorId(Uuid::new_v4()),
            referrer: None,
            user_agent: None,
            read_time_ms: None,
            experiment_id: None,
            variant_id: None,
            recommended_slug: None,
            created_at_ms: now_ms(),
        })
        .await
        .unwrap();
        repo.insert_media(&Media {
            id: Uuid::new_v4(),
            disk_name: "keep.png".into(),
            content_type: "image/png".into(),
            size_bytes: 3,
            sha256: "abc".into(),
            created_at_ms: now_ms(),
        })
        .await
        .unwrap();

        // Publish so the FTS index has rows for this document.
        repo.publish_document(doc_id).await.unwrap();
        assert_eq!(repo.search_documents("doomed", 10).await.unwrap().len(), 1);

        repo.delete_document(doc_id).await.unwrap();

        // Gone from every listing, including the FTS virtual table (no FK).
        assert!(repo.get_document(doc_id).await.unwrap().is_none());
        assert!(repo.list_documents(user.id).await.unwrap().is_empty());
        assert!(repo.list_all_documents().await.unwrap().is_empty());
        assert!(
            repo.search_documents("doomed", 10)
                .await
                .unwrap()
                .is_empty()
        );

        // Cascaded dependents are cleared.
        assert!(repo.document_tags(doc_id).await.unwrap().is_empty());
        assert!(
            repo.comments_for_document(doc_id, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.experiments_for_document(doc_id)
                .await
                .unwrap()
                .is_empty()
        );
        let blocks: Vec<(String,)> = sqlx::query_as("SELECT id FROM blocks WHERE document_id = ?")
            .bind(doc_id.to_string())
            .fetch_all(&repo.pool)
            .await
            .unwrap();
        assert!(blocks.is_empty());

        // Media and analytics survive: media untouched, events keep rows with
        // their document_id set to NULL.
        assert!(repo.media_by_disk_name("keep.png").await.unwrap().is_some());
        let events: Vec<(Option<String>,)> =
            sqlx::query_as("SELECT document_id FROM analytics_events")
                .fetch_all(&repo.pool)
                .await
                .unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].0.is_none());

        // The slug is freed for reuse, and deleting a missing id errors.
        let again = repo.create_document(user.id, "Doomed Post").await.unwrap();
        assert_eq!(again.slug, "doomed-post");
        assert!(matches!(
            repo.delete_document(doc_id).await,
            Err(RepositoryError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn analytics_event_roundtrips_recommended_slug() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Read Now").await.unwrap();
        let doc_id = full.document.id;

        repo.record_analytics_event(&AnalyticsEvent {
            id: Uuid::new_v4(),
            document_id: PostId(doc_id),
            event_type: "recommendation_click".into(),
            band: None,
            block_id: None,
            pageview_id: Uuid::new_v4(),
            visitor_id: VisitorId(Uuid::new_v4()),
            referrer: None,
            user_agent: None,
            read_time_ms: None,
            experiment_id: None,
            variant_id: None,
            recommended_slug: Some("next-post".into()),
            created_at_ms: now_ms(),
        })
        .await
        .unwrap();

        let slug: Option<String> = sqlx::query_scalar(
            "SELECT recommended_slug FROM analytics_events WHERE event_type = 'recommendation_click'",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(slug.as_deref(), Some("next-post"));
    }

    #[tokio::test]
    async fn referrer_counts_group_by_referrer() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Sources").await.unwrap();
        let doc_id = full.document.id;

        let view = |pv: Uuid, visitor: VisitorId, referrer: Option<&str>| AnalyticsEvent {
            id: Uuid::new_v4(),
            document_id: PostId(doc_id),
            event_type: "view".into(),
            band: None,
            block_id: None,
            pageview_id: pv,
            visitor_id: visitor,
            referrer: referrer.map(String::from),
            user_agent: None,
            read_time_ms: None,
            experiment_id: None,
            variant_id: None,
            recommended_slug: None,
            created_at_ms: now_ms(),
        };

        repo.record_analytics_event(&view(
            Uuid::new_v4(),
            VisitorId(Uuid::new_v4()),
            Some("https://www.google.com/"),
        ))
        .await
        .unwrap();
        repo.record_analytics_event(&view(
            Uuid::new_v4(),
            VisitorId(Uuid::new_v4()),
            Some("https://www.google.com/"),
        ))
        .await
        .unwrap();
        repo.record_analytics_event(&view(Uuid::new_v4(), VisitorId(Uuid::new_v4()), None))
            .await
            .unwrap();
        // Non-view events must be ignored.
        repo.record_analytics_event(&AnalyticsEvent {
            event_type: "banded_scroll".into(),
            band: Some(100),
            ..view(Uuid::new_v4(), VisitorId(Uuid::new_v4()), None)
        })
        .await
        .unwrap();

        let counts = repo.referrer_counts(doc_id).await.unwrap();
        assert_eq!(counts.len(), 2);
        let google = counts
            .iter()
            .find(|(r, _)| r.as_deref() == Some("https://www.google.com/"))
            .expect("google referrer bucket");
        assert_eq!(google.1, 2);
        let direct = counts
            .iter()
            .find(|(r, _)| r.is_none())
            .expect("direct bucket");
        assert_eq!(direct.1, 1);
    }

    #[tokio::test]
    async fn dashboard_metrics_windows_and_completion() {
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Metrics").await.unwrap();
        let doc_id = full.document.id;

        let now = 1_000_000_000i64;
        let week = 7 * 24 * 60 * 60 * 1000;

        let event = |event_type: &str, band: Option<i64>, pv: Uuid, at: i64| AnalyticsEvent {
            id: Uuid::new_v4(),
            document_id: PostId(doc_id),
            event_type: event_type.into(),
            band,
            block_id: None,
            pageview_id: pv,
            visitor_id: VisitorId(Uuid::new_v4()),
            referrer: None,
            user_agent: None,
            read_time_ms: None,
            experiment_id: None,
            variant_id: None,
            recommended_slug: None,
            created_at_ms: at,
        };

        // Two views this week, one last week.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        repo.record_analytics_event(&event("view", None, a, now - 100))
            .await
            .unwrap();
        repo.record_analytics_event(&event("view", None, b, now - 200))
            .await
            .unwrap();
        repo.record_analytics_event(&event("view", None, c, now - week - 100))
            .await
            .unwrap();
        // One of this week's pageviews reached the end; the other only 25%.
        repo.record_analytics_event(&event("banded_scroll", Some(100), a, now - 50))
            .await
            .unwrap();
        repo.record_analytics_event(&event("banded_scroll", Some(25), b, now - 50))
            .await
            .unwrap();

        let metrics = repo.dashboard_metrics(now).await.unwrap();
        assert_eq!(metrics.len(), 1, "only the viewed document appears");
        let m = &metrics[0];
        assert_eq!(m.views_7d, 2);
        assert_eq!(m.views_prev_7d, 1);
        assert_eq!(m.views_total, 3);
        assert_eq!(m.completed, 1);
    }

    #[test]
    fn escape_snippet_keeps_marks_but_escapes_body_text() {
        assert_eq!(
            escape_snippet("hello <mark>payload</mark> world"),
            "hello <mark>payload</mark> world"
        );
        assert_eq!(
            escape_snippet("hello <script>alert(1)</script> here"),
            "hello &lt;script&gt;alert(1)&lt;/script&gt; here"
        );
        // Highlight markers inserted next to markup must survive escaping.
        assert_eq!(
            escape_snippet("see <mark>script</mark> <script>bad</script>"),
            "see <mark>script</mark> &lt;script&gt;bad&lt;/script&gt;"
        );
        // An unmatched marker that is really body text gets escaped.
        assert_eq!(escape_snippet("a </mark> b"), "a &lt;/mark&gt; b");
    }

    #[tokio::test]
    async fn resave_after_kind_change_parks_without_collision() {
        // Regression (live-DB bug): a block dropped in an earlier save parks at
        // `pos - 1e9`. If the row occupying that slot in a later save has a
        // smaller rowid, SQLite updates it *into* the parked slot before the
        // parked row vacates it, transiently violating UNIQUE(document_id, position).
        let repo = repo().await;
        let user = seed_user(&repo).await;
        let full = repo.create_document(user.id, "Churn").await.unwrap();
        let doc_id = full.document.id;

        let md = "# H\n\na\n\nb";
        let full = repo.get_document(doc_id).await.unwrap().unwrap();
        let parsed = forgepost_content::parse_markdown(md);
        let merged = forgepost_content::merge_blocks(
            &full.document.blocks,
            &full.document.versions,
            parsed,
            now_ms(),
        );
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();

        // Simulate a block that was dropped by an earlier save: parked at
        // position 1 - 1e9. Inserting it now gives it the newest rowid, so on
        // the next save SQLite processes the live block at position 1 first
        // and collides with this parked row before it can vacate.
        let parked_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO blocks (id, document_id, kind, position, current_version_id, created_at_ms, updated_at_ms)
             VALUES (?, ?, 'Paragraph', ?, ?, ?, ?)",
        )
        .bind(parked_id.to_string())
        .bind(doc_id.to_string())
        .bind(1 - 1_000_000_000i64)
        .bind(Uuid::new_v4().to_string())
        .bind(now_ms())
        .bind(now_ms())
        .execute(&repo.pool)
        .await
        .unwrap();

        let full = repo.get_document(doc_id).await.unwrap().unwrap();
        let parsed = forgepost_content::parse_markdown(md);
        let merged = forgepost_content::merge_blocks(
            &full.document.blocks,
            &full.document.versions,
            parsed,
            now_ms(),
        );
        repo.save_document_blocks(doc_id, &merged.blocks, &merged.versions)
            .await
            .unwrap();

        let live: Vec<(i64,)> =
            sqlx::query_as("SELECT position FROM blocks WHERE document_id = ? AND position >= 0")
                .bind(doc_id.to_string())
                .fetch_all(&repo.pool)
                .await
                .unwrap();
        assert_eq!(live, vec![(0,), (1,), (2,)]);
    }
}
