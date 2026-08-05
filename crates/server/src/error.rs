//! Uniform JSON error envelope for API handlers.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::repository::RepositoryError;

pub struct ApiError(pub RepositoryError);

impl ApiError {
    pub fn unauthorized() -> Self {
        Self(RepositoryError::Unauthorized)
    }
    pub fn forbidden() -> Self {
        Self(RepositoryError::Forbidden)
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(RepositoryError::InvalidInput(msg.into()))
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self(RepositoryError::Conflict(msg.into()))
    }
    pub fn rate_limited() -> Self {
        Self(RepositoryError::RateLimited)
    }
}

impl From<RepositoryError> for ApiError {
    fn from(err: RepositoryError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            RepositoryError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            RepositoryError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            RepositoryError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".into()),
            RepositoryError::InvalidInput(m) => (StatusCode::BAD_REQUEST, m),
            RepositoryError::Conflict(m) => (StatusCode::CONFLICT, m),
            RepositoryError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate limited".into()),
            RepositoryError::Uuid(_) => (StatusCode::BAD_REQUEST, "invalid id".into()),
            RepositoryError::Database(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            RepositoryError::Migration(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}
