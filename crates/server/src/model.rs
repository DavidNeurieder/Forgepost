//! Server-side domain records (users, sessions, comments, documents).

use openpublish_content::Document;
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
