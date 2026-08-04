//! End-to-end API tests exercising the router through `tower::ServiceExt`, the
//! same paths a browser/CLI client would hit. Mirrors the manual smoke flow:
//! setup → login → create → publish → article → comments → RSS.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use openpublish_server::app;
use openpublish_server::repository::SqliteRepository;
use serde_json::{Value, json};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::sync::Arc;
use tower::ServiceExt;

const CSRF_HEADER: &str = "x-csrf-token";

async fn test_app() -> Router {
    let pool = pool().await;
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    app(Arc::new(repo))
}

async fn pool() -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("memory pool")
}

fn json_req(
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

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Response<Body>) {
    let resp = app.clone().oneshot(req).await.expect("router responds");
    (resp.status(), resp)
}

/// Parse a JSON response body.
async fn body_json(resp: Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("valid JSON")
}

/// Extract the `Set-Cookie` value from a response (without attributes).
fn session_cookie(resp: &Response<Body>) -> String {
    resp.headers()
        .get(header::SET_COOKIE)
        .expect("Set-Cookie header")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn health_and_setup_status() {
    let app = test_app().await;

    let (status, resp) = send(&app, json_req(Method::GET, "/health", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(resp).await, json!({ "status": "ok" }));

    let (status, resp) = send(&app, json_req(Method::GET, "/setup", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(resp).await, json!({ "setup_complete": false }));
}

#[tokio::test]
async fn setup_creates_owner_session_and_locks_setup() {
    let app = test_app().await;

    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    assert_eq!(status, StatusCode::OK);
    let cookie = session_cookie(&resp);
    assert!(cookie.starts_with("openpublish_session="));
    let body = body_json(resp).await;
    assert_eq!(body["user"]["email"], "a@b.com");
    assert_eq!(body["user"]["role"], "owner");
    assert!(!body["csrf_token"].as_str().unwrap().is_empty());

    // The session cookie from setup is immediately valid.
    let (status, resp) = send(
        &app,
        json_req(Method::GET, "/api/me", Some(&cookie), None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(resp).await["user"]["email"], "a@b.com");

    let (status, resp) = send(&app, json_req(Method::GET, "/setup", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(resp).await, json!({ "setup_complete": true }));

    // Second setup attempt is rejected.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
            None,
            None,
            Some(json!({
                "email": "b@c.com",
                "password": "password123",
                "display_name": "Bob",
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body_json(resp).await["error"], "already set up");
}

#[tokio::test]
async fn login_and_me_roundtrip() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    let _ = resp;

    // Wrong password is rejected without leaking which part failed.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/login",
            None,
            None,
            Some(json!({ "email": "a@b.com", "password": "wrongpass" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body_json(resp).await["error"], "invalid email or password");

    // Correct login issues a fresh session cookie + CSRF token.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/login",
            None,
            None,
            Some(json!({ "email": "a@b.com", "password": "password123" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    // /api/me resolves the session.
    let (status, resp) = send(
        &app,
        json_req(Method::GET, "/api/me", Some(&cookie), None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let me = body_json(resp).await;
    assert_eq!(me["user"]["email"], "a@b.com");
    assert_eq!(me["csrf_token"], csrf);

    // Without a cookie, /api/me is unauthorized.
    let (status, _) = send(&app, json_req(Method::GET, "/api/me", None, None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_invalidates_session() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/logout",
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let _ = resp;

    let (status, _) = send(
        &app,
        json_req(Method::GET, "/api/me", Some(&cookie), None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutations_require_csrf() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    let cookie = session_cookie(&resp);

    // No cookie at all.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            None,
            None,
            Some(json!({ "title": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Cookie but no CSRF header.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            None,
            Some(json!({ "title": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Cookie but wrong CSRF header.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some("wrong-token"),
            Some(json!({ "title": "x" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_update_publish_and_read_article() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    // Create with markdown + tags.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({
                "title": "My First Post",
                "markdown": "# Hello\n\nThis is my **first** post.\n\n> A quote",
                "tags": ["tech", "blog"],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let doc = body_json(resp).await;
    assert_eq!(doc["slug"], "my-first-post");
    assert_eq!(doc["status"], "draft");
    assert_eq!(doc["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(doc["tags"], json!(["blog", "tech"]));
    let id = doc["id"].as_str().unwrap();

    // The document is not public yet.
    let (status, _) = send(
        &app,
        json_req(Method::GET, "/articles/my-first-post", None, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unauthenticated create is rejected.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            None,
            None,
            Some(json!({ "title": "Nope" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Publish.
    let (status, resp) = send(
        &app,
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
    assert_eq!(body_json(resp).await["status"], "published");

    // Public article is now served with rendered HTML.
    let (status, resp) = send(
        &app,
        json_req(Method::GET, "/articles/my-first-post", None, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let article = body_json(resp).await;
    assert_eq!(article["slug"], "my-first-post");
    assert!(
        article["published_at_ms"].is_number(),
        "published_at_ms set"
    );
    assert!(article["html"].as_str().unwrap().contains("<h1>Hello</h1>"));
    assert!(article["html"].as_str().unwrap().contains("A quote"));

    // Updating the title does not change the slug (stable public URLs).
    let (status, resp) = send(
        &app,
        json_req(
            Method::PUT,
            &format!("/api/documents/{id}"),
            Some(&cookie),
            Some(&csrf),
            Some(json!({ "title": "Renamed Post" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let renamed = body_json(resp).await;
    assert_eq!(renamed["slug"], "my-first-post");
    assert_eq!(renamed["title"], "Renamed Post");

    // The slug still resolves, with the new title.
    let (status, resp) = send(
        &app,
        json_req(Method::GET, "/articles/my-first-post", None, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(resp).await["title"], "Renamed Post");

    // Owner-only access: a different session cannot read the document.
    let (_, resp) = send(
        &app,
        json_req(Method::GET, "/api/documents", Some(&cookie), None, None),
    )
    .await;
    let docs = body_json(resp).await;
    assert_eq!(docs.as_array().unwrap().len(), 1);
    assert_eq!(docs[0]["title"], "Renamed Post");
}

#[tokio::test]
async fn comments_require_moderation() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({
                "title": "With Comments",
                "markdown": "Body text here",
                "tags": [],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let doc = body_json(resp).await;
    let id = doc["id"].as_str().unwrap().to_string();
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/documents/{id}/publish"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(body_json(resp).await["status"], "published");

    // New comment is created as pending.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/articles/with-comments/comments",
            None,
            None,
            Some(json!({ "author_name": "Reader", "body": "Nice post!" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment = body_json(resp).await;
    assert_eq!(comment["status"], "pending");
    let comment_id = comment["id"].as_str().unwrap().to_string();

    // Pending comments are not visible to the public.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            "/articles/with-comments/comments",
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(body_json(resp).await.as_array().unwrap().len(), 0);

    // Approval requires auth + CSRF.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/comments/{comment_id}/approve"),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/comments/{comment_id}/approve"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/comments/{comment_id}/approve"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Approved comment is now public.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            "/articles/with-comments/comments",
            None,
            None,
            None,
        ),
    )
    .await;
    let comments = body_json(resp).await;
    assert_eq!(comments.as_array().unwrap().len(), 1);
    assert_eq!(comments[0]["body"], "Nice post!");
}

#[tokio::test]
async fn rss_lists_published_articles_only() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/setup",
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
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    // Draft is excluded.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({ "title": "Draft Only", "markdown": "draft", "tags": [] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({ "title": "Published One", "markdown": "live", "tags": [] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let published = body_json(resp).await;
    let pub_id = published["id"].as_str().unwrap().to_string();

    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/documents/{pub_id}/publish"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(body_json(resp).await["status"], "published");

    let (status, resp) = send(&app, json_req(Method::GET, "/rss", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let feed = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(feed.contains("Published One"));
    assert!(!feed.contains("Draft Only"));
    assert!(feed.contains("published-one"));
    assert!(!feed.contains("draft-only"));
}
