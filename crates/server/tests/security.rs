//! Layered security regression suite — Sprint 1 of `old_docs/security_testing.md`.
//!
//! Complements the existing `api.rs`/`pages.rs`/`system.rs` tests by pinning
//! *security invariants* rather than individual HTTP flows:
//!
//! - **Authorization**: an anonymous-vs-owner matrix over the guarded endpoints
//!   (401 for the JSON API, `/login` redirects for the pages, 401 for
//!   `/admin/media`).
//! - **CSRF**: mutating endpoints reject absent/forged tokens (403) even with a
//!   valid session, and accept the genuine token (or the form-field variant).
//! - **Sessions**: cookie attributes, Secure-flag gating on TLS, that only the
//!   SHA-256 of the token is persisted, logout invalidation, and rejection of
//!   forged/substituted tokens.
//! - **Uploads**: the image pipeline sniffs bytes (declared type and client
//!   filename are advisory, never trusted), files land as `<uuid>.<sniffed_ext>`,
//!   and adversarial `/media/{name}` URLs are 404.
//!
//! Rate-limit throttling, XFF bypass, setup-race atomicity and Host-header
//! policy are already covered in `api.rs`/`system.rs` and are not repeated.
//! Property tests for the rate limiter, Markdown renderer, slugifier and zip
//! import live in their crates (Sprint 3).

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use forgepost_analytics::RateLimiter;
use forgepost_application::ports::SessionRepo;
use forgepost_server::app_with_media;
use forgepost_server::app_with_security;
use forgepost_server::routes::ClientIpConfig;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

mod common;
use common::*;

// ---------------------------------------------------------------------------
// Authorization matrix
// ---------------------------------------------------------------------------

/// The guarded JSON API endpoints. Anonymous: 401.
#[tokio::test]
async fn anonymous_api_matrix_rejected() {
    let app = security_app().await;
    let id = Uuid::new_v4().to_string();

    let mut cases: Vec<(Method, String, Option<serde_json::Value>)> = vec![
        (Method::GET, "/api/me".into(), None),
        (Method::GET, "/api/documents".into(), None),
        (
            Method::POST,
            "/api/documents".into(),
            Some(json!({ "title": "x", "markdown": "y" })),
        ),
        (
            Method::PUT,
            format!("/api/documents/{id}"),
            Some(json!({ "title": "x", "markdown": "y" })),
        ),
        (Method::POST, format!("/api/documents/{id}/publish"), None),
        (
            Method::POST,
            "/api/render".into(),
            Some(json!({ "markdown": "hi" })),
        ),
        (Method::GET, "/api/comments/pending".into(), None),
        (Method::POST, format!("/api/comments/{id}/approve"), None),
        (
            Method::POST,
            "/api/experiments".into(),
            Some(json!({ "name": "e", "config": {} })),
        ),
        (Method::POST, format!("/api/experiments/{id}/start"), None),
        (Method::POST, format!("/api/experiments/{id}/stop"), None),
        (Method::POST, format!("/api/experiments/{id}/decide"), None),
        (Method::POST, format!("/api/experiments/{id}/promote"), None),
        (
            Method::POST,
            format!("/api/experiments/{id}/no-winner"),
            None,
        ),
    ];

    for (method, uri, body) in cases.drain(..) {
        let (status, _) = send(&app, json_req(method.clone(), &uri, None, None, body)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "anonymous must not reach {method} {uri}"
        );
    }
}

