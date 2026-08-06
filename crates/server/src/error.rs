//! Uniform JSON error envelope for API handlers.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::repository::RepositoryError;

pub struct ApiError(pub RepositoryError);

/// Like `ApiError` but renders an HTML error page instead of JSON. Page
/// handlers use this so `/admin`, `/articles/...` and friends show a styled
/// error page rather than a bare JSON body.
pub struct PageError(pub ApiError);

impl From<RepositoryError> for PageError {
    fn from(err: RepositoryError) -> Self {
        Self(ApiError::from(err))
    }
}

impl From<ApiError> for PageError {
    fn from(err: ApiError) -> Self {
        Self(err)
    }
}

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

impl ApiError {
    pub fn status_and_message(&self) -> (StatusCode, String) {
        match &self.0 {
            RepositoryError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            RepositoryError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            RepositoryError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".into()),
            RepositoryError::InvalidInput(m) => (StatusCode::BAD_REQUEST, m.clone()),
            RepositoryError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            RepositoryError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate limited".into()),
            RepositoryError::Uuid(_) => (StatusCode::BAD_REQUEST, "invalid id".into()),
            RepositoryError::Database(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            RepositoryError::Migration(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = self.status_and_message();
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}
