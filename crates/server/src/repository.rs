//! Storage-agnostic repository layer.
//!
//! The domain never touches SQL. A Postgres implementation of [`Repository`]
//! can be added later without touching routes or domain logic (§5, §5.4).

use async_trait::async_trait;
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

#[async_trait]
pub trait Repository: Send + Sync {
    /// True once the first-time `/setup` wizard has been completed (M1 wires
    /// the actual admin creation; M0 only exposes the status).
    async fn is_setup_complete(&self) -> Result<bool, RepositoryError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("not found: {0}")]
    NotFound(String),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_and_setup_starts_incomplete() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory pool");
        let repo = SqliteRepository::from_pool(pool);
        repo.migrate().await.expect("migrations apply");

        assert!(!repo.is_setup_complete().await.expect("reads settings"));
    }
}
