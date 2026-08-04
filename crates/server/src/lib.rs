//! Server library: application wiring, routes, and the repository layer.

pub mod auth;
pub mod error;
pub mod model;
pub mod repository;
pub mod routes;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

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
        .route("/health", get(routes::health))
        .route("/setup", get(routes::setup_status).post(routes::setup))
        .route("/api/setup", get(routes::setup_status).post(routes::setup))
        .route("/api/login", post(routes::login))
        .route("/api/logout", post(routes::logout))
        .route("/api/me", get(routes::me))
        .route(
            "/api/documents",
            get(routes::list_documents).post(routes::create_document),
        )
        .route(
            "/api/documents/{id}",
            get(routes::get_document).put(routes::update_document),
        )
        .route(
            "/api/documents/{id}/publish",
            post(routes::publish_document),
        )
        .route("/api/comments/{id}/approve", post(routes::approve_comment))
        .route("/api/comments/pending", get(routes::pending_comments))
        .route("/api/articles", get(routes::list_articles))
        .route("/api/articles/{slug}", get(routes::article))
        .route(
            "/api/articles/{slug}/comments",
            get(routes::list_comments).post(routes::create_comment),
        )
        .route("/api/render", post(routes::render_markdown))
        .route("/articles/{slug}", get(routes::article))
        .route(
            "/articles/{slug}/comments",
            get(routes::list_comments).post(routes::create_comment),
        )
        .route("/rss", get(routes::rss))
        .route("/api/rss", get(routes::rss))
        .with_state(state)
}