/// The owner-gated server-rendered pages. Anonymous: redirect to `/login`;
/// `/admin/media` (JSON-shaped) is a 401.
#[tokio::test]
async fn anonymous_pages_redirect_to_login() {
    let app = security_app().await;
    let c = Uuid::new_v4().to_string();

    let cases: Vec<(String, &str)> = vec![
        ("/admin".into(), "GET"),
        ("/admin/new".into(), "POST"),
        ("/admin/editor/".to_string() + &c + "/publish", "POST"),
        ("/admin/editor/".to_string() + &c + "/delete", "POST"),
        ("/admin/experiments/".to_string() + &c + "/start", "POST"),
        ("/admin/comments/".to_string() + &c + "/approve", "POST"),
        ("/admin/settings".into(), "POST"),
        ("/admin/import".into(), "POST"),
    ];
    // `/admin/stats/{id}/experiments` needs a `block_id` to mount its form
    // extractor before the auth guard; it is covered by its JSON API twins in
    // `anonymous_api_matrix_rejected`.

    for (uri, method) in cases {
        let request = if method == "GET" {
            req(Method::GET, uri.as_str(), None, None)
        } else {
            // Extractors run before the handler: give each POST a body its
            // extractor can parse so the auth guard is what rejects the
            // anonymous caller.
            let (content_type, payload): (&str, &[u8]) = if uri.starts_with("/admin/import") {
                // Multipart with an empty `data` part.
                (
                    "multipart/form-data; boundary=x",
                    b"--x\r\nContent-Disposition: form-data; name=\"data\"\r\n\r\n\r\n--x--\r\n"
                        .as_slice(),
                )
            } else {
                ("application/x-www-form-urlencoded", b"name=Site".as_slice())
            };
            Request::builder()
                .method(Method::POST)
                .uri(uri.as_str())
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(payload.to_vec()))
                .unwrap()
        };
        let (status, resp) = send(&app, request).await;
        assert_eq!(
            status,
            StatusCode::SEE_OTHER,
            "anonymous {method} {uri} must redirect, got {status}"
        );
        let location = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let is_login = location
            .as_deref()
            .map(|l| l.starts_with("/login"))
            .unwrap_or(false);
        assert!(is_login, "redirect target for {uri} was {location:?}");
    }

    // The media endpoint is JSON-shaped even though it lives under /admin: a
    // well-formed multipart body with no session is still a 401.
    let body = b"--x\r\nContent-Disposition: form-data; name=\"data\"; filename=\"a.png\"\r\n\r\nx\r\n--x--\r\n";
    let request = Request::builder()
        .method(Method::POST)
        .uri("/admin/media")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=x")
        .body(Body::from(body.to_vec()))
        .unwrap();
    let (status, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The owner's session unlocks the guarded surface; the CSRF gate is applied on
/// top for mutating routes (verified in the CSRF section).
#[tokio::test]
async fn owner_session_unlocks_guarded_surface() {
    let app = security_app().await;
    let (cookie, _csrf, id, _slug) = seed_published_article(&app).await;

    let (status, _) = send(&app, req(Method::GET, "/admin", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, req(Method::GET, "/api/me", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        req(Method::GET, "/api/documents", Some(&cookie), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Mutating without a CSRF token is a distinct 403 (not auth-related).
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            &format!("/api/documents/{id}/publish"),
            Some(&cookie),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// CSRF enforcement
// ---------------------------------------------------------------------------

/// For each mutating route: no token → 403, wrong token → 403, correct token →
/// the handler's own outcome (anything but 403 proves the token passed).
#[tokio::test]
async fn mutating_api_routes_enforce_csrf() {
    let app = security_app().await;
    let (cookie, csrf, id, _slug) = seed_published_article(&app).await;

    let cases: Vec<(String, Option<serde_json::Value>)> = vec![
        (
            "POST /api/documents".into(),
            Some(json!({ "title": "CSRF", "markdown": "new doc" })),
        ),
        (
            format!("PUT /api/documents/{id}"),
            Some(json!({ "title": "Renamed", "markdown": "edited" })),
        ),
        (format!("POST /api/documents/{id}/publish"), None),
        (format!("POST /api/experiments/{id}/start"), None),
    ];

    for (method_uri, body) in cases {
        let (method, uri) = method_uri.split_once(' ').expect("method and uri");
        let method = Method::from_bytes(method.as_bytes()).unwrap();
        let build =
            |csrf: Option<&str>| json_req(method.clone(), uri, Some(&cookie), csrf, body.clone());

        let (missing, _) = send(&app, build(None)).await;
        assert_eq!(
            missing,
            StatusCode::FORBIDDEN,
            "{method} {uri} must require the CSRF token"
        );
        let (wrong, _) = send(&app, build(Some("deadbeef-wrong"))).await;
        assert_eq!(
            wrong,
            StatusCode::FORBIDDEN,
            "{method} {uri} must reject a wrong CSRF token"
        );
        let (valid, _) = send(&app, build(Some(&csrf))).await;
        assert_ne!(
            valid,
            StatusCode::FORBIDDEN,
            "{method} {uri} rejected a genuine CSRF token"
        );
    }
}

/// Server-rendered forms accept the token as a hidden `csrf_token` field and
/// still reject a missing/mismatched one.
#[tokio::test]
async fn page_forms_accept_token_field_and_reject_forgery() {
    let app = security_app().await;
    let (cookie, csrf, _id, _slug) = seed_published_article(&app).await;

    // `/admin/new` only parses the `csrf_token` field (via `CsrfForm`), so the
    // form-path token check is exercised without other validation noise.
    let with_field = |token: Option<&str>| {
        let fields: Vec<(&str, &str)> = token.map(|t| vec![("csrf_token", t)]).unwrap_or_default();
        form_req(cookie.as_str(), fields.as_slice())
    };
    let (no_token, _) = send(&app, with_field(None)).await;
    assert_eq!(
        no_token,
        StatusCode::FORBIDDEN,
        "form without token rejected"
    );
    let (wrong, _) = send(&app, with_field(Some("bad-token"))).await;
    assert_eq!(
        wrong,
        StatusCode::FORBIDDEN,
        "form with wrong token rejected"
    );
    let (valid, _) = send(&app, with_field(Some(&csrf))).await;
    assert!(
        valid.is_redirection(),
        "valid form token accepted ({valid}); expect a redirect"
    );
}

fn form_req(cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    Request::builder()
        .method(Method::POST)
        .uri("/admin/new")
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

fn url_encode(s: &str) -> String {
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

// ---------------------------------------------------------------------------
// Session lifecycle
// ---------------------------------------------------------------------------

/// Login sets a cookie with hostile-proof attributes, minus `Secure` on plain
/// HTTP.
#[tokio::test]
async fn session_cookie_carries_hardening_attributes() {
    let app = security_app().await;
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
    let raw = set_cookie(&resp);
    assert!(raw.contains("HttpOnly"), "cookie must be HttpOnly");
    assert!(raw.contains("SameSite=Lax"), "cookie must be SameSite=Lax");
    assert!(raw.contains("Path=/"), "cookie must be Path=/");
    assert!(
        raw.contains("Max-Age=2592000"),
        "cookie must expire after 30 days, got: {raw}"
    );
    assert!(
        !raw.contains("Secure"),
        "no Secure flag on plain HTTP: {raw}"
    );
    assert!(
        !raw.contains("SameSite=Strict") && !raw.contains("SameSite=None"),
        "SameSite must be Lax: {raw}"
    );
}

/// When the server is configured for TLS, the cookie carries `Secure`, and the
/// clear-cookie on logout does too.
#[tokio::test]
async fn secure_flag_is_set_when_tls_configured() {
    let pool = pool().await;
    let repo = common::migrated_repo(pool).await;
    let app = app_with_security(
        repo,
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_LOGIN_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_COMMENT_MAX),
        Arc::new(ClientIpConfig::default()),
        None,
        true,
        None,
    );
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
    let raw = set_cookie(&resp);
    assert!(raw.contains("; Secure"), "cookie must be Secure over TLS");
    assert!(raw.contains("HttpOnly"));

    let cookie = session_cookie(&resp);
    let csrf = csrf_for(&app, &cookie).await;
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
    let cleared = set_cookie(&resp);
    assert!(cleared.contains("Max-Age=0"));
    assert!(cleared.contains("; Secure"), "clear cookie also Secure");
}

/// The database stores only `sha256(token)`: the raw cookie value held by the
/// browser never appears in the `sessions` table.
#[tokio::test]
async fn raw_session_token_is_never_persisted() {
    use forgepost_domain::security::sha256_hex;

    let pool = pool().await;
    let repo = common::migrated_repo(pool.clone()).await;
    let app = app_with_security(
        repo.clone(),
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_LOGIN_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_COMMENT_MAX),
        Arc::new(ClientIpConfig::default()),
        None,
        false,
        None,
    );
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
    let token = cookie
        .strip_prefix("forgepost_session=")
        .expect("setup cookie names the session token");

    let hashes: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(&pool)
        .await
        .expect("sessions table readable");
    assert_eq!(hashes.len(), 1, "exactly one session after setup");
    assert_ne!(&hashes[0], token, "raw token must never be persisted");
    assert_eq!(hashes[0], sha256_hex(token), "only the sha256 is stored");

    assert!(
        repo.session_by_token(token)
            .await
            .expect("repo works")
            .is_some(),
        "raw cookie resolves to the stored session via its hash"
    );
}

/// Logout deletes the session server-side: the old cookie is dead for both the
/// page surface and the JSON API.
#[tokio::test]
async fn logout_invalidates_the_session_everywhere() {
    let app = security_app().await;
    let (cookie, csrf, _id, _slug) = seed_published_article(&app).await;

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
    assert!(set_cookie(&resp).contains("Max-Age=0"));

    let (status, _) = send(&app, req(Method::GET, "/api/me", Some(&cookie), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "API session invalidated");
    let (status, _) = send(&app, req(Method::GET, "/admin", Some(&cookie), None)).await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "page session invalidated (redirect to login)"
    );
}

/// A forged or unissued cookie is rejected, including an attempt to use it as
/// if it were a valid session token.
#[tokio::test]
async fn forged_and_substituted_tokens_are_rejected() {
    let app = security_app().await;
    let (cookie, csrf, _id, _slug) = seed_published_article(&app).await;

    let forged = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let (status, _) = send(&app, req(Method::GET, "/api/me", Some(forged), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "forged cookie rejected");

    let unissued = "99999999-9999-4999-8999-999999999999";
    let (status, _) = send(&app, req(Method::GET, "/admin", Some(unissued), None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);

    // An unissued token cannot act on the owner's behalf, even with their CSRF
    // (the session never resolved, so it is knocked back at auth).
    let (status, _) = send(
        &app,
        json_req(
            Method::POST,
            "/api/logout",
            Some(unissued),
            Some(&csrf),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "unissued token cannot log out a session"
    );
    let _ = cookie;
}

// ---------------------------------------------------------------------------
// Upload hardening
// ---------------------------------------------------------------------------

#[allow(clippy::unused_async)]
async fn media_app(dir: &tempfile::TempDir) -> axum::Router {
    let pool = pool().await;
    let repo = common::migrated_repo(pool).await;
    app_with_media(
        repo,
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        false,
        dir.path().to_path_buf(),
    )
}

/// A PNG with a real 8-byte magic (tests never decode the image).
fn tiny_png() -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R']);
    png
}

/// Multipart request uploading `bytes` as `filename` with a declared
/// `content_type`, authenticated as `cookie` with `csrf`.
fn upload(
    cookie: &str,
    csrf: &str,
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> Request<Body> {
    let boundary = "sec-boundary";
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

/// The magic-byte sniff is authoritative: a mismatched declared content type
/// cannot smuggle an image in (or out).
#[tokio::test]
async fn uploaded_type_comes_from_magic_bytes_not_declared_type() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let (cookie, csrf) = setup_owner(&app).await;

    // A PNG declared as an .ico still stores + serves as image/png.
    let png = tiny_png();
    let (status, resp) = send(
        &app,
        upload(&cookie, &csrf, "favicon.ico", "image/x-icon", &png),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url = body_json(resp).await["url"].as_str().unwrap().to_string();
    let (status, resp) = send(&app, req(Method::GET, &url, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );

    // SVG bytes declaring image/png are rejected *because of the bytes* — the
    // declared header cannot launder a scriptable document into the store.
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec();
    let (status, resp) = send(&app, upload(&cookie, &csrf, "x.png", "image/png", &svg)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body_text(resp).await.contains("unsupported image type"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// Filename never reaches the filesystem: traversal-shaped names still yield a
/// `<uuid>.<ext>` file, and the extension comes from the sniff.
#[tokio::test]
async fn client_filename_is_ignored_and_storage_is_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let (cookie, csrf) = setup_owner(&app).await;

    for (filename, expect_ext) in [
        ("../../../../etc/cron.d/evil.png", "png"),
        ("/absolute/path.png", "png"),
        ("..\\..\\evil.png", "png"),
        ("payload.exe", "png"),
        ("x", "png"),
    ] {
        let (status, resp) = send(
            &app,
            upload(&cookie, &csrf, filename, "image/png", &tiny_png()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "upload of {filename:?}");
        let url = body_json(resp).await["url"].as_str().unwrap().to_string();
        let name = &url["/media/".len()..];
        let stem = &name[..name.len() - expect_ext.len() - 1];
        assert!(
            Uuid::parse_str(stem).is_ok(),
            "stored name must be <uuid>.{expect_ext}, got {name}"
        );
        assert!(name.ends_with(&format!(".{expect_ext}")), "got {name}");
    }

    // Disk contains exactly one file per upload, all uuid-shaped, and the
    // traversal-named client filename never appears.
    let disk: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(disk.len(), 5);
    for name in &disk {
        assert!(
            !name.contains("evil") && !name.contains("..") && !name.contains('/'),
            "disk name must not reflect the client filename: {name}"
        );
        let stem = name.strip_suffix(".png").unwrap();
        assert!(Uuid::parse_str(stem).is_ok(), "uuid-shaped: {name}");
    }
}

/// Served files get the sniffed (stored) content type plus hardening headers,
/// with `nosniff` so the browser cannot re-classify stored bytes.
#[tokio::test]
async fn served_media_has_sniffed_type_and_nosniff() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;
    let (cookie, csrf) = setup_owner(&app).await;

    let (status, resp) = send(
        &app,
        upload(&cookie, &csrf, "cat.gif", "image/png", b"GIF89a bytes"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url = body_json(resp).await["url"].as_str().unwrap().to_string();
    assert!(url.ends_with(".gif"), "extension from the sniff: {url}");

    let (status, resp) = send(&app, req(Method::GET, &url, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/gif"
    );
    assert_eq!(
        resp.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        "public, max-age=31536000, immutable"
    );
}

/// Adversarial encodings of traversal and control characters cannot escape the
/// media root: every one of them is a 404.
#[tokio::test]
async fn adversarial_media_names_are_404() {
    let dir = tempfile::tempdir().unwrap();
    let app = media_app(&dir).await;

    for name in [
        "..%2Fforgepost.db",
        "%2e%2e%2fetc%2fpasswd",
        "%2E%2E%2Fsecret.txt",
        "%5c..%5c..%5cforgepost.db",
        "a%2Fb.png",
        "..",
        ".",
        "..%00.png",
        "%00",
        "%2e%2e",
        "a..%2F..%2Fb.png",
        "x/..%2F..%2F..%2Fetc%2Fshadow",
    ] {
        let uri = format!("/media/{name}");
        let (status, _) = send(&app, req(Method::GET, &uri, None, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} must 404");
    }
}
