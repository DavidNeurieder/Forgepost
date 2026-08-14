//! Deploy-script tests.
//!
//! `system.rs` drives the binary with explicit CLI flags, which bypasses the
//! whole `deploy/` story. These tests instead validate the deployment contract:
//!   * the scripts reference the real repository (catching the kind of URL
//!     drift that once left stale `my_blog` references behind),
//!   * the scripts pass a `bash -n` syntax check,
//!   * a server started purely from the env-file keys `install.sh` writes
//!     actually serves the creator journey, and the real `forgepost-backup.sh`
//!     produces a valid JSON export + media tarball and prunes old backups.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::Method;
use reqwest::blocking::Client;
use serde_json::{Value, json};

const PASSWORD: &str = "correct horse battery staple";
const CSRF_HEADER: &str = "x-csrf-token";
const CANONICAL_SLUG: &str = "DavidNeurieder/Forgepost";

fn deploy_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy")
}

/// Normalize a git remote URL to its `owner/repo` slug.
fn repo_slug(remote: &str) -> String {
    let remote = remote.trim_end_matches(".git");
    if let Some(rest) = remote.strip_prefix("https://github.com/") {
        return rest.to_string();
    }
    if let Some(rest) = remote.strip_prefix("git@github.com:") {
        return rest.to_string();
    }
    if let Some(pos) = remote.rfind("github.com") {
        return remote[pos + "github.com".len() + 1..].to_string();
    }
    remote.to_string()
}

/// The env-file keys `install.sh` writes (see `forgepost.env` generation).
const INSTALL_ENV_KEYS: &[&str] = &[
    "FORGEPOST_ADDR",
    "FORGEPOST_TLS_DOMAIN",
    "DATABASE_URL",
    "FORGEPOST_MEDIA_DIR",
    "RUST_LOG",
];

#[test]
fn deploy_scripts_reference_the_real_repository() {
    let deploy = deploy_dir();

    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .expect("git remote get-url origin");
    assert!(out.status.success(), "origin remote must be configured");
    let origin = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        repo_slug(origin.trim()),
        CANONICAL_SLUG,
        "origin must point at the canonical repository"
    );

    let install = fs::read_to_string(deploy.join("install.sh")).expect("read install.sh");
    assert!(
        install.contains(&format!("https://github.com/{CANONICAL_SLUG}.git")),
        "install.sh must clone the canonical repository"
    );
    for key in INSTALL_ENV_KEYS {
        assert!(
            install.contains(key),
            "install.sh must write the {key} env key"
        );
    }

    for unit in ["forgepost.service", "forgepost-backup.service"] {
        let unit_text = fs::read_to_string(deploy.join(unit)).expect(unit);
        assert!(
            unit_text.contains(&format!("https://github.com/{CANONICAL_SLUG}")),
            "{unit} must document the canonical repository"
        );
    }
}

