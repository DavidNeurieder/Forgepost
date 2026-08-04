//! Server library: application wiring, routes, and the repository layer.

pub mod repository;
pub mod routes;

use std::sync::Arc;

use axum::Router;

use crate::repository::Repository;

/// Shared application state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<dyn Repository>,
}

/// Build the Axum router. Storage is behind a `Repository` trait so a Postgres
/// implementation can later be swapped in without touching routes (§5.4).
pub fn app(repo: Arc<dyn Repository>) -> Router {
    let state = AppState { repo };
    Router::new()
        .route("/health", axum::routing::get(routes::health))
        .route("/setup", axum::routing::get(routes::setup_status))
        .with_state(state)
}
