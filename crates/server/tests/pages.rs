//! Integration tests for the server-rendered pages (single binary). These
//! mirror the browser flows from `frontend/e2e`: first-run redirects, setup,
//! login/logout, the admin dashboard, the editor save/publish loop, the public
//! article page (with tracker + data attributes), comment moderation, static
//! assets, and the stats page experiment form.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use openpublish_server::app;
use openpublish_server::repository::SqliteRepository;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use tower::ServiceExt;

const CSRF_HEADER: &str = "x-csrf-token";

async fn test_app() -> Router {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("memory pool");
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    app(Arc::new(repo))
}

/// Percent-encode a single form field value (URL-encoded forms).
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build a URL-encoded form request. `fields` are (name, value).
fn form_req(
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    fields: &[(&str, &str)],
) -> Request<Body> {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    builder
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// A request with an optional cookie and CSRF header.
fn req(method: Method, uri: &str, cookie: Option<&str>, csrf: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if let Some(t) = csrf {
        builder = builder.header(CSRF_HEADER, t);
    }
    builder.body(Body::empty()).unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Response<Body>) {
    let resp = app.clone().oneshot(req).await.expect("router responds");
    (resp.status(), resp)
}

async fn body_text(resp: Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf-8 body")
}

async fn body_json(resp: Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("valid JSON")
}

/// Session cookie value from a `Set-Cookie` header (attributes stripped).
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

/// The `Location` header of a redirect response.
fn location(resp: &Response<Body>) -> String {
    resp.headers()
        .get(header::LOCATION)
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string()
}

/// Complete setup through the page and return the owner's session cookie.
async fn setup_owner(app: &Router) -> String {
    let (_, resp) = send(
        app,
        form_req(
            Method::POST,
            "/setup",
            None,
            &[
                ("email", "a@b.com"),
                ("display", "Alice"),
                ("password", "password123"),
                ("confirm", "password123"),
            ],
        ),
    )
    .await;
    session_cookie(&resp)
}

/// Session CSRF token for `cookie`.
async fn csrf_for(app: &Router, cookie: &str) -> String {
    let (_, resp) = send(app, req(Method::GET, "/api/me", Some(cookie), None)).await;
    body_json(resp).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string()
}

// ---------------------------------------------------------------------------
// First-run redirects and setup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_run_redirects_to_setup() {
    let app = test_app().await;

    let (status, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/setup");

    let (status, resp) = send(&app, req(Method::GET, "/login", None, None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/setup");

    let (status, resp) = send(&app, req(Method::GET, "/setup", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Create the owner account"));
    assert!(html.contains("id=\"email\""));
    assert!(html.contains("id=\"confirm\""));
}

#[tokio::test]
async fn setup_form_creates_owner_and_redirects_to_admin() {
    let app = test_app().await;

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/setup",
            None,
            &[
                ("email", "a@b.com"),
                ("display", "Alice"),
                ("password", "password123"),
                ("confirm", "password123"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/admin");
    let cookie = session_cookie(&resp);

    // Setup is now complete: /setup redirects to /login, / shows the home page.
    let (status, resp) = send(&app, req(Method::GET, "/setup", None, None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");

    let (status, resp) = send(&app, req(Method::GET, "/", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("No published posts yet."));
}

#[tokio::test]
async fn setup_form_validates_input() {
    let app = test_app().await;

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/setup",
            None,
            &[
                ("email", "not-an-email"),
                ("display", ""),
                ("password", "short"),
                ("confirm", "different"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Enter a valid email address."));
    assert!(!html.contains("Password must be at least 8 characters."));

    // Validation short-circuits on the first error; a valid email then
    // surfaces the password rules, and so on.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/setup",
            None,
            &[
                ("email", "a@b.co"),
                ("display", ""),
                ("password", "short"),
                ("confirm", "different"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Password must be at least 8 characters."));

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/setup",
            None,
            &[
                ("email", "a@b.co"),
                ("display", ""),
                ("password", "password123"),
                ("confirm", "different"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Passwords do not match."));

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/setup",
            None,
            &[
                ("email", "a@b.co"),
                ("display", " "),
                ("password", "password123"),
                ("confirm", "password123"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Enter a display name."));
}

// ---------------------------------------------------------------------------
// Login / logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_logout_roundtrip() {
    let app = test_app().await;
    let _ = setup_owner(&app).await;

    // Wrong password re-renders the form with an error.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/login",
            None,
            &[("email", "a@b.com"), ("password", "wrongpass")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body_text(resp).await.contains("invalid email or password"));

    // Correct password creates a session and redirects to /admin.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/login",
            None,
            &[("email", "a@b.com"), ("password", "password123")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/admin");
    let cookie = session_cookie(&resp);

    // /admin is reachable with the fresh session.
    let (status, _) = send(&app, req(Method::GET, "/admin", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);

    // Logout requires CSRF, then clears the session.
    let csrf = csrf_for(&app, &cookie).await;
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/logout",
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");

    let (status, _) = send(&app, req(Method::GET, "/admin", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "session invalidated");
}

// ---------------------------------------------------------------------------
// Admin + editor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_requires_login() {
    let app = test_app().await;
    let _ = setup_owner(&app).await;

    let (status, resp) = send(&app, req(Method::GET, "/admin", None, None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/login?flash=not_authorized"
    );
}

#[tokio::test]
async fn new_post_editor_save_and_publish() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/new",
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let editor_uri = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(editor_uri.starts_with("/admin/editor/"), "got {editor_uri}");

    // Editor shows the untitled draft.
    let (status, resp) = send(&app, req(Method::GET, &editor_uri, Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Untitled"));
    assert!(html.contains("draft"));

    // Save without CSRF is rejected.
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("title", "My First Post"),
                ("tags", "tech, blog"),
                ("markdown", "# Hello\n\nSome **body** text."),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "missing csrf rejected");

    // Save with CSRF redirects to the editor with a flash.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("title", "My First Post"),
                ("tags", "tech, blog"),
                ("markdown", "# Hello\n\nSome **body** text."),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), format!("{editor_uri}?flash=saved"));

    let (status, resp) = send(
        &app,
        req(
            Method::GET,
            &format!("{editor_uri}?flash=saved"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Saved"));
    assert!(html.contains("My First Post"));
    assert!(html.contains("# Hello"));
    assert!(html.contains("tech, blog"));

    // Publish.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            &format!("{editor_uri}/publish"),
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), format!("{editor_uri}?flash=published"));

    let (status, resp) = send(
        &app,
        req(
            Method::GET,
            &format!("{editor_uri}?flash=published"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Published"));
    assert!(html.contains("published"));

    // The post is on the home page.
    let (status, resp) = send(&app, req(Method::GET, "/", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("My First Post"));
}

// ---------------------------------------------------------------------------
// Public article page + tracker
// ---------------------------------------------------------------------------

/// Setup, create, and publish "Hello World". Returns the session cookie.
async fn seed_published(app: &Router) -> String {
    let cookie = setup_owner(app).await;
    let csrf = csrf_for(app, &cookie).await;
    let (_, resp) = send(
        app,
        form_req(
            Method::POST,
            "/admin/new",
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    let editor_uri = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let (_, resp) = send(
        app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("title", "Hello World"),
                ("tags", "tech"),
                (
                    "markdown",
                    "# Big Title\n\nFirst paragraph.\n\nSecond paragraph.",
                ),
            ],
        ),
    )
    .await;
    let _ = resp;
    let (_, resp) = send(
        app,
        form_req(
            Method::POST,
            &format!("{editor_uri}/publish"),
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    let _ = resp;
    cookie
}

#[tokio::test]
async fn article_page_renders_html_tracker_and_visitor_cookie() {
    let app = test_app().await;
    let _ = seed_published(&app).await;

    let (status, resp) = send(&app, req(Method::GET, "/articles/hello-world", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .expect("visitor cookie minted")
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.starts_with("opv="), "got {set_cookie}");

    let html = body_text(resp).await;
    assert!(html.contains("<h1>Big Title</h1>"));
    assert!(html.contains("First paragraph."));
    assert!(html.contains("data-block-id="));
    assert!(html.contains("/static/tracker.js"));
    assert!(html.contains("trackArticle"));
    assert!(html.contains("Comments"));
    assert!(html.contains("No comments yet."));

    // Repeated visits with the same visitor are stable.
    let (_, resp) = send(
        &app,
        req(
            Method::GET,
            "/articles/hello-world",
            Some(&set_cookie),
            None,
        ),
    )
    .await;
    assert!(body_text(resp).await.contains("Big Title"));
}

#[tokio::test]
async fn article_page_404_is_html() {
    let app = test_app().await;
    let _ = seed_published(&app).await;

    let (status, resp) = send(
        &app,
        req(Method::GET, "/articles/does-not-exist", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let html = body_text(resp).await;
    assert!(html.contains("Article not found"));
    assert!(html.contains("Back to home"));
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn comment_flow_from_public_to_approved() {
    let app = test_app().await;
    let cookie = seed_published(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    // Public comment form needs a name and body.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/articles/hello-world/comments",
            None,
            &[("author", "Reader"), ("body", "Nice post!")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/articles/hello-world?flash=comment_pending"
    );

    // Pending comment is not public yet.
    let (_, resp) = send(&app, req(Method::GET, "/articles/hello-world", None, None)).await;
    assert!(body_text(resp).await.contains("No comments yet."));

    // Find the pending comment id via the API, then approve from the page.
    let (_, resp) = send(
        &app,
        req(Method::GET, "/api/comments/pending", Some(&cookie), None),
    )
    .await;
    let pending = body_json(resp).await;
    let comment_id = pending.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Approval without auth redirects to login.
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &format!("/admin/comments/{comment_id}/approve"),
            None,
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // Approval without CSRF is rejected.
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &format!("/admin/comments/{comment_id}/approve"),
            Some(&cookie),
            &[],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "missing csrf rejected");

    // Approve.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            &format!("/admin/comments/{comment_id}/approve"),
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/admin?flash=comment_approved"
    );

    // Now public.
    let (_, resp) = send(&app, req(Method::GET, "/articles/hello-world", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("Reader"));
    assert!(html.contains("Nice post!"));
    assert!(!html.contains("No comments yet."));

    // Empty comment re-renders the article with an error.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/articles/hello-world/comments",
            None,
            &[("author", ""), ("body", "")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body_text(resp)
            .await
            .contains("Name and comment are required.")
    );
}

// ---------------------------------------------------------------------------
// Stats + experiments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stats_page_renders_and_creates_experiment() {
    let app = test_app().await;
    let cookie = seed_published(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    let (_, resp) = send(
        &app,
        req(Method::GET, "/api/documents", Some(&cookie), None),
    )
    .await;
    let docs = body_json(resp).await;
    let doc_id = docs.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, resp) = send(
        &app,
        req(
            Method::GET,
            &format!("/admin/stats/{doc_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Analytics"));
    assert!(html.contains("Views (estimated)"));
    assert!(html.contains("No experiments yet"));
    assert!(html.contains("Create experiment"));

    // Create an experiment through the page form.
    let (_, resp) = send(
        &app,
        req(
            Method::GET,
            &format!("/api/documents/{doc_id}"),
            Some(&cookie),
            None,
        ),
    )
    .await;
    let doc = body_json(resp).await;
    let block_id = doc["blocks"][0]["id"].as_str().unwrap().to_string();

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            &format!("/admin/stats/{doc_id}/experiments"),
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("block_id", &block_id),
                ("name", "Headline test"),
                ("traffic_weight", "100"),
                ("variant_1_content", "New headline"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location(&resp),
        format!("/admin/stats/{doc_id}?flash=experiment_created")
    );

    let (_, resp) = send(
        &app,
        req(Method::GET, &location(&resp), Some(&cookie), None),
    )
    .await;
    let html = body_text(resp).await;
    assert!(html.contains("Experiment created."));
    assert!(html.contains("Headline test"));
    assert!(html.contains("draft"));
    assert!(html.contains("Start experiment"));
}

// ---------------------------------------------------------------------------
// Static assets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn static_assets_are_served() {
    let app = test_app().await;
    let _ = setup_owner(&app).await;

    for (name, content_type) in [
        ("app.css", "text/css"),
        ("favicon.svg", "image/svg+xml"),
        ("tracker.js", "application/javascript"),
    ] {
        let (status, resp) = send(
            &app,
            req(Method::GET, &format!("/static/{name}"), None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            content_type
        );
        let body = body_text(resp).await;
        assert!(!body.is_empty());
    }

    let (status, _) = send(&app, req(Method::GET, "/static/missing.txt", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Settings (blog name + theme)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settings_page_requires_login() {
    let app = test_app().await;

    let (status, resp) = send(&app, req(Method::GET, "/admin/settings", None, None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/login?flash=not_authorized"
    );
}

#[tokio::test]
async fn settings_page_shows_current_values_and_default_theme() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;

    let (status, resp) = send(
        &app,
        req(Method::GET, "/admin/settings", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Blog name"));
    assert!(html.contains("Save settings"));
    assert!(html.contains("id=\"name\""));
    assert!(html.contains("id=\"theme\""));
    assert!(html.contains("System (auto)"));
    assert!(html.contains("data-theme=\"system\""));
    assert!(html.contains("value=\"OpenPublish\""));

    // Defaults also show up on the anonymous pages.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("<h1>OpenPublish</h1>"));
    assert!(html.contains("data-theme=\"system\""));
}

#[tokio::test]
async fn settings_form_updates_name_and_theme() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("name", "My Journal"),
                ("theme", "sepia"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/admin/settings?flash=settings_saved"
    );

    // The redirect target shows the flash and the saved values.
    let (status, resp) = send(
        &app,
        req(
            Method::GET,
            "/admin/settings?flash=settings_saved",
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Settings saved."));
    assert!(html.contains("value=\"My Journal\""));
    assert!(html.contains("<option value=\"sepia\" selected>Sepia</option>"));

    // The home page and RSS feed pick up the new name and theme.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("<h1>My Journal</h1>"));
    assert!(html.contains("data-theme=\"sepia\""));

    let (_, resp) = send(&app, req(Method::GET, "/rss", None, None)).await;
    let body = body_text(resp).await;
    assert!(body.contains("<title>My Journal</title>"));
}

#[tokio::test]
async fn settings_form_validates_input() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    // Empty name.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[("csrf_token", &csrf), ("name", "  "), ("theme", "dark")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Enter a blog name."));

    // Unknown theme.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[("csrf_token", &csrf), ("name", "Fine"), ("theme", "neon")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Unknown theme."));

    // Nothing was persisted.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("<h1>OpenPublish</h1>"));
    assert!(html.contains("data-theme=\"system\""));
}
