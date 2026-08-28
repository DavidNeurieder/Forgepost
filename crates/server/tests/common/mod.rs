//! Shared harness for the security regression suite. Lives in `common/mod.rs`
//! (not `common.rs`) so Cargo does not treat it as its own test target.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use forgepost_analytics::RateLimiter;
use forgepost_infrastructure::sqlite::SqliteRepository;
use forgepost_server::app_with_security;
use forgepost_server::routes::ClientIpConfig;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::sync::Arc;
use tower::ServiceExt;

pub const CSRF_HEADER: &str = "x-csrf-token";

/// An in-memory pool. `SqliteRepository::from_pool` only needs one connection,
/// so a pool and repo built from the same handle observe the same database.
pub async fn pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("memory pool")
}

pub async fn migrated_repo(pool: SqlitePool) -> Arc<SqliteRepository> {
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    Arc::new(repo)
}

/// Default-limit router for the authorization/CSRF modules.
pub async fn security_app() -> Router {
    let pool = pool().await;
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    app_with_security(
        Arc::new(repo),
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_LOGIN_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_COMMENT_MAX),
        Arc::new(ClientIpConfig::default()),
        None,
        false,
        None,
    )
}

pub fn json_req(
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if let Some(t) = csrf {
        builder = builder.header(CSRF_HEADER, t);
    }
    match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

pub fn req(method: Method, uri: &str, cookie: Option<&str>, csrf: Option<&str>) -> Request<Body> {
    json_req(method, uri, cookie, csrf, None)
}

pub async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Response<Body>) {
    let resp = app.clone().oneshot(request).await.expect("router responds");
    (resp.status(), resp)
}

pub async fn body_text(resp: Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf-8 body")
}

pub async fn body_json(resp: Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("valid JSON")
}

/// Raw `Set-Cookie` header value (attributes included).
pub fn set_cookie(resp: &Response<Body>) -> String {
    resp.headers()
        .get(header::SET_COOKIE)
        .expect("Set-Cookie header")
        .to_str()
        .unwrap()
        .to_string()
}

/// Session cookie value with attributes stripped.
pub fn session_cookie(resp: &Response<Body>) -> String {
    set_cookie(resp)
        .split(';')
        .next()
        .unwrap()
        .trim()
        .to_string()
}

/// Complete setup through the API and return `(cookie, csrf)`.
pub async fn setup_owner(app: &Router) -> (String, String) {
    let (status, resp) = send(
        app,
        json_req(
            Method::POST,
            "/api/setup",
            None,
            None,
            Some(json!({
                "email": "a@b.com",
                "password": "password123",
                "display_name": "Alice",
            })),
        ),
    )
    .await;
    assert!(
        status.is_redirection() || status == StatusCode::OK,
        "setup failed: {status}"
    );
    let cookie = session_cookie(&resp);
    let csrf = fallback_csrf(body_json(resp).await, &cookie, app).await;
    (cookie, csrf)
}

/// `/api/setup` returns the session csrf in its body; if the body lacks the
/// field, fall back to the `/api/me` lookup.
async fn fallback_csrf(body: Value, cookie: &str, app: &Router) -> String {
    match body["csrf_token"].as_str() {
        Some(t) => t.to_string(),
        None => csrf_for(app, cookie).await,
    }
}

/// Session CSRF token for `cookie`.
pub async fn csrf_for(app: &Router, cookie: &str) -> String {
    let (status, resp) = send(app, req(Method::GET, "/api/me", Some(cookie), None)).await;
    assert!(
        status == StatusCode::OK,
        "expected authenticated /api/me, got {status}"
    );
    body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Create + publish a document, returning `(cookie, csrf, document_id, slug)`.
pub async fn seed_published_article(app: &Router) -> (String, String, String, String) {
    let (cookie, csrf) = setup_owner(app).await;
    let (_, resp) = send(
        app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({
                "title": "Security Suite Post",
                "markdown": "# Headline\n\nBody paragraph.",
                "tags": ["security"],
            })),
        ),
    )
    .await;
    let doc = body_json(resp).await;
    let id = doc["id"].as_str().unwrap().to_string();
    let slug = doc["slug"].as_str().unwrap().to_string();
    let (status, _) = send(
        app,
        json_req(
            Method::POST,
            &format!("/api/documents/{id}/publish"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (cookie, csrf, id, slug)
}
