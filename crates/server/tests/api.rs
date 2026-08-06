//! End-to-end API tests exercising the router through `tower::ServiceExt`, the
//! same paths a browser/CLI client would hit. Mirrors the manual smoke flow:
//! setup → login → create → publish → article → comments → RSS.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use openpublish_experiments::assign_variant;
use openpublish_server::app;
use openpublish_server::repository::SqliteRepository;
use serde_json::{Value, json};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

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

    let (status, resp) = send(&app, json_req(Method::GET, "/api/setup", None, None, None)).await;
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

    let (status, resp) = send(&app, json_req(Method::GET, "/api/setup", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_json(resp).await, json!({ "setup_complete": true }));

    // Second setup attempt is rejected.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/setup",
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
        json_req(Method::GET, "/api/articles/my-first-post", None, None, None),
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
        json_req(Method::GET, "/api/articles/my-first-post", None, None, None),
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
        json_req(Method::GET, "/api/articles/my-first-post", None, None, None),
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
            "/api/articles/with-comments/comments",
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
            "/api/articles/with-comments/comments",
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
            "/api/articles/with-comments/comments",
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
    // Links derive from the Host (no site.url configured in tests) and the
    // pubDate is an RFC-822 timestamp rather than a bare epoch.
    assert!(feed.contains("<link>http://localhost/articles/published-one</link>"));
    assert!(feed.contains("<pubDate>"));
    assert!(!feed.contains("example.invalid"));

    let (status, resp) = send(&app, json_req(Method::GET, "/robots.txt", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let robots = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(robots.contains("User-agent: *"));
    assert!(robots.contains("Sitemap: http://localhost/sitemap.xml"));

    let (status, resp) = send(&app, json_req(Method::GET, "/sitemap.xml", None, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let sitemap = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(sitemap.contains("<loc>http://localhost/</loc>"));
    assert!(sitemap.contains("<loc>http://localhost/articles/published-one</loc>"));
    assert!(!sitemap.contains("draft-only"));
}

#[tokio::test]
async fn render_preview_parses_and_renders() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
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
    let cookie = session_cookie(&resp);

    // Render requires auth.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/render",
            None,
            None,
            Some(json!({ "markdown": "# Hi\n\nBody" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/render",
            Some(&cookie),
            None,
            Some(json!({ "markdown": "# Hi\n\nSome **bold** body\n\n> quote" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let view = body_json(resp).await;
    assert_eq!(view["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(view["blocks"][0]["kind"], "Heading { level: 1 }");
    assert!(view["html"].as_str().unwrap().contains("<h1>Hi</h1>"));
    assert!(
        view["html"]
            .as_str()
            .unwrap()
            .contains("<blockquote>quote</blockquote>")
    );
}

#[tokio::test]
async fn articles_list_is_public_and_lists_published_only() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
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
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    let (_, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({ "title": "Hidden Draft", "markdown": "draft", "tags": [] })),
        ),
    )
    .await;
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({ "title": "Public Post", "markdown": "live", "tags": [] })),
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

    // Public endpoint, no auth required.
    let (status, resp) = send(
        &app,
        json_req(Method::GET, "/api/articles", None, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let articles = body_json(resp).await;
    assert_eq!(articles.as_array().unwrap().len(), 1);
    assert_eq!(articles[0]["slug"], "public-post");
    assert_eq!(articles[0]["title"], "Public Post");
}

#[tokio::test]
async fn pending_comments_are_listed_and_approvable() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
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
            Some(json!({ "title": "Moderate Me", "markdown": "body", "tags": [] })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = body_json(resp).await["id"].as_str().unwrap().to_string();
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

    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/articles/moderate-me/comments",
            None,
            None,
            Some(json!({ "author_name": "Spam", "body": "buy my stuff" })),
        ),
    )
    .await;
    let comment_id = body_json(resp).await["id"].as_str().unwrap().to_string();

    // Pending list requires auth.
    let (status, _) = send(
        &app,
        json_req(Method::GET, "/api/comments/pending", None, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, resp) = send(
        &app,
        json_req(
            Method::GET,
            "/api/comments/pending",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pending = body_json(resp).await;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["body"], "buy my stuff");

    let (_, _resp) = send(
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
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            "/api/comments/pending",
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(body_json(resp).await.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn editing_with_insert_keeps_stable_block_ids() {
    let app = test_app().await;
    let (_, resp) = send(
        &app,
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
                "title": "Stable IDs",
                "markdown": "# Heading\n\nOriginal body.",
                "tags": [],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let doc = body_json(resp).await;
    let id = doc["id"].as_str().unwrap().to_string();
    let heading_id = doc["blocks"][0]["id"].as_str().unwrap().to_string();

    // Insert a paragraph at the top (before the heading) and reword body.
    let (status, resp) = send(
        &app,
        json_req(
            Method::PUT,
            &format!("/api/documents/{id}"),
            Some(&cookie),
            Some(&csrf),
            Some(json!({
                "title": "Stable IDs",
                "markdown": "Inserted at top.\n\n# Heading\n\nReworded body.",
                "tags": [],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "edit with insert must not collide");
    let edited = body_json(resp).await;
    assert_eq!(edited["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(
        edited["blocks"][1]["id"], heading_id,
        "heading keeps its id"
    );

    // Publish and confirm the public article reflects the edit.
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
    let (_, resp) = send(
        &app,
        json_req(Method::GET, "/api/articles/stable-ids", None, None, None),
    )
    .await;
    let article = body_json(resp).await;
    let html = article["html"].as_str().unwrap();
    assert!(html.contains("Inserted at top."));
    assert!(html.contains("<h1>Heading</h1>"));
}

// ---------------------------------------------------------------------------
// M2: analytics
// ---------------------------------------------------------------------------

/// Setup an owner, create a 3-block article, and publish it. Returns the owner
/// session cookie, CSRF token, document id, slug, and the published block ids.
async fn seed_published_article(app: &Router) -> (String, String, String, String, Vec<String>) {
    let (_, resp) = send(
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
    let cookie = session_cookie(&resp);
    let csrf = body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();

    let (_, resp) = send(
        app,
        json_req(
            Method::POST,
            "/api/documents",
            Some(&cookie),
            Some(&csrf),
            Some(json!({
                "title": "Analytics Post",
                "markdown": "# Headline\n\nBody paragraph one.\n\nBody paragraph two.",
                "tags": ["tech"],
            })),
        ),
    )
    .await;
    let doc = body_json(resp).await;
    let id = doc["id"].as_str().unwrap().to_string();
    let slug = doc["slug"].as_str().unwrap().to_string();
    let block_ids: Vec<String> = doc["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_string())
        .collect();

    let (_, resp) = send(
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
    assert_eq!(body_json(resp).await["status"], "published");
    (cookie, csrf, id, slug, block_ids)
}

#[tokio::test]
async fn analytics_events_record_and_aggregate() {
    let app = test_app().await;
    let (session_cookie, _, id, slug, block_ids) = seed_published_article(&app).await;
    let visitor = "11111111-1111-1111-1111-111111111111";
    let session = "22222222-2222-2222-2222-222222222222";

    // First event mints the visitor cookie.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            None,
            None,
            Some(json!({
                "slug": slug,
                "session_id": session,
                "kind": "view",
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .expect("visitor cookie minted")
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.starts_with("opv="));

    let cookie = format!("opv={visitor}");
    let post = |body: Value| async {
        let (status, resp) = send(
            &app,
            json_req(Method::POST, "/api/events", Some(&cookie), None, Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "event accepted");
        resp
    };

    // Scroll through the whole article.
    for band in [25, 50, 75, 100] {
        post(json!({
            "slug": slug, "session_id": session, "kind": "banded_scroll",
            "payload": { "band": band },
        }))
        .await;
    }
    post(json!({
        "slug": slug, "session_id": session, "kind": "article_read",
        "payload": { "read_time_ms": 42_000 },
    }))
    .await;
    for bid in &block_ids {
        post(json!({
            "slug": slug, "session_id": session, "kind": "block_impression",
            "block_id": bid, "payload": {},
        }))
        .await;
    }

    let (status, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/stats"),
            Some(&session_cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stats = body_json(resp).await;

    assert_eq!(stats["article"]["views"], 1);
    assert_eq!(stats["article"]["unique_readers"], 1);
    assert_eq!(stats["article"]["read_events"], 1);
    assert_eq!(stats["article"]["avg_read_time_ms"], 42_000);
    assert_eq!(stats["article"]["completion"], 1.0);

    let bands = stats["article"]["band_reach"].as_array().unwrap();
    assert_eq!(bands.len(), 4);
    assert_eq!(bands[0]["band"], 25);
    assert_eq!(bands[0]["pageviews"], 1);
    assert_eq!(bands[3]["band"], 100);
    assert_eq!(bands[3]["pageviews"], 1);

    // Every block reported an impression and an estimated reach.
    let blocks = stats["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);
    for b in blocks {
        assert_eq!(b["impressions"], 1);
        assert_eq!(b["is_estimate"], true);
    }
    // Single-pageview sample: first block reached by the viewer.
    assert_eq!(blocks[0]["estimated_reach"], 1);
    assert!(blocks[0]["preview"].as_str().unwrap().contains("Headline"));
}

#[tokio::test]
async fn analytics_rejects_unknown_slug_and_bad_payloads() {
    let app = test_app().await;
    let (_, _, _, slug, block_ids) = seed_published_article(&app).await;

    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            None,
            None,
            Some(json!({
                "slug": "does-not-exist",
                "session_id": "22222222-2222-2222-2222-222222222222",
                "kind": "view",
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            None,
            None,
            Some(json!({
                "slug": slug,
                "session_id": "22222222-2222-2222-2222-222222222222",
                "kind": "banded_scroll",
                "payload": { "band": 37 },
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "invalid band rejected");

    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            None,
            None,
            Some(json!({
                "slug": slug,
                "session_id": "22222222-2222-2222-2222-222222222222",
                "kind": "block_impression",
                "block_id": "33333333-3333-3333-3333-333333333333",
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown block rejected");

    // Experiments are not supported until M3.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            None,
            None,
            Some(json!({
                "slug": slug,
                "session_id": "22222222-2222-2222-2222-222222222222",
                "kind": "experiment_impression",
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let _ = block_ids;
}

#[tokio::test]
async fn analytics_stats_require_owner() {
    let app = test_app().await;
    let (_, _, id, _, _) = seed_published_article(&app).await;

    let (status, _) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/stats"),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Send one tracking event from `visitor` with an explicit visitor cookie so
/// several distinct readers can be simulated without persisting the minted
/// cookie between calls.
async fn post_event(app: &Router, visitor: &str, body: Value) {
    let (status, _) = send(
        app,
        json_req(
            Method::POST,
            "/api/events",
            Some(&format!("opv={visitor}")),
            None,
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "event accepted");
}

/// Full drop-off scenario across two visitors:
///   visitor A reads the whole article; visitor B only reaches the top.
/// This exercises events → aggregations → estimated per-block reach → drop-off.
#[tokio::test]
async fn analytics_multi_visitor_dropoff() {
    let app = test_app().await;
    let (session_cookie, _, id, slug, block_ids) = seed_published_article(&app).await;
    assert_eq!(block_ids.len(), 3, "seed article has 3 blocks");

    // Visitor A: full read + impressions for the first two blocks.
    let a = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let a_session = "aaaaaa01-0000-0000-0000-000000000000";
    post_event(
        &app,
        a,
        json!({ "slug": slug, "session_id": a_session, "kind": "view", "payload": {} }),
    )
    .await;
    for band in [25, 50, 75, 100] {
        post_event(
            &app,
            a,
            json!({
                "slug": slug, "session_id": a_session, "kind": "banded_scroll",
                "payload": { "band": band },
            }),
        )
        .await;
    }
    post_event(
        &app,
        a,
        json!({
            "slug": slug, "session_id": a_session, "kind": "article_read",
            "payload": { "read_time_ms": 42_000 },
        }),
    )
    .await;
    post_event(
        &app,
        a,
        json!({
            "slug": slug, "session_id": a_session, "kind": "block_impression",
            "block_id": block_ids[0], "payload": {},
        }),
    )
    .await;
    post_event(
        &app,
        a,
        json!({
            "slug": slug, "session_id": a_session, "kind": "block_impression",
            "block_id": block_ids[1], "payload": {},
        }),
    )
    .await;

    // Visitor B: only reaches the top of the page.
    let b = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let b_session = "bbbbbb01-0000-0000-0000-000000000000";
    post_event(
        &app,
        b,
        json!({ "slug": slug, "session_id": b_session, "kind": "view", "payload": {} }),
    )
    .await;
    post_event(
        &app,
        b,
        json!({
            "slug": slug, "session_id": b_session, "kind": "banded_scroll",
            "payload": { "band": 25 },
        }),
    )
    .await;
    post_event(
        &app,
        b,
        json!({
            "slug": slug, "session_id": b_session, "kind": "block_impression",
            "block_id": block_ids[0], "payload": {},
        }),
    )
    .await;

    let (status, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/stats"),
            Some(&session_cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stats = body_json(resp).await;

    // Article-level.
    assert_eq!(stats["article"]["views"], 2);
    assert_eq!(stats["article"]["unique_readers"], 2);
    assert_eq!(stats["article"]["avg_read_time_ms"], 42_000);
    assert_eq!(stats["article"]["read_events"], 1);
    assert_eq!(stats["article"]["completion"], 0.5);

    // Cumulative scroll bands: A reached everything, B only 25%.
    let bands: serde_json::Map<String, Value> = stats["article"]["band_reach"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["band"].to_string(), b["pageviews"].clone()))
        .collect();
    assert_eq!(bands["25"], json!(2));
    assert_eq!(bands["50"], json!(1));
    assert_eq!(bands["75"], json!(1));
    assert_eq!(bands["100"], json!(1));

    // Per-block: impressions are exact; reach is scroll-estimated.
    let blocks = stats["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);
    let by_pos: std::collections::HashMap<Value, &Value> =
        blocks.iter().map(|b| (b["position"].clone(), b)).collect();
    let b0 = by_pos[&json!(0)];
    let b1 = by_pos[&json!(1)];
    assert_eq!(b0["impressions"], 2, "both visitors rendered block 0");
    assert_eq!(b0["estimated_reach"], 2, "everyone reaches the top block");
    assert_eq!(b0["estimated_dropoff"], 0);
    assert_eq!(b1["impressions"], 1, "only visitor A rendered block 1");
    assert_eq!(b1["estimated_reach"], 1, "visitor B stopped before block 1");
    assert_eq!(b1["estimated_dropoff"], 1, "one reader leaves at block 1");
    assert_eq!(blocks[0]["is_estimate"], true);
    assert!(blocks[0]["preview"].as_str().unwrap().contains("Headline"));
}

#[tokio::test]
async fn analytics_events_are_rate_limited() {
    use openpublish_server::analytics::RateLimiter;
    use openpublish_server::app_with;

    let pool = pool().await;
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    let app = app_with(Arc::new(repo), RateLimiter::new(3));
    let (_, _, _, slug, _) = seed_published_article(&app).await;

    let event = json!({
        "slug": slug,
        "session_id": "22222222-2222-2222-2222-222222222222",
        "kind": "view",
        "payload": {},
    });
    for _ in 0..3 {
        let (status, _) = send(
            &app,
            json_req(Method::POST, "/api/events", None, None, Some(event.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = send(
        &app,
        json_req(Method::POST, "/api/events", None, None, Some(event)),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn article_includes_rendered_blocks() {
    let app = test_app().await;
    let (_, _, _, slug, block_ids) = seed_published_article(&app).await;

    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/articles/{slug}"),
            None,
            None,
            None,
        ),
    )
    .await;
    let article = body_json(resp).await;
    let rendered = article["rendered_blocks"].as_array().unwrap();
    assert_eq!(rendered.len(), 3);
    for (rb, bid) in rendered.iter().zip(&block_ids) {
        assert_eq!(rb["id"], json!(bid));
        assert!(!rb["html"].as_str().unwrap().is_empty());
    }
    assert!(rendered[0]["html"].as_str().unwrap().contains("<h1>"));
}

// ---------------------------------------------------------------------------
// Experiments (M3)
// ---------------------------------------------------------------------------

/// Create an experiment on `block_id` (a "New headline" variant) and start it.
/// Returns the create response (includes `id` and `variants`).
async fn seed_running_experiment(
    app: &Router,
    cookie: &str,
    csrf: &str,
    doc_id: &str,
    block_id: &str,
    min_sample: u64,
) -> Value {
    let (status, resp) = send(
        app,
        json_req(
            Method::POST,
            "/api/experiments",
            Some(cookie),
            Some(csrf),
            Some(json!({
                "document_id": doc_id,
                "block_id": block_id,
                "name": "Headline test",
                "traffic_weight": 50,
                "confidence_threshold": 0.95,
                "min_sample_per_variant": min_sample,
                "variants": [
                    { "content": { "text": "New headline" }, "weight": 50 },
                ],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "experiment created");
    let exp = body_json(resp).await;
    let id = exp["id"].as_str().unwrap().to_string();
    let (status, _) = send(
        app,
        json_req(
            Method::POST,
            &format!("/api/experiments/{id}/start"),
            Some(cookie),
            Some(csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "experiment started");
    exp
}

/// Control/variant ids from an experiment view.
fn variant_ids(exp: &Value) -> (String, String) {
    let variants = exp["variants"].as_array().unwrap();
    let control = variants
        .iter()
        .find(|v| v["is_control"] == json!(true))
        .expect("control variant");
    let variant = variants
        .iter()
        .find(|v| v["is_control"] == json!(false))
        .expect("non-control variant");
    (
        control["id"].as_str().unwrap().to_string(),
        variant["id"].as_str().unwrap().to_string(),
    )
}

/// Distinct visitors whose traffic-split assignment matches `want` (true =
/// variant, false = control), mirroring the server's deterministic assignment.
fn visitors_for(exp_id: Uuid, control: Uuid, variant: Uuid, want: bool, n: usize) -> Vec<Uuid> {
    let mut out = Vec::new();
    let mut i = 0u128;
    while out.len() < n {
        i += 1;
        let v = Uuid::from_u128(i);
        let chosen = assign_variant(&exp_id, &v, control, 0.5, &[(variant, 50.0)]);
        if (chosen == variant) == want {
            out.push(v);
        }
    }
    out
}

/// Impress (and optionally convert) a visitor on an experiment.
async fn post_experiment_event(
    app: &Router,
    slug: &str,
    visitor: Uuid,
    exp_id: &str,
    variant_id: &str,
    convert: bool,
) {
    post_event(
        app,
        &visitor.to_string(),
        json!({
            "slug": slug,
            "session_id": Uuid::new_v4(),
            "kind": "experiment_impression",
            "experiment_id": exp_id,
            "variant_id": variant_id,
            "payload": {},
        }),
    )
    .await;
    if convert {
        post_event(
            app,
            &visitor.to_string(),
            json!({
                "slug": slug,
                "session_id": Uuid::new_v4(),
                "kind": "experiment_conversion",
                "experiment_id": exp_id,
                "variant_id": variant_id,
                "payload": {},
            }),
        )
        .await;
    }
}

#[tokio::test]
async fn experiment_lifecycle_and_live_report() {
    let app = test_app().await;
    let (cookie, csrf, id, slug, block_ids) = seed_published_article(&app).await;
    let exp = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 100).await;
    let exp_id = exp["id"].as_str().unwrap().to_string();
    assert_eq!(exp["goal"], "completion");
    assert_eq!(exp["variants"].as_array().unwrap().len(), 2);

    // Fresh report: no data, no decision, engine recommends continuing.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/experiments"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    let list = body_json(resp).await;
    let e = &list.as_array().unwrap()[0];
    assert_eq!(e["status"], "running");
    assert_eq!(e["report"]["recommendation"]["type"], "continue");
    assert_eq!(e["report"]["variants"].as_array().unwrap().len(), 2);
    assert_eq!(e["decisions"].as_array().unwrap().len(), 0);

    // Experiments need at least one variant.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/experiments",
            Some(&cookie),
            Some(&csrf),
            Some(json!({
                "document_id": id,
                "block_id": block_ids[1],
                "variants": [],
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Owner-only: unauthenticated reads are rejected.
    let (status, _) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/experiments"),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let _ = slug;
    let _ = exp_id;
}

#[tokio::test]
async fn experiment_serves_stable_variants_to_visitors() {
    let app = test_app().await;
    let (cookie, csrf, id, slug, block_ids) = seed_published_article(&app).await;
    let exp = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 100).await;
    let exp_id = exp["id"].as_str().unwrap().to_string();
    let (control_id, variant_id) = variant_ids(&exp);

    let visitor = "99999999-9999-9999-9999-999999999999";
    let get_article = || async {
        let (_, resp) = send(
            &app,
            json_req(
                Method::GET,
                &format!("/api/articles/{slug}"),
                Some(&format!("opv={visitor}")),
                None,
                None,
            ),
        )
        .await;
        body_json(resp).await
    };

    let a1 = get_article().await;
    let rb = &a1["rendered_blocks"][0];
    assert_eq!(rb["experiment_id"], json!(exp_id));
    let assigned = rb["variant_id"].as_str().unwrap().to_string();
    assert!(assigned == control_id || assigned == variant_id);
    let expected = if assigned == variant_id {
        "New headline"
    } else {
        "Headline"
    };
    assert!(
        rb["html"].as_str().unwrap().contains(expected),
        "block shows the assigned variant: {}",
        rb["html"]
    );
    // Non-experiment blocks carry no experiment attributes.
    assert!(a1["rendered_blocks"][1]["experiment_id"].is_null());
    assert!(a1["rendered_blocks"][1]["variant_id"].is_null());

    // Reloading with the same visitor keeps the same assignment.
    let a2 = get_article().await;
    assert_eq!(a2["rendered_blocks"][0]["variant_id"], json!(assigned));
}

#[tokio::test]
async fn experiment_rejects_invalid_events_and_requires_owner() {
    let app = test_app().await;
    let (cookie, csrf, id, slug, block_ids) = seed_published_article(&app).await;
    let exp = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 100).await;
    let exp_id = exp["id"].as_str().unwrap().to_string();
    let (_, variant_id) = variant_ids(&exp);
    let session = Uuid::new_v4();

    // Unknown experiment id -> 400.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            Some("opv=v1"),
            None,
            Some(json!({
                "slug": slug,
                "session_id": session,
                "kind": "experiment_impression",
                "experiment_id": "00000000-0000-0000-0000-000000000000",
                "variant_id": variant_id,
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Variant from a different experiment -> 400.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            Some("opv=v1"),
            None,
            Some(json!({
                "slug": slug,
                "session_id": session,
                "kind": "experiment_impression",
                "experiment_id": exp_id,
                "variant_id": "00000000-0000-0000-0000-000000000000",
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A stopped experiment rejects further impressions.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/experiments/{exp_id}/stop"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/events",
            Some("opv=v1"),
            None,
            Some(json!({
                "slug": slug,
                "session_id": session,
                "kind": "experiment_impression",
                "experiment_id": exp_id,
                "variant_id": variant_id,
                "payload": {},
            })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn experiment_promotes_clear_winner() {
    let app = test_app().await;
    let (cookie, csrf, id, slug, block_ids) = seed_published_article(&app).await;
    let exp = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 2).await;
    let exp_id = Uuid::from_str(exp["id"].as_str().unwrap()).unwrap();
    let exp_id_str = exp_id.to_string();
    let (control_id, variant_id) = variant_ids(&exp);

    // Control converts at 0%, variant converts at 100%.
    for v in visitors_for(
        exp_id,
        Uuid::from_str(&control_id).unwrap(),
        Uuid::from_str(&variant_id).unwrap(),
        false,
        5,
    ) {
        post_experiment_event(&app, &slug, v, &exp_id_str, &control_id, false).await;
    }
    for v in visitors_for(
        exp_id,
        Uuid::from_str(&control_id).unwrap(),
        Uuid::from_str(&variant_id).unwrap(),
        true,
        5,
    ) {
        post_experiment_event(&app, &slug, v, &exp_id_str, &variant_id, true).await;
    }

    // Run the decision rules: clear winner -> promote.
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/experiments/{exp_id_str}/decide"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let outcome = body_json(resp).await;
    assert_eq!(outcome["decision"], "winner");
    assert_eq!(outcome["winner_variant_id"], json!(variant_id));
    assert!(outcome["effect_size"].as_f64().unwrap() > 0.5);
    assert!(outcome["confidence"].as_f64().unwrap() > 0.95);

    // The article now serves the winner as canonical content, no longer as an
    // experiment overlay.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/articles/{slug}"),
            Some("opv=whatever"),
            None,
            None,
        ),
    )
    .await;
    let article = body_json(resp).await;
    let rb = &article["rendered_blocks"][0];
    assert!(rb["html"].as_str().unwrap().contains("New headline"));
    assert!(rb["experiment_id"].is_null());

    // The document itself reflects the promoted version.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    let doc = body_json(resp).await;
    assert_eq!(
        doc["blocks"][0]["content"],
        json!({ "text": "New headline" })
    );

    // Decision recorded, experiment concluded.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/experiments"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    let list = body_json(resp).await;
    let e = &list.as_array().unwrap()[0];
    assert_eq!(e["status"], "decided");
    assert_eq!(e["decision"], "winner");
    assert_eq!(e["winning_variant_id"], json!(variant_id));
    let decisions = e["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["decision"], "winner");
}

#[tokio::test]
async fn experiment_concludes_no_winner() {
    let app = test_app().await;
    let (cookie, csrf, id, slug, block_ids) = seed_published_article(&app).await;
    let exp = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 2).await;
    let exp_id = Uuid::from_str(exp["id"].as_str().unwrap()).unwrap();
    let exp_id_str = exp_id.to_string();
    let (control_id, variant_id) = variant_ids(&exp);

    // Control converts at 100%, variant at 0%: variant is clearly worse.
    for v in visitors_for(
        exp_id,
        Uuid::from_str(&control_id).unwrap(),
        Uuid::from_str(&variant_id).unwrap(),
        false,
        5,
    ) {
        post_experiment_event(&app, &slug, v, &exp_id_str, &control_id, true).await;
    }
    for v in visitors_for(
        exp_id,
        Uuid::from_str(&control_id).unwrap(),
        Uuid::from_str(&variant_id).unwrap(),
        true,
        5,
    ) {
        post_experiment_event(&app, &slug, v, &exp_id_str, &variant_id, false).await;
    }

    let (_, resp) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/experiments/{exp_id_str}/decide"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    let outcome = body_json(resp).await;
    assert_eq!(outcome["decision"], "no_improvement");
    assert!(outcome["winner_variant_id"].is_null());
    assert!(outcome["promoted_version_id"].is_null());

    // No promotion: article keeps control content and no experiment attributes.
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/articles/{slug}"),
            Some("opv=whatever"),
            None,
            None,
        ),
    )
    .await;
    let article = body_json(resp).await;
    let rb = &article["rendered_blocks"][0];
    assert!(rb["html"].as_str().unwrap().contains("<h1>Headline</h1>"));
    assert!(rb["experiment_id"].is_null());
}

#[tokio::test]
async fn experiment_manual_stop_and_manual_promote() {
    let app = test_app().await;
    let (cookie, csrf, id, slug, block_ids) = seed_published_article(&app).await;
    let exp = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 100).await;
    let exp_id = exp["id"].as_str().unwrap().to_string();
    let (_, variant_id) = variant_ids(&exp);

    // Manual stop records a 'stopped' decision and rejects events.
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/experiments/{exp_id}/stop"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, resp) = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/documents/{id}/experiments"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(body_json(resp).await[0]["status"], "stopped");

    // Manual promote overrides the threshold and ships the best variant.
    let exp2 = seed_running_experiment(&app, &cookie, &csrf, &id, &block_ids[0], 100).await;
    let exp2_id = exp2["id"].as_str().unwrap().to_string();
    let exp2_uuid = Uuid::from_str(&exp2_id).unwrap();
    let (control2, variant2) = variant_ids(&exp2);
    for v in visitors_for(
        exp2_uuid,
        Uuid::from_str(&control2).unwrap(),
        Uuid::from_str(&variant2).unwrap(),
        true,
        2,
    ) {
        post_experiment_event(&app, &slug, v, &exp2_id, &variant2, true).await;
    }
    let (status, resp) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/experiments/{exp2_id}/promote"),
            Some(&cookie),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let outcome = body_json(resp).await;
    assert_eq!(outcome["decision"], "winner");
    assert_eq!(outcome["winner_variant_id"], json!(variant2));
    let _ = variant_id;
}
