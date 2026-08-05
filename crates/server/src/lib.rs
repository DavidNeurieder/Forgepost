//! Server library: application wiring, routes, and the repository layer.

pub mod analytics;
pub mod auth;
pub mod error;
pub mod model;
pub mod repository;
pub mod routes;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::analytics::RateLimiter;
use crate::repository::Repository;

/// Shared application state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<dyn Repository>,
    pub rate_limiter: RateLimiter,
}

/// Build the Axum router with the default analytics rate limit. Storage is
/// behind a `Repository` trait so a Postgres implementation can later be
/// swapped in without touching routes (§5.4).
pub fn app(repo: Arc<dyn Repository>) -> Router {
    app_with(repo, RateLimiter::new(RateLimiter::DEFAULT_MAX))
}

/// Build the router with an explicit analytics rate limiter (tests use a tight
/// limit to exercise the 429 path).
pub fn app_with(repo: Arc<dyn Repository>, rate_limiter: RateLimiter) -> Router {
    let state = AppState { repo, rate_limiter };
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
        .route("/api/events", post(routes::record_event))
        .route("/api/documents/{id}/stats", get(routes::document_stats))
        .route("/articles/{slug}", get(routes::article))
        .route(
            "/articles/{slug}/comments",
            get(routes::list_comments).post(routes::create_comment),
        )
        .route("/rss", get(routes::rss))
        .route("/api/rss", get(routes::rss))
        .with_state(state)
}
