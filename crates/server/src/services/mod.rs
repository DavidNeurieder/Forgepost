//! Application services: business logic extracted from HTTP handlers.
//!
//! Each service holds an `Arc<dyn Repository>` (and any other dependencies it
//! needs) and exposes domain-focused methods.  Handlers become thin: parse the
//! request → call a service method → map the result into an HTTP response.

pub mod analytics;
pub mod article;
pub mod auth;
pub mod comment;
pub mod document;
pub mod experiment;
pub mod settings;

use std::fmt;

// ---------------------------------------------------------------------------
// Service error
// ---------------------------------------------------------------------------

/// Domain-level error returned by every service method.
#[derive(Debug)]
pub enum ServiceError {
    /// Validation failed (bad input, missing fields, etc.).
    Validation(String),
    /// The requested entity was not found.
    NotFound(String),
    /// The caller is not authenticated.
    Unauthorized,
    /// Authenticated but not allowed.
    Forbidden,
    /// Logical conflict (e.g. setup already complete).
    Conflict(String),
    /// Rate limit exceeded.
    RateLimited,
    /// Database or infrastructure failure.
    Internal(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(m) => write!(f, "bad request: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::Forbidden => write!(f, "forbidden"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::RateLimited => write!(f, "rate limited"),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for ServiceError {}

/// Map a [`crate::repository::RepositoryError`] into a [`ServiceError`].
impl From<crate::repository::RepositoryError> for ServiceError {
    fn from(err: crate::repository::RepositoryError) -> Self {
        use crate::repository::RepositoryError;
        match err {
            RepositoryError::NotFound(m) => Self::NotFound(m),
            RepositoryError::Unauthorized => Self::Unauthorized,
            RepositoryError::Forbidden => Self::Forbidden,
            RepositoryError::InvalidInput(m) => Self::Validation(m),
            RepositoryError::Conflict(m) => Self::Conflict(m),
            RepositoryError::RateLimited => Self::RateLimited,
            RepositoryError::Database(e) => Self::Internal(e.to_string()),
            RepositoryError::Migration(e) => Self::Internal(e.to_string()),
            RepositoryError::Io(e) => Self::Internal(e.to_string()),
            RepositoryError::Uuid(e) => Self::Validation(format!("invalid id: {e}")),
        }
    }
}

impl From<std::io::Error> for ServiceError {
    fn from(err: std::io::Error) -> Self {
        Self::Internal(err.to_string())
    }
}
