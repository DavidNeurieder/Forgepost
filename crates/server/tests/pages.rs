//! Integration tests for the server-rendered pages (single binary). These
//! mirror the browser flows from `frontend/e2e`: first-run redirects, setup,
//! login/logout, the admin dashboard, the editor save/publish loop, the public
//! article page (with tracker + data attributes), comment moderation, static
//! assets, and the stats page experiment form.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode, header};
use forgepost_server::analytics::RateLimiter;
use forgepost_server::app;
use forgepost_server::app_with_media;
use forgepost_server::repository::{SettingsRepo, SqliteRepository};
use http_body_util::BodyExt;
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

/// Like `test_app()` but with comments enabled (they default to disabled).
async fn test_app_with_comments() -> Router {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("memory pool");
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    repo.set_setting("comments.enabled", "1")
        .await
        .expect("enable comments");
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
                (
                    "markdown",
                    "# Hello\n\nSome **body** text.\n\n- **A** item\n- B item",
                ),
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
    assert!(html.contains("- **A** item")); // blocks -> markdown round-trips lists
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

#[tokio::test]
async fn home_card_uses_first_resolvable_image() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let (_, resp) = send(
        &app,
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
    let (_, _) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("title", "Images"),
                ("tags", ""),
                (
                    "markdown",
                    "![Broken](fastlane/x.png)\n\n![Badge](https://example.com/badge.png)",
                ),
            ],
        ),
    )
    .await;
    let (_, _) = send(
        &app,
        form_req(
            Method::POST,
            &format!("{editor_uri}/publish"),
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;

    // The first image is a bare-relative ref that cannot resolve; the card
    // must fall through to the absolute URL instead of emitting a broken thumb.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("post-card-thumb"));
    assert!(html.contains("<img src=\"https://example.com/badge.png\""));
    assert!(!html.contains("fastlane/x.png"));
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
                    "# Big Title\n\nFirst paragraph.\n\n- **Tech:** Rust\n- Keep it simple",
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
    let app = test_app_with_comments().await;
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
    // Lists render as real bullets with inline formatting.
    assert!(
        html.contains("<ul>\n<li><strong>Tech:</strong> Rust</li>\n<li>Keep it simple</li>\n</ul>")
    );
    assert!(html.contains("data-block-id="));
    assert!(html.contains("/static/tracker.js"));
    assert!(html.contains("trackArticle"));
    assert!(html.contains("Comments"));
    assert!(html.contains("No comments yet."));

    // SEO head: canonical, meta description, Open Graph/Twitter, JSON-LD,
    // and no `noindex` (it is replaced by the article head_meta block).
    assert!(!html.contains("<meta name=\"robots\" content=\"noindex\">"));
    assert!(html.contains("<meta name=\"description\" content=\"First paragraph.\">"));
    assert!(
        html.contains("<link rel=\"canonical\" href=\"http://localhost/articles/hello-world\">")
    );
    assert!(html.contains("<meta property=\"og:type\" content=\"article\">"));
    assert!(html.contains("<meta property=\"og:site_name\" content=\"Forgepost\">"));
    assert!(html.contains("<meta property=\"og:title\" content=\"Hello World\">"));
    assert!(
        html.contains(
            "<meta property=\"og:url\" content=\"http://localhost/articles/hello-world\">"
        )
    );
    assert!(html.contains("<meta name=\"twitter:card\" content=\"summary\">"));
    assert!(html.contains("application/ld+json"));
    assert!(html.contains("\"@type\": \"BlogPosting\""));
    assert!(html.contains("\"headline\": \"Hello World\""));
    assert!(html.contains("\"author\": { \"@type\": \"Person\", \"name\": \"Alice\" }"));
    assert!(html.contains("\"datePublished\": \""));

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
async fn video_block_renders_click_to_load_and_video_seo() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let editor_uri = create_draft(&app, &cookie, &csrf).await;
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("title", "Video Post"),
                ("tags", "video"),
                (
                    "markdown",
                    "Intro paragraph.\n\nhttps://www.youtube.com/watch?v=dQw4w9WgXcQ\n\nOutro.",
                ),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = send(
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

    // Editor round-trips the URL back into the markdown textarea.
    let (status, resp) = send(&app, req(Method::GET, &editor_uri, Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(
        html.contains("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
        "video block serializes back to its URL"
    );

    // Public article: click-to-load box, lazy thumbnail, no iframe on load,
    // and both SEO hooks (og:video + JSON-LD VideoObject).
    let (status, resp) = send(&app, req(Method::GET, "/articles/video-post", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("class=\"video-box\""));
    assert!(html.contains("data-video"));
    assert!(html.contains("data-src=\"https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ\""));
    assert!(html.contains("i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
    assert!(
        !html.contains("<iframe"),
        "no third-party iframe on page load"
    );
    assert!(html.contains("aria-label=\"Play video\""));

    // embed.js is loaded for the click-to-load behavior.
    assert!(html.contains("/static/embed.js"));

    // og:video + JSON-LD VideoObject for the article's first video.
    assert!(
        html.contains("<meta property=\"og:video\" content=\"https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ\">")
    );
    assert!(html.contains("<meta property=\"og:video:type\" content=\"text/html\">"));
    assert!(html.contains("\"@type\": \"VideoObject\""));
    assert!(html.contains("\"embedUrl\": \"https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ\""));
    assert!(
        html.contains("\"thumbnailUrl\": \"https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg\"")
    );

    // The preview API renders the same click-to-load markup.
    let (status, resp) = send(
        &app,
        Request::builder()
            .method(Method::POST)
            .uri("/api/render")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "markdown": "https://youtu.be/dQw4w9WgXcQ",
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let preview = body_json(resp).await["html"].as_str().unwrap().to_string();
    assert!(preview.contains("class=\"video-box\""));
    assert!(preview.contains("youtube-nocookie.com"));
}

#[tokio::test]
async fn article_without_video_has_no_video_seo() {
    let app = test_app().await;
    let _ = seed_published(&app).await;

    let (_, resp) = send(&app, req(Method::GET, "/articles/hello-world", None, None)).await;
    let html = body_text(resp).await;
    assert!(
        !html.contains("og:video"),
        "no og:video without a video block"
    );
    assert!(!html.contains("VideoObject"));
    assert!(!html.contains("/static/embed.js"));
    assert!(!html.contains("video-box"));
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
async fn comments_section_hidden_when_disabled() {
    let app = test_app().await;
    let _ = seed_published(&app).await;

    let (_, resp) = send(&app, req(Method::GET, "/articles/hello-world", None, None)).await;
    let html = body_text(resp).await;
    assert!(!html.contains("<h2>Comments</h2>"));
    assert!(!html.contains("No comments yet."));
    assert!(!html.contains("Post comment"));

    // Posting still works but just bounces the reader back with a notice.
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
        "/articles/hello-world?flash=comments_disabled"
    );
}

#[tokio::test]
async fn comment_flow_from_public_to_approved() {
    let app = test_app_with_comments().await;
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

#[tokio::test]
async fn stats_page_shows_shares_and_traffic_sources() {
    let app = test_app().await;
    let cookie = seed_published(&app).await;

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

    // One view coming from a search engine, one share.
    let session = "99999999-9999-9999-9999-999999999999";
    let view = Request::builder()
        .method(Method::POST)
        .uri("/api/events")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::REFERER, "https://www.google.com/search?q=forgepost")
        .body(Body::from(
            serde_json::json!({
                "slug": "hello-world",
                "session_id": session,
                "kind": "view",
                "payload": {},
            })
            .to_string(),
        ))
        .unwrap();
    let (status, _) = send(&app, view).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let share = Request::builder()
        .method(Method::POST)
        .uri("/api/events")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "slug": "hello-world",
                "session_id": session,
                "kind": "share_click",
                "payload": {},
            })
            .to_string(),
        ))
        .unwrap();
    let (status, _) = send(&app, share).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

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
    assert!(html.contains("Shares"));
    assert!(html.contains("Traffic sources"));
    assert!(html.contains(">1<"), "the single view is counted");
    assert!(html.contains("Search"), "google referrer buckets to Search");
    assert!(html.contains("100%"), "the share of traffic is shown");
}

#[tokio::test]
async fn dashboard_shows_week_callout_and_game_feel_columns() {
    let app = test_app().await;
    let cookie = seed_published(&app).await;

    // A view with a Referer so the post has measurable activity this week.
    let view = Request::builder()
        .method(Method::POST)
        .uri("/api/events")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::REFERER, "https://news.ycombinator.com/")
        .body(Body::from(
            serde_json::json!({
                "slug": "hello-world",
                "session_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "kind": "view",
                "payload": {},
            })
            .to_string(),
        ))
        .unwrap();
    let (status, _) = send(&app, view).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, resp) = send(&app, req(Method::GET, "/admin", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("This week"));
    assert!(html.contains("Most read this week"));
    assert!(html.contains("Hello World"));
    assert!(html.contains("Views (7d)"));
    assert!(html.contains("Δ vs last week"));
    assert!(html.contains("Reached end"));
    assert!(
        html.contains(r#"<td class="muted">new</td>"#),
        "a fresh post shows 'new' rather than a delta"
    );
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
        ("embed.js", "application/javascript"),
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
// Media uploads (M6)
// ---------------------------------------------------------------------------

/// Like `test_app()` but with an isolated media directory for uploads.
async fn media_app(dir: &tempfile::TempDir) -> Router {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("memory pool");
    let repo = SqliteRepository::from_pool(pool);
    repo.migrate().await.expect("migrations apply");
    app_with_media(
        Arc::new(repo),
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        false,
        dir.path().to_path_buf(),
    )
}

/// A PNG whose first 8 bytes are the real magic (enough for the sniffer; the
/// tests never decode the image).
fn tiny_png() -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R']);
    png
}

/// A multipart/form-data request with a single `data` file part.
fn multipart_req(
    cookie: &str,
    csrf: &str,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> Request<Body> {
    let boundary = "test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"data\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method(Method::POST)
        .uri("/admin/media")
        .header(header::COOKIE, cookie)
        .header(CSRF_HEADER, csrf)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn media_upload_requires_login_and_csrf() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let png = tiny_png();

    // No session: 401, nothing written.
    let (status, _resp) = send(&app, multipart_req("", "", "a.png", "image/png", &png)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let cookie = setup_owner(&app).await;

    // Authenticated but wrong CSRF: 403, nothing written.
    let (status, _) = send(
        &app,
        multipart_req(&cookie, "nope", "a.png", "image/png", &png),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn media_upload_and_serve_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let png = tiny_png();

    let (status, resp) = send(
        &app,
        multipart_req(&cookie, &csrf, "cat.png", "image/png", &png),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url = body_json(resp).await["url"].as_str().unwrap().to_string();
    assert!(url.starts_with("/media/"));
    assert!(url.ends_with(".png"));

    // Exactly one file, stored under the generated name (client filename never
    // used: the file on disk is not "cat.png").
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let disk_name = entries[0].as_ref().unwrap().file_name();
    assert_eq!(format!("/media/{}", disk_name.to_string_lossy()), url);

    // Serve: original bytes, sniffed content type, hardened headers, and it
    // works without a session cookie.
    let (status, resp) = send(&app, req(Method::GET, &url, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(
        resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "public, max-age=31536000, immutable"
    );
    let served = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(served.as_ref(), png.as_slice());
}

#[tokio::test]
async fn media_upload_rejects_svg_oversize_and_serves_unknown_as_404() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    // SVG is rejected outright (scriptable if served from the same origin).
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec();
    let (status, resp) = send(
        &app,
        multipart_req(&cookie, &csrf, "x.svg", "image/svg+xml", &svg),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let html = body_text(resp).await;
    assert!(html.contains("unsupported image type"));

    // Empty upload is rejected.
    let (status, _) = send(
        &app,
        multipart_req(&cookie, &csrf, "x.png", "image/png", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Oversize upload is rejected and nothing lands on disk.
    let mut big = tiny_png();
    big.extend(std::iter::repeat_n(0u8, 10 * 1024 * 1024 + 1));
    let (status, _) = send(
        &app,
        multipart_req(&cookie, &csrf, "big.png", "image/png", &big),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

    // Traversal-shaped names never reach the filesystem.
    let (status, _) = send(
        &app,
        req(Method::GET, "/media/..%2Fforgepost.db", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unknown (but well-formed) names are 404.
    let (status, _) = send(
        &app,
        req(
            Method::GET,
            "/media/00000000-0000-0000-0000-000000000000.png",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Markdown import (single .md or .zip with images)
// ---------------------------------------------------------------------------

/// A multipart/form-data request for `/admin/import` with one `data` part.
fn import_req(cookie: &str, csrf: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
    let boundary = "import-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"data\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method(Method::POST)
        .uri("/admin/import")
        .header(header::COOKIE, cookie)
        .header(CSRF_HEADER, csrf)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

/// Build an in-memory zip archive from `(path, bytes)` entries.
fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }
    buf
}

/// Follow an import's redirect to the editor page and return its HTML.
async fn editor_after_import(app: &Router, cookie: &str, resp: Response<Body>) -> String {
    let editor_uri = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(editor_uri.starts_with("/admin/editor/"), "got {editor_uri}");
    let (status, resp) = send(app, req(Method::GET, &editor_uri, Some(cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    body_text(resp).await
}

#[tokio::test]
async fn import_requires_login_and_csrf() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let md = b"# Hello";

    let (status, _resp) = send(&app, import_req("", "", "post.md", md)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let cookie = setup_owner(&app).await;
    let (status, _) = send(&app, import_req(&cookie, "wrong-token", "post.md", md)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn import_markdown_with_front_matter_creates_draft() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    let md = concat!(
        "---\n",
        "title: Imported Post\n",
        "tags: tech\n",
        "---\n",
        "\n",
        "# Heading\n",
        "\n",
        "Body text with a remote ![img](https://example.com/a.png).\n"
    );
    let (status, resp) = send(&app, import_req(&cookie, &csrf, "post.md", md.as_bytes())).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let html = editor_after_import(&app, &cookie, resp).await;
    assert!(html.contains("Imported Post"), "title from front matter");
    assert!(html.contains(r#"value="tech""#), "tags from front matter");
    assert!(html.contains("Body text with a remote"), "body imported");
    assert!(
        html.contains("![img](https://example.com/a.png)"),
        "remote image untouched"
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().count() == 0,
        "no media written"
    );
}

#[tokio::test]
async fn import_zip_uploads_images_and_rewrites_refs() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    let png = tiny_png();
    let zip = zip_bytes(&[
        (
            "post.md",
            b"# Post\n\n![A](images/a.png)\n\n![B](https://x/b.png)\n",
        ),
        ("images/a.png", &png),
    ]);
    let (status, resp) = send(&app, import_req(&cookie, &csrf, "post.zip", &zip)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let html = editor_after_import(&app, &cookie, resp).await;

    // The bundled image is rewritten to the media store; the remote stays.
    assert!(!html.contains("images/a.png"), "local ref rewritten");
    assert!(html.contains("[A](/media/"), "image rewritten to /media/");
    assert!(html.contains("![B](https://x/b.png)"), "remote kept");
    assert!(html.contains("Post"), "title from filename stem");

    // Exactly one image landed on disk, and it is served back.
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let disk_name = entries[0].as_ref().unwrap().file_name();
    let url = format!("/media/{}", disk_name.to_string_lossy());
    let (status, resp) = send(&app, req(Method::GET, &url, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let served = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(served.as_ref(), png.as_slice());
}

#[tokio::test]
async fn import_rejects_bad_archives_and_local_refs_without_zip() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    // Plain .md with a local image reference: tell the user to zip it.
    let (status, _) = send(
        &app,
        import_req(&cookie, &csrf, "post.md", b"# X\n\n![a](img.png)\n"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Zip with no markdown.
    let zip = zip_bytes(&[("a.txt", b"nope")]);
    let (status, _) = send(&app, import_req(&cookie, &csrf, "a.zip", &zip)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Zip with multiple markdown files.
    let zip = zip_bytes(&[("a.md", b"a"), ("b.md", b"b")]);
    let (status, _) = send(&app, import_req(&cookie, &csrf, "a.zip", &zip)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Not a zip at all.
    let (status, _) = send(
        &app,
        import_req(&cookie, &csrf, "a.zip", b"this is not a zip"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Wrong file type.
    let (status, _) = send(&app, import_req(&cookie, &csrf, "a.txt", b"hello")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

// ---------------------------------------------------------------------------
// Deleting posts
// ---------------------------------------------------------------------------

/// POST /admin/new and return the `/admin/editor/{id}` redirect target.
async fn create_draft(app: &Router, cookie: &str, csrf: &str) -> String {
    let (status, resp) = send(
        app,
        form_req(
            Method::POST,
            "/admin/new",
            Some(cookie),
            &[("csrf_token", csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    resp.headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn delete_post_requires_login_and_csrf() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let editor_uri = create_draft(&app, &cookie, &csrf).await;
    let delete_uri = format!("{editor_uri}/delete");

    // No session → redirect to login.
    let (status, _) = send(
        &app,
        form_req(Method::POST, &delete_uri, None, &[("csrf_token", &csrf)]),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // Wrong CSRF → forbidden.
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &delete_uri,
            Some(&cookie),
            &[("csrf_token", "bad-token")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The draft is untouched by either attempt.
    let (status, _) = send(&app, req(Method::GET, &editor_uri, Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn owner_can_delete_published_post_and_url_goes_404() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let editor_uri = create_draft(&app, &cookie, &csrf).await;
    let delete_uri = format!("{editor_uri}/delete");

    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("title", "Doomed"),
                ("tags", "tech"),
                ("markdown", "# Doomed\n\nSome body."),
                ("csrf_token", &csrf),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = send(
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

    // The published article is live before the delete.
    let (status, _) = send(&app, req(Method::GET, "/articles/doomed", None, None)).await;
    assert_eq!(status, StatusCode::OK);

    // Delete it.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            &delete_uri,
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/admin?flash=deleted"
    );

    // Gone from the dashboard (flash confirms), article 404s, editor loses it.
    let (status, resp) = send(
        &app,
        req(Method::GET, "/admin?flash=deleted", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Post deleted."), "flash shown on dashboard");
    assert!(!html.contains("Doomed"), "post gone from dashboard list");

    let (status, _) = send(&app, req(Method::GET, "/articles/doomed", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(&app, req(Method::GET, &editor_uri, Some(&cookie), None)).await;
    assert_ne!(status, StatusCode::OK);

    // Deleting again reports the missing document.
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &delete_uri,
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleted_post_slug_is_reusable() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let editor_uri = create_draft(&app, &cookie, &csrf).await;

    // Take the "reusable" slug, then delete the post.
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(&cookie),
            &[
                ("title", "Reusable"),
                ("markdown", "# Reusable\n\nBody."),
                ("csrf_token", &csrf),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = send(
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
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &format!("{editor_uri}/delete"),
            Some(&cookie),
            &[("csrf_token", &csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // A new post with the same title saves without a slug collision.
    let editor_uri2 = create_draft(&app, &cookie, &csrf).await;
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            &editor_uri2,
            Some(&cookie),
            &[
                ("title", "Reusable"),
                ("markdown", "# Reusable\n\nBody."),
                ("csrf_token", &csrf),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, resp) = send(&app, req(Method::GET, &editor_uri2, Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body_text(resp).await.contains("Reusable"));
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
    assert!(html.contains("id=\"url\""));
    assert!(html.contains("id=\"tagline\""));
    assert!(html.contains("System (auto)"));
    assert!(html.contains("data-theme=\"system\""));
    assert!(html.contains("value=\"Forgepost\""));
    assert!(html.contains("canonical links, Open Graph, sitemap, robots, and RSS"));

    // Defaults also show up on the anonymous pages.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("class=\"brand\" href=\"/\">Forgepost</a>"));
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
                ("url", "https://journal.example.com"),
                ("tagline", "Notes on software."),
                ("image", "https://journal.example.com/og.png"),
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
    assert!(html.contains("value=\"https://journal.example.com\""));
    assert!(html.contains("value=\"Notes on software.\""));
    assert!(html.contains("value=\"https://journal.example.com/og.png\""));

    // The home page and RSS feed pick up the new name, theme, URL, and tagline.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("class=\"brand\" href=\"/\">My Journal</a>"));
    assert!(html.contains("data-theme=\"sepia\""));
    assert!(html.contains("<meta name=\"description\" content=\"Notes on software.\">"));
    assert!(html.contains("<link rel=\"canonical\" href=\"https://journal.example.com\">"));
    assert!(html.contains("<meta property=\"og:site_name\" content=\"My Journal\">"));
    assert!(
        html.contains(
            "<meta property=\"og:image\" content=\"https://journal.example.com/og.png\">"
        )
    );
    assert!(html.contains("<meta property=\"og:image:width\" content=\"1200\">"));
    assert!(html.contains("<meta property=\"og:image:height\" content=\"630\">"));
    assert!(html.contains("<meta name=\"twitter:card\" content=\"summary_large_image\">"));

    let (_, resp) = send(&app, req(Method::GET, "/rss", None, None)).await;
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/rss+xml; charset=utf-8"
    );
    let body = body_text(resp).await;
    assert!(body.contains("<title>My Journal</title>"));
    assert!(body.contains("<link>https://journal.example.com</link>"));
    assert!(body.contains("<description>Notes on software.</description>"));

    // The configured URL also drives robots.txt and the sitemap.
    let (_, resp) = send(&app, req(Method::GET, "/robots.txt", None, None)).await;
    assert!(
        body_text(resp)
            .await
            .contains("Sitemap: https://journal.example.com/sitemap.xml")
    );
}

#[tokio::test]
async fn default_image_relative_path_is_absolutized() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;
    let (status, _) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("name", "Forgepost"),
                ("theme", "system"),
                ("url", "https://example.com"),
                ("image", "/media/og.png"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(
        html.contains("<meta property=\"og:image\" content=\"https://example.com/media/og.png\">")
    );
    assert!(html.contains("<meta property=\"og:image:width\" content=\"1200\">"));
    assert!(html.contains("<meta name=\"twitter:card\" content=\"summary_large_image\">"));
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

    // Malformed URL.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("name", "Fine"),
                ("theme", "dark"),
                ("url", "not-a-url"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Site URL must start with http:// or https://."));

    // Default image must be an uploaded /media/… path or an absolute URL.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("name", "Fine"),
                ("theme", "dark"),
                ("url", "https://ok.example.com"),
                ("image", "not-a-url"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains(
        "Default image must be an uploaded /media/… URL or a full http:// or https:// URL."
    ));

    // Oversized tagline.
    let (status, resp) = send(
        &app,
        form_req(
            Method::POST,
            "/admin/settings",
            Some(&cookie),
            &[
                ("csrf_token", &csrf),
                ("name", "Fine"),
                ("theme", "dark"),
                ("url", "https://ok.example.com"),
                ("tagline", &"x".repeat(201)),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(html.contains("Tagline is too long (200 characters max)."));

    // Nothing was persisted.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("class=\"brand\" href=\"/\">Forgepost</a>"));
    assert!(html.contains("data-theme=\"system\""));
}

#[tokio::test]
async fn tag_pages_list_published_posts_only() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    async fn publish(app: &Router, cookie: &str, csrf: &str, title: &str, tags: &str) {
        let editor_uri = create_draft(app, cookie, csrf).await;
        let (status, _) = send(
            app,
            form_req(
                Method::POST,
                &editor_uri,
                Some(cookie),
                &[
                    ("csrf_token", csrf),
                    ("title", title),
                    ("tags", tags),
                    ("markdown", &format!("# {title}\n\nSome body text.")),
                ],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (status, _) = send(
            app,
            form_req(
                Method::POST,
                &format!("{editor_uri}/publish"),
                Some(cookie),
                &[("csrf_token", csrf)],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
    }

    publish(&app, &cookie, &csrf, "Rust Post", "rust, tech").await;
    publish(&app, &cookie, &csrf, "Other Post", "cooking").await;

    // The tag page lists only the matching post.
    let (_, resp) = send(&app, req(Method::GET, "/tags/rust", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("<h1>Tag: rust</h1>"));
    assert!(html.contains("Rust Post"));
    assert!(!html.contains("Other Post"));

    // Tag matching is case-insensitive (tags are stored lowercase).
    let (status, _) = send(&app, req(Method::GET, "/tags/RUST", None, None)).await;
    assert_eq!(status, StatusCode::OK);

    // Unknown tag -> 404.
    let (status, _) = send(&app, req(Method::GET, "/tags/nope", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Home links to tag pages, not search.
    let (_, resp) = send(&app, req(Method::GET, "/", None, None)).await;
    let html = body_text(resp).await;
    assert!(html.contains("href=\"/tags/rust\""));
    assert!(!html.contains("href=\"/search?q="));
}

// ---------------------------------------------------------------------------
// Keep reading recommendations
// ---------------------------------------------------------------------------

async fn publish_with(app: &Router, cookie: &str, csrf: &str, title: &str, tags: &str) -> String {
    let editor_uri = create_draft(app, cookie, csrf).await;
    let (status, _) = send(
        app,
        form_req(
            Method::POST,
            &editor_uri,
            Some(cookie),
            &[
                ("csrf_token", csrf),
                ("title", title),
                ("tags", tags),
                ("markdown", &format!("# {title}\n\nSome body text.")),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = send(
        app,
        form_req(
            Method::POST,
            &format!("{editor_uri}/publish"),
            Some(cookie),
            &[("csrf_token", csrf)],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    editor_uri
}

#[tokio::test]
async fn article_page_recommends_related_posts() {
    let app = test_app().await;
    let cookie = setup_owner(&app).await;
    let csrf = csrf_for(&app, &cookie).await;

    // Two tech posts and two food posts. Reading a tech post must rank the
    // other tech post first (shared tag), then backfill with the newest.
    publish_with(&app, &cookie, &csrf, "Tech One", "tech").await;
    publish_with(&app, &cookie, &csrf, "Tech Two", "tech").await;
    publish_with(&app, &cookie, &csrf, "Food One", "food").await;
    publish_with(&app, &cookie, &csrf, "Food Two", "food").await;

    let (status, resp) = send(&app, req(Method::GET, "/articles/tech-one", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;

    assert!(html.contains("Keep reading"), "related section present");
    let tech_two = html.find("Tech Two").expect("tag match ranked first");
    let food_one = html.find("Food One").expect("backfill listed");
    let food_two = html.find("Food Two").expect("backfill listed");
    assert!(
        tech_two < food_one,
        "shared-tag match comes before backfill"
    );
    assert!(
        tech_two < food_two,
        "shared-tag match comes before backfill"
    );

    // Cards carry the tracking attribute; the current post is never recommended.
    assert!(html.contains("data-recommended-slug=\"tech-two\""));
    assert!(html.contains("data-recommended-slug=\"food-one\""));
    assert!(!html.contains("href=\"/articles/tech-one\""));
    assert!(!html.contains("data-recommended-slug=\"tech-one\""));
}

#[tokio::test]
async fn article_page_without_other_posts_has_no_keep_reading() {
    let app = test_app().await;
    let _ = seed_published(&app).await;

    let (status, resp) = send(&app, req(Method::GET, "/articles/hello-world", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let html = body_text(resp).await;
    assert!(!html.contains("Keep reading"));
    assert!(!html.contains("data-recommended-slug"));
}
