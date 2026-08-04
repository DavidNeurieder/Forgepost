//! HTTP routes (M0: health + setup status).

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{AppState, repository::RepositoryError};

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
}

pub async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

#[derive(Serialize)]
pub struct SetupStatus {
    pub setup_complete: bool,
}

pub async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    let setup_complete = state.repo.is_setup_complete().await?;
    Ok(Json(SetupStatus { setup_complete }))
}

/// Uniform JSON error envelope for API handlers.
pub struct ApiError(RepositoryError);

impl From<RepositoryError> for ApiError {
    fn from(err: RepositoryError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            RepositoryError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            RepositoryError::Database(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            RepositoryError::Migration(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}
