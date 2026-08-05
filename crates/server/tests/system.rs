//! Full system tests.
//!
//! Unlike `api.rs` (which drives the router in-process), these tests spawn the
//! real `openpublish` binary over a real TCP socket against a real on-disk
//! SQLite database and walk through the entire creator journey: first-run
//! setup, writing and publishing, reading the blog externally, RSS, analytics,
//! comment moderation, block experiments (assign / measure / promote), logout,
//! and backup via `openpublish export`.
//!
//! Each test picks a free port and a throwaway temp directory, so tests run in
//! parallel with the rest of the workspace without interference.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::Method;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, COOKIE};
use serde_json::{Value, json};

const PASSWORD: &str = "correct horse battery staple";
const CSRF_HEADER: &str = "x-csrf-token";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Grab a free TCP port by binding, then releasing, an ephemeral socket.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A running `openpublish serve` process plus its scratch database.
struct Server {
    child: Child,
    _tmp: tempfile::TempDir,
    db_path: PathBuf,
    base: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_ready(base: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last_err = String::new();
    while Instant::now() < deadline {
        match Client::new().get(format!("{base}/health")).send() {
            Ok(resp) if resp.status().is_success() => return,
            Ok(resp) => last_err = format!("health returned {}", resp.status()),
            Err(err) => last_err = err.to_string(),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("server did not become ready within 60s: {last_err}");
}

fn start_server() -> Server {
    let tmp = tempfile::tempdir().expect("temp dir");
    let db_path = tmp.path().join("system.db");
    let db_url = format!("sqlite://{}", db_path.display());
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_openpublish"))
        .args([
            "serve",
            "--database-url",
            &db_url,
            "--addr",
            &format!("127.0.0.1:{port}"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn openpublish serve");
    let server = Server {
        child,
        _tmp: tmp,
        db_path,
        base,
    };
    wait_ready(&server.base);
    server
}

/// Authenticated creator session: keeps the session cookie and CSRF token.
struct Creator {
    http: Client,
    base: String,
    csrf: Option<String>,
}

impl Creator {
    fn new(base: &str) -> Self {
        Self {
            http: Client::builder()
                .cookie_store(true)
                .build()
                .expect("client"),
            base: base.to_string(),
            csrf: None,
        }
    }

    /// `body` may be `None`; a 2xx with an empty body yields `Value::Null`.
    fn json(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(csrf) = &self.csrf {
            req = req.header(CSRF_HEADER, csrf);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().expect("request succeeds");
        let status = resp.status().as_u16();
        let text = resp.text().expect("response body");
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).expect("response is JSON")
        };
        (status, value)
    }

    fn get(&self, path: &str) -> (u16, Value) {
        self.json(Method::GET, path, None)
    }

    fn post(&self, path: &str, body: Option<Value>) -> (u16, Value) {
        self.json(Method::POST, path, body)
    }

    fn put(&self, path: &str, body: Value) -> (u16, Value) {
        self.json(Method::PUT, path, Some(body))
    }

    fn setup(&mut self, email: &str, display_name: &str) {
        let (status, body) = self.json(
            Method::POST,
            "/api/setup",
            Some(json!({
                "email": email,
                "password": PASSWORD,
                "display_name": display_name,
            })),
        );
        assert_eq!(status, 200, "first-run setup succeeds");
        self.csrf = body["csrf_token"].as_str().map(str::to_string);
        assert!(self.csrf.is_some(), "setup returns a CSRF token");
    }

    fn login(&mut self, email: &str) {
        let (status, body) = self.json(
            Method::POST,
            "/api/login",
            Some(json!({ "email": email, "password": PASSWORD })),
        );
        assert_eq!(status, 200, "login succeeds");
        self.csrf = body["csrf_token"].as_str().map(str::to_string);
    }

    fn me(&self) -> (u16, Value) {
        self.get("/api/me")
    }

    fn logout(&self) -> u16 {
        self.post("/api/logout", None).0
    }
}

/// Unauthenticated reader: one anonymous `opv` visitor identity.
struct Visitor {
    http: Client,
    base: String,
    opv: String,
}

impl Visitor {
    fn new(base: &str, opv: &str) -> Self {
        Self {
            http: Client::builder()
                .cookie_store(false)
                .build()
                .expect("client"),
            base: base.to_string(),
            opv: opv.to_string(),
        }
    }

    /// Record one analytics event as this visitor. `experiment` carries
    /// `experiment_id` / `variant_id` for experiment events.
    fn event(
        &self,
        slug: &str,
        session_id: &str,
        kind: &str,
        block_id: Option<&str>,
        experiment: Option<&Value>,
        payload: Value,
    ) {
        let mut body = json!({
            "slug": slug,
            "session_id": session_id,
            "kind": kind,
            "payload": payload,
        });
        if let Some(bid) = block_id {
            body["block_id"] = json!(bid);
        }
        if let Some(exp) = experiment {
            body["experiment_id"] = exp["experiment_id"].clone();
            body["variant_id"] = exp["variant_id"].clone();
        }
        let status = self
            .http
            .post(format!("{}/api/events", self.base))
            .header(COOKIE, format!("opv={}", self.opv))
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .expect("event accepted")
            .status()
            .as_u16();
        assert_eq!(status, 204, "event {} accepted", kind);
    }

    /// Fetch the public article as this visitor.
    fn article(&self, slug: &str) -> Value {
        self.http
            .get(format!("{}/api/articles/{}", self.base, slug))
            .header(COOKIE, format!("opv={}", self.opv))
            .send()
            .expect("article fetch")
            .json::<Value>()
            .expect("article JSON")
    }
}

/// Run `openpublish export` against a database file and return the parsed JSON.
fn export_database(db_path: &Path) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_openpublish"))
        .args([
            "export",
            "--database-url",
            &format!("sqlite://{}", db_path.display()),
        ])
        .output()
        .expect("openpublish export runs");
    assert!(
        out.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("export is JSON")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The whole creator loop in one process, the way a solo operator + a public
/// reader would actually use it.
#[test]
fn creator_journey_end_to_end() {
    let server = start_server();
    let base = &server.base;

    // 1. The server is up and un-configured.
    let resp = reqwest::blocking::get(format!("{base}/health")).expect("GET /health");
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().expect("health is JSON");
    assert_eq!(body, json!({ "status": "ok" }));

    let resp = reqwest::blocking::get(format!("{base}/api/setup")).expect("GET /api/setup");
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().expect("setup status is JSON");
    assert_eq!(body, json!({ "setup_complete": false }));

    // 2. First-run setup creates the creator account and starts a session.
    let mut creator = Creator::new(base);
    creator.setup("alice@example.com", "Alice");
    let (status, me) = creator.me();
    assert_eq!(status, 200);
    assert_eq!(me["user"]["email"], "alice@example.com");
    assert_eq!(me["user"]["display_name"], "Alice");

    // 3. Write a post (Markdown -> block tree with stable ids).
    let (status, doc) = creator.post(
        "/api/documents",
        Some(json!({
            "title": "My First Post",
            "markdown": "# Headline\n\nFirst paragraph.\n\nSecond paragraph.",
            "tags": ["blog", "tech"],
        })),
    );
    assert_eq!(status, 200, "create document");
    let id = doc["id"].as_str().expect("document id").to_string();
    let slug = doc["slug"].as_str().expect("slug").to_string();
    assert_eq!(slug, "my-first-post");
    let blocks = doc["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 3);
    let heading_block = blocks[0]["id"].as_str().unwrap().to_string();

    // 4. Edit keeps the stable public URL.
    let (status, edited) = creator.put(
        &format!("/api/documents/{id}"),
        json!({ "title": "My First Post (edited)" }),
    );
    assert_eq!(status, 200);
    assert_eq!(edited["slug"], "my-first-post");

    // 5. Not public yet, then publish.
    let resp = reqwest::blocking::get(format!("{base}/api/articles/{slug}")).expect("draft fetch");
    assert_eq!(resp.status().as_u16(), 400, "draft is not public");

    let (status, published) = creator.post(&format!("/api/documents/{id}/publish"), None);
    assert_eq!(status, 200);
    assert_eq!(published["status"], "published");

    // 6. External readers can view the rendered post.
    let resp = reqwest::blocking::get(format!("{base}/api/articles/{slug}")).expect("article");
    assert_eq!(resp.status().as_u16(), 200);
    let article: Value = resp.json().expect("article JSON");
    assert_eq!(article["slug"], slug);
    assert!(
        article["html"]
            .as_str()
            .unwrap()
            .contains("<h1>Headline</h1>")
    );
    let rendered = article["rendered_blocks"].as_array().unwrap();
    assert_eq!(rendered.len(), 3);
    assert!(rendered[0]["experiment_id"].is_null());
    assert!(rendered[0]["variant_id"].is_null());

    // 7. RSS lists the published post.
    let resp = reqwest::blocking::get(format!("{base}/rss")).expect("rss");
    assert_eq!(resp.status().as_u16(), 200);
    let feed = resp.text().expect("rss body");
    assert!(feed.contains("My First Post (edited)"));
    assert!(feed.contains("my-first-post"));

    // 8. Three readers scroll the post; two finish it.
    let session = |i: u32| format!("22222222-2222-2222-2222-{:012}", i);
    let v1 = "11111111-1111-1111-1111-111111111111";
    let v2 = "11111111-1111-1111-1111-111111111112";
    let v3 = "11111111-1111-1111-1111-111111111113";
    let reader = |opv: &str| Visitor::new(base, opv);
    for (visitor, session_suffix, complete) in [(v1, 1, true), (v2, 2, true), (v3, 3, false)] {
        let v = reader(visitor);
        v.event(
            &slug,
            &session(session_suffix),
            "view",
            None,
            None,
            json!({}),
        );
        let bands: &[i64] = if complete {
            &[25, 50, 75, 100]
        } else {
            &[25, 50]
        };
        for band in bands {
            v.event(
                &slug,
                &session(session_suffix),
                "banded_scroll",
                None,
                None,
                json!({ "band": band }),
            );
        }
        for bid in blocks {
            v.event(
                &slug,
                &session(session_suffix),
                "block_impression",
                Some(bid["id"].as_str().unwrap()),
                None,
                json!({}),
            );
        }
        v.event(
            &slug,
            &session(session_suffix),
            "article_read",
            None,
            None,
            json!({ "read_time_ms": if complete { 45_000 } else { 5_000 } }),
        );
    }

    let (status, stats) = creator.get(&format!("/api/documents/{id}/stats"));
    assert_eq!(status, 200);
    assert_eq!(stats["article"]["views"], 3);
    assert_eq!(stats["article"]["unique_readers"], 3);
    assert_eq!(stats["article"]["read_events"], 3);
    assert_eq!(stats["article"]["completion"], json!(2.0 / 3.0));
    let bands = stats["article"]["band_reach"].as_array().unwrap();
    assert_eq!(bands.last().unwrap()["band"], 100);
    assert_eq!(bands.last().unwrap()["pageviews"], 2);
    let block_stats = stats["blocks"].as_array().unwrap();
    assert_eq!(block_stats.len(), 3);
    assert_eq!(block_stats[0]["impressions"], 3);

    // 9. A reader comments; the creator moderates it into the public post.
    let resp = reqwest::blocking::Client::new()
        .post(format!("{base}/api/articles/{slug}/comments"))
        .json(&json!({ "author_name": "Bob", "body": "Nice post!" }))
        .send()
        .expect("post comment");
    assert_eq!(resp.status().as_u16(), 201, "comment created");
    let comment: Value = resp.json().expect("comment JSON");
    let comment_id = comment["id"].as_str().unwrap().to_string();

    let (status, pending) = creator.get("/api/comments/pending");
    assert_eq!(status, 200);
    assert_eq!(pending.as_array().unwrap().len(), 1);

    let (status, _) = creator.post(&format!("/api/comments/{comment_id}/approve"), None);
    assert_eq!(status, 204, "comment approved");

    let resp = reqwest::blocking::get(format!("{base}/api/articles/{slug}/comments"))
        .expect("public comments");
    assert_eq!(resp.status().as_u16(), 200);
    let comments: Value = resp.json().expect("comments JSON");
    assert_eq!(comments.as_array().unwrap().len(), 1);
    assert_eq!(
        comments[0]["body"], "Nice post!",
        "approved comment is public"
    );

    // 10. A/B test the headline.
    let (status, exp) = creator.post(
        "/api/experiments",
        Some(json!({
            "document_id": id,
            "block_id": heading_block,
            "name": "Headline test",
            "traffic_weight": 50,
            "variants": [
                { "content": { "text": "New headline" }, "weight": 50 },
            ],
        })),
    );
    assert_eq!(status, 200, "experiment created");
    let exp_id = exp["id"].as_str().unwrap().to_string();
    assert_eq!(exp["status"], "draft");
    assert_eq!(exp["variants"].as_array().unwrap().len(), 2);
    let control_id = exp["variants"][0]["id"].as_str().unwrap().to_string();
    let test_id = exp["variants"][1]["id"].as_str().unwrap().to_string();
    assert_eq!(exp["variants"][0]["is_control"], true);

    let (status, _) = creator.post(&format!("/api/experiments/{exp_id}/start"), None);
    assert_eq!(status, 204, "experiment started");

    // 11. Assignment is stable per visitor and reflects the traffic split.
    let assigned_a = visitor_assignment(base, &slug, v1, &exp_id);
    let assigned_b = visitor_assignment(base, &slug, v1, &exp_id);
    assert_eq!(assigned_a, assigned_b, "same visitor sees the same variant");
    assert!(
        assigned_a == control_id || assigned_a == test_id,
        "visitor is assigned a real variant"
    );
    let promoted_visitor = "33333333-3333-3333-3333-333333333333";
    let _ = visitor_assignment(base, &slug, promoted_visitor, &exp_id);

    // 12. Readers see impressions and conversions; the live report counts them.
    let convert = |opv: &str, sid: u32, variant: &str, does: bool| {
        let v = Visitor::new(base, opv);
        v.event(
            &slug,
            &session(sid),
            "experiment_impression",
            None,
            Some(&json!({ "experiment_id": exp_id, "variant_id": variant })),
            json!({}),
        );
        if does {
            v.event(
                &slug,
                &session(sid),
                "experiment_conversion",
                None,
                Some(&json!({ "experiment_id": exp_id, "variant_id": variant })),
                json!({}),
            );
        }
    };
    // Three conversions on the test variant, none on control.
    for (i, visitor) in [v1, v2, promoted_visitor].iter().enumerate() {
        convert(visitor, 40 + i as u32, &test_id, true);
    }
    convert(v3, 43, &control_id, false);

    let (status, list) = creator.get(&format!("/api/documents/{id}/experiments"));
    assert_eq!(status, 200);
    let report = &list.as_array().unwrap()[0]["report"];
    let variants = report["variants"].as_array().unwrap();
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0]["impressions"], json!(1)); // control: one impression
    assert_eq!(variants[0]["conversions"], json!(0));
    assert_eq!(variants[1]["impressions"], json!(3)); // test: three impressions
    assert_eq!(variants[1]["conversions"], json!(3));

    // 13. Low sample size: the sequential rules keep collecting.
    let (status, outcome) = creator.post(&format!("/api/experiments/{exp_id}/decide"), None);
    assert_eq!(status, 200);
    assert!(outcome.is_null(), "no decision below the min-sample guard");

    // 14. Manual promote: the article now canonically shows the winner.
    let (status, outcome) = creator.post(&format!("/api/experiments/{exp_id}/promote"), None);
    assert_eq!(status, 200);
    assert_eq!(outcome["decision"], "winner");
    assert_eq!(outcome["winner_variant_id"], json!(test_id));

    let resp = reqwest::blocking::get(format!("{base}/api/articles/{slug}")).expect("article");
    assert_eq!(resp.status().as_u16(), 200);
    let article: Value = resp.json().expect("post-promotion article JSON");
    let rb = &article["rendered_blocks"][0];
    assert!(
        rb["html"].as_str().unwrap().contains("New headline"),
        "promoted variant is the canonical block"
    );
    assert!(
        rb["experiment_id"].is_null(),
        "no longer an experiment overlay"
    );

    let (_, list) = creator.get(&format!("/api/documents/{id}/experiments"));
    let decided = &list.as_array().unwrap()[0];
    assert_eq!(decided["status"], "decided");
    assert_eq!(decided["decision"], "winner");
    assert_eq!(decided["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(decided["decisions"][0]["winner_variant_id"], json!(test_id));

    // 15. Logout invalidates the session, and the creator can log back in.
    assert_eq!(creator.logout(), 204);
    assert_eq!(creator.me().0, 401);
    creator.login("alice@example.com");
    let (status, me) = creator.me();
    assert_eq!(status, 200);
    assert_eq!(me["user"]["email"], "alice@example.com");

    // 16. Backup: export covers documents, experiments, and decisions.
    let dump = export_database(&server.db_path);
    assert_eq!(dump["documents"].as_array().unwrap().len(), 1);
    assert_eq!(dump["experiments"].as_array().unwrap().len(), 1);
    assert_eq!(dump["experiment_decisions"].as_array().unwrap().len(), 1);
    // Rows are serialized as tuples in field order; see `export_json`.
    assert_eq!(dump["experiments"][0][4], json!("decided")); // status
    assert_eq!(dump["experiment_decisions"][0][3], json!("winner")); // decision
}

/// GET the article as `opv` and return the experiment variant it was assigned.
fn visitor_assignment(base: &str, slug: &str, opv: &str, exp_id: &str) -> String {
    let article = Visitor::new(base, opv).article(slug);
    let rb = &article["rendered_blocks"][0];
    assert_eq!(rb["experiment_id"], json!(exp_id));
    rb["variant_id"].as_str().unwrap().to_string()
}

/// A reader concludes "no improvement": content stays canonical, decision kept.
#[test]
fn no_winner_conclusion() {
    let server = start_server();
    let base = &server.base;
    let mut creator = Creator::new(base);
    creator.setup("carol@example.com", "Carol");
    let (_, doc) = creator.post(
        "/api/documents",
        Some(json!({ "title": "No Winner Post", "markdown": "# Control\n\nBody." })),
    );
    let id = doc["id"].as_str().unwrap().to_string();
    let slug = doc["slug"].as_str().unwrap().to_string();
    let heading = doc["blocks"][0]["id"].as_str().unwrap().to_string();
    creator.post(&format!("/api/documents/{id}/publish"), None);

    let (_, exp) = creator.post(
        "/api/experiments",
        Some(json!({
            "document_id": id,
            "block_id": heading,
            "traffic_weight": 100,
            "variants": [{ "content": { "text": "Worse headline" }, "weight": 50 }],
        })),
    );
    let exp_id = exp["id"].as_str().unwrap().to_string();
    creator.post(&format!("/api/experiments/{exp_id}/start"), None);

    let (status, outcome) = creator.post(&format!("/api/experiments/{exp_id}/no-winner"), None);
    assert_eq!(status, 200);
    assert_eq!(outcome["decision"], "no_improvement");

    // The article keeps the original content and drops the overlay.
    let resp = reqwest::blocking::get(format!("{base}/api/articles/{slug}")).expect("article");
    let article: Value = resp.json().expect("article JSON");
    let rb = &article["rendered_blocks"][0];
    assert!(rb["html"].as_str().unwrap().contains("Control"));
    assert!(rb["experiment_id"].is_null());

    let (_, list) = creator.get(&format!("/api/documents/{id}/experiments"));
    let e = &list.as_array().unwrap()[0];
    assert_eq!(e["status"], "decided");
    assert_eq!(e["decision"], "no_improvement");
}

/// Setup is a one-time event: restarting on the same database skips it, and
/// a second setup attempt is rejected.
#[test]
fn fresh_setup_locks_and_second_serve_skips_setup() {
    let server = start_server();
    let base = &server.base;
    let mut creator = Creator::new(base);
    creator.setup("dave@example.com", "Dave");

    // A second setup attempt is refused.
    let resp = reqwest::blocking::Client::new()
        .post(format!("{base}/api/setup"))
        .json(&json!({
            "email": "eve@example.com",
            "password": PASSWORD,
            "display_name": "Eve",
        }))
        .send()
        .expect("second setup");
    assert_eq!(
        resp.status().as_u16(),
        409,
        "setup is locked after first run"
    );

    // Stop, restart on the same database file, and confirm it is configured.
    let port = free_port();
    let base2 = format!("http://127.0.0.1:{port}");
    let db_url = format!("sqlite://{}", server.db_path.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_openpublish"))
        .args([
            "serve",
            "--database-url",
            &db_url,
            "--addr",
            &format!("127.0.0.1:{port}"),
        ])
        .spawn()
        .expect("spawn second serve");
    wait_ready(&base2);
    let resp = reqwest::blocking::get(format!("{base2}/api/setup")).expect("setup status");
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().expect("setup status JSON");
    assert_eq!(body, json!({ "setup_complete": true }));
    let _ = child.kill();
    let _ = child.wait();
}
