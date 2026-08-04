//! Storage-agnostic repository layer.
//!
//! The domain never touches SQL. A Postgres implementation of [`Repository`]
//! can be added later without touching routes or domain logic (§5, §5.4).

use async_trait::async_trait;
use openpublish_content::{
    Block, BlockContent, BlockKind, BlockVersion, Document, DocumentId, now_ms,
};
use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

use crate::auth::{SESSION_TTL_MS, sha256_hex};
use crate::model::{Comment, DocumentSummary, FullDocument, Session, User};

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

        Ok(json!({
            "version": 1,
            "exported_at_ms": now_ms(),
            "settings": settings,
            "users": users,
            "documents": documents,
            "comments": comments,
        }))
    }
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
