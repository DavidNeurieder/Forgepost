//! Server library: application wiring, routes, and the repository layer.

pub mod analytics;
pub mod auth;
pub mod error;
pub mod experiments;
pub mod import;
pub mod model;
pub mod pages;
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
    /// Set once TLS is active so session/visitor cookies get the `Secure` flag.
    pub secure_cookies: bool,
    /// Directory where uploaded media bytes live (`/media/*` serves them).
    pub media_dir: std::path::PathBuf,
}

/// Build the Axum router with the default analytics rate limit. Storage is
/// behind a `Repository` trait so a Postgres implementation can later be
/// swapped in without touching routes (§5.4). Uploads use a temp media dir.
pub fn app(repo: Arc<dyn Repository>) -> Router {
    app_with_config(
        repo,
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        false,
        None,
    )
}

/// Build the router with an explicit analytics rate limiter (tests use a tight
/// limit to exercise the 429 path).
pub fn app_with(repo: Arc<dyn Repository>, rate_limiter: RateLimiter) -> Router {
    app_with_config(repo, rate_limiter, false, None)
}

/// Build the router for HTTPS serving: cookies carry the `Secure` flag.
pub fn app_secure(repo: Arc<dyn Repository>) -> Router {
    app_with_config(repo, RateLimiter::new(RateLimiter::DEFAULT_MAX), true, None)
}

/// Build the router with an explicit media directory (upload handler writes
/// here; the serve handler reads from here).
pub fn app_with_media(
    repo: Arc<dyn Repository>,
    rate_limiter: RateLimiter,
    secure_cookies: bool,
    media_dir: std::path::PathBuf,
) -> Router {
    app_with_config(repo, rate_limiter, secure_cookies, Some(media_dir))
}

fn app_with_config(
    repo: Arc<dyn Repository>,
    rate_limiter: RateLimiter,
    secure_cookies: bool,
    media_dir: Option<std::path::PathBuf>,
) -> Router {
    let media_dir = match media_dir {
        Some(dir) => dir,
        None => std::env::temp_dir().join("forgepost-media"),
    };
    let state = AppState {
        repo,
        rate_limiter,
        secure_cookies,
        media_dir,
    };
    Router::new()
        // Server-rendered pages (the whole blog UI lives in the binary now).
        .route("/", get(pages::home_page))
        .route("/search", get(pages::search_page))
        .route("/tags/{tag}", get(pages::tag_page))
        .route("/setup", get(pages::setup_page).post(pages::setup_form))
        .route("/login", get(pages::login_page).post(pages::login_form))
        .route("/logout", post(pages::logout_form))
        .route("/admin", get(pages::dashboard_page))
        .route("/admin/new", post(pages::new_post))
        .route(
            "/admin/editor/{id}",
            get(pages::editor_page).post(pages::editor_save),
        )
        .route("/admin/editor/{id}/publish", post(pages::editor_publish))
        .route("/admin/editor/{id}/delete", post(pages::delete_post))
        .route("/admin/stats/{id}", get(pages::stats_page))
        .route(
            "/admin/stats/{id}/experiments",
            post(pages::create_experiment_page),
        )
        .route(
            "/admin/experiments/{id}/{action}",
            post(pages::experiment_action),
        )
        .route("/admin/comments/{id}/approve", post(pages::approve_comment))
        .route(
            "/admin/settings",
            get(pages::settings_page).post(pages::settings_form),
        )
        .route("/articles/{slug}", get(pages::article_page))
        .route("/articles/{slug}/comments", post(pages::comment_form))
        .route("/static/{name}", get(pages::static_file))
        .route("/media/{name}", get(pages::media_file))
        .route("/admin/media", post(pages::media_upload))
        .route("/admin/import", post(pages::import_post))
        .route("/rss", get(routes::rss))
        .route("/robots.txt", get(routes::robots_txt))
        .route("/sitemap.xml", get(routes::sitemap_xml))
        // Headless JSON API (unchanged; consumed by the pages above and by
        // external tools). Public read endpoints live under `/api/articles`.
        .route("/health", get(routes::health))
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
        .route(
            "/api/documents/{id}/experiments",
            get(routes::list_experiments),
        )
        .route("/api/experiments", post(routes::create_experiment))
        .route(
            "/api/experiments/{id}/start",
            post(routes::start_experiment),
        )
        .route("/api/experiments/{id}/stop", post(routes::stop_experiment))
        .route(
            "/api/experiments/{id}/decide",
            post(routes::decide_experiment),
        )
        .route(
            "/api/experiments/{id}/promote",
            post(routes::promote_experiment),
        )
        .route(
            "/api/experiments/{id}/no-winner",
            post(routes::conclude_no_winner),
        )
        .route("/api/rss", get(routes::rss))
        .with_state(state)
}