#[test]
fn deploy_scripts_pass_bash_syntax_check() {
    for script in ["install.sh", "update.sh", "forgepost-backup.sh"] {
        let out = Command::new("bash")
            .arg("-n")
            .arg(deploy_dir().join(script))
            .output()
            .expect("bash -n");
        assert!(
            out.status.success(),
            "{script} has a syntax error:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// Runtime: boot the server from install.sh-style env, then run the real
// backup script against it.
// ---------------------------------------------------------------------------

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
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

/// Minimal API client carrying the session cookie + CSRF token.
struct Api {
    http: Client,
    base: String,
    csrf: Option<String>,
}

impl Api {
    fn call(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
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

    fn setup(&mut self) {
        let (status, body) = self.call(
            Method::POST,
            "/api/setup",
            Some(json!({
                "email": "owner@example.com",
                "password": PASSWORD,
                "display_name": "Owner",
            })),
        );
        assert_eq!(status, 200, "first-run setup succeeds");
        self.csrf = body["csrf_token"].as_str().map(str::to_string);
        assert!(self.csrf.is_some(), "setup returns a CSRF token");
    }
}

fn list_backups(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut dbs = Vec::new();
    let mut tarballs = Vec::new();
    for entry in fs::read_dir(dir).expect("read backups dir") {
        let name = entry
            .expect("dir entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.starts_with("db-") && name.ends_with(".json") {
            dbs.push(name);
        } else if name.starts_with("media-") && name.ends_with(".tar.gz") {
            tarballs.push(name);
        }
    }
    (dbs, tarballs)
}

#[test]
fn install_style_env_boots_the_server_and_backup_script_runs() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();
    let media = root.join("media");
    let backups = root.join("backups");
    fs::create_dir_all(&media).expect("media dir");
    fs::create_dir_all(&backups).expect("backups dir");
    fs::write(media.join("dummy.jpg"), b"not a real jpeg").expect("dummy media file");

    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let db_path = root.join("forgepost.db");
    let envs = [
        ("FORGEPOST_ADDR".to_string(), format!("127.0.0.1:{port}")),
        (
            "DATABASE_URL".to_string(),
            format!("sqlite://{}", db_path.display()),
        ),
        (
            "FORGEPOST_MEDIA_DIR".to_string(),
            media.display().to_string(),
        ),
        ("RUST_LOG".to_string(), "info".to_string()),
    ];

    let mut child = Command::new(env!("CARGO_BIN_EXE_forgepost"))
        .arg("serve")
        .envs(envs.iter().cloned())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn forgepost serve via install-style env");
    wait_ready(&base);

    // The install.sh env contract is honored: the DB lands where DATABASE_URL
    // says, and the creator journey works over that config.
    let mut api = Api {
        http: Client::builder()
            .cookie_store(true)
            .build()
            .expect("client"),
        base: base.clone(),
        csrf: None,
    };
    api.setup();
    let (status, doc) = api.call(
        Method::POST,
        "/api/documents",
        Some(json!({
            "title": "Deploy Post",
            "markdown": "# Headline\n\nBody.",
            "tags": ["ops"],
        })),
    );
    assert_eq!(status, 200, "create document");
    let id = doc["id"].as_str().expect("document id").to_string();
    let slug = doc["slug"].as_str().expect("slug").to_string();
    let (status, published) = api.call(Method::POST, &format!("/api/documents/{id}/publish"), None);
    assert_eq!(status, 200);
    assert_eq!(published["status"], "published");
    let (status, article) = api.call(Method::GET, &format!("/api/articles/{slug}"), None);
    assert_eq!(status, 200, "published post is public");
    assert!(
        article["html"]
            .as_str()
            .unwrap()
            .contains("<h1>Headline</h1>"),
        "article renders"
    );
    assert!(db_path.exists(), "DATABASE_URL is honored");
    let _ = child.kill();
    let _ = child.wait();

    // Run the real backup script with path overrides against this layout.
    let run_backup = || {
        let out = Command::new("bash")
            .arg(deploy_dir().join("forgepost-backup.sh"))
            .env("FORGEPOST_BACKUP_BIN", env!("CARGO_BIN_EXE_forgepost"))
            .env(
                "FORGEPOST_BACKUP_DB",
                format!("sqlite://{}", db_path.display()),
            )
            .env("FORGEPOST_BACKUP_MEDIA_DIR", media.display().to_string())
            .env("FORGEPOST_BACKUP_DIR", backups.display().to_string())
            .env("FORGEPOST_BACKUP_RETENTION_DAYS", "30")
            .output()
            .expect("forgepost-backup.sh runs");
        assert!(
            out.status.success(),
            "forgepost-backup.sh failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_backup();

    let (dbs, tarballs) = list_backups(&backups);
    assert_eq!(dbs.len(), 1, "one DB export: {dbs:?}");
    assert_eq!(tarballs.len(), 1, "one media tarball: {tarballs:?}");

    let export: Value =
        serde_json::from_str(&fs::read_to_string(backups.join(&dbs[0])).expect("read export"))
            .expect("export is JSON");
    assert_eq!(export["documents"].as_array().unwrap().len(), 1);
    assert!(
        fs::read_to_string(backups.join(&dbs[0]))
            .expect("read export")
            .contains("Deploy Post"),
        "export contains the published document"
    );

    let tar_list = Command::new("tar")
        .args(["-tzf", backups.join(&tarballs[0]).to_str().unwrap()])
        .output()
        .expect("tar -tzf");
    assert!(tar_list.status.success());
    let listing = String::from_utf8_lossy(&tar_list.stdout);
    assert!(
        listing.contains("media/dummy.jpg"),
        "tarball holds the media"
    );

    // Retention: backdate a fake pair 40 days and rerun; it must be pruned
    // while today's artifacts survive.
    let stale_db = backups.join("db-1999-01-01.json");
    let stale_media = backups.join("media-1999-01-01.tar.gz");
    fs::write(&stale_db, "{}").expect("stale db");
    fs::write(&stale_media, b"stale").expect("stale tarball");
    for stale in [&stale_db, &stale_media] {
        let out = Command::new("touch")
            .args(["-d", "40 days ago", stale.to_str().unwrap()])
            .output()
            .expect("backdate stale backup");
        assert!(out.status.success());
    }
    run_backup();

    assert!(!stale_db.exists(), "stale db pruned by retention");
    assert!(!stale_media.exists(), "stale tarball pruned by retention");
    let (dbs, tarballs) = list_backups(&backups);
    assert_eq!(dbs.len(), 1, "current export survives: {dbs:?}");
    assert_eq!(tarballs.len(), 1, "current tarball survives: {tarballs:?}");
}
