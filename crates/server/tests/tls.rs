//! TLS system tests.
//!
//! Spawns the real `forgepost` binary over a real socket with
//! bring-your-own certificates (`--tls-cert`/`--tls-key`), then verifies:
//!   - HTTPS works with the self-signed cert trusted as a root,
//!   - session cookies carry the `Secure` flag when TLS is active (and not
//!     when it is plain HTTP),
//!   - the HTTP→HTTPS redirect listener 301s to the https:// URL,
//!   - `--tls-cert` without `--tls-key` is rejected at startup.
//!
//! ACME issuance is not exercised here: it needs a real publicly resolvable
//! domain, so it stays manual.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rcgen::{CertificateParams, DnType, Ia5String, KeyPair, SanType};
use reqwest::blocking::Client;
use reqwest::header::{COOKIE, SET_COOKIE};
use serde_json::{Value, json};

const PASSWORD: &str = "correct horse battery staple";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A running `forgepost serve` process plus its scratch database.
struct Server {
    child: Child,
    _tmp: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_ready(client: &Client, url: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last_err = String::new();
    while Instant::now() < deadline {
        match client.get(url).send() {
            Ok(resp) if resp.status().is_success() => return,
            Ok(resp) => last_err = format!("health returned {}", resp.status()),
            Err(err) => last_err = err.to_string(),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("server did not become ready within 60s: {last_err}");
}

/// Spawn the binary. `client` + `ready_url` are used to wait until the server
/// is accepting requests (HTTPS client + https:// URL for TLS modes, plain
/// HTTP otherwise).
fn start_server(
    tls_port: u16,
    redirect_port: u16,
    cert: Option<(&str, &str)>,
    client: Client,
    ready_url: String,
) -> Server {
    let tmp = tempfile::tempdir().expect("temp dir");
    let db_url = format!("sqlite://{}", tmp.path().join("tls.db").display());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_forgepost"));
    cmd.args([
        "serve",
        "--database-url",
        &db_url,
        "--addr",
        &format!("127.0.0.1:{tls_port}"),
        "--http-redirect-port",
        &redirect_port.to_string(),
    ]);
    if let Some((cert, key)) = cert {
        cmd.args(["--tls-cert", cert, "--tls-key", key]);
    }
    let child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn forgepost serve");
    let server = Server { child, _tmp: tmp };
    wait_ready(&client, &ready_url);
    server
}

/// A client that trusts `cert_pem` as a root, for HTTPS requests.
fn tls_client(cert_pem: &str) -> Client {
    Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(cert_pem.as_bytes()).expect("cert"))
        .build()
        .expect("tls client")
}

/// A plain HTTP client that never follows redirects.
fn plain_client() -> Client {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("plain client")
}

/// A self-signed cert for `127.0.0.1` / `localhost`, returned as
/// `(cert_pem, key_pem)`.
fn make_cert() -> (String, String) {
    let key = KeyPair::generate().expect("generate key");
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).expect("params");
    params
        .distinguished_name
        .push(DnType::CommonName, "forgepost tls test");
    params.subject_alt_names = vec![
        SanType::DnsName(Ia5String::try_from("localhost").expect("dns name")),
        SanType::IpAddress("127.0.0.1".parse().expect("ip")),
    ];
    let cert = params.self_signed(&key).expect("self-signed cert");
    (cert.pem(), key.serialize_pem())
}

fn cert_files() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
    let (cert_pem, key_pem) = make_cert();
    let tmp = tempfile::tempdir().expect("temp dir");
    let cert_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    std::fs::write(&cert_path, &cert_pem).expect("write cert");
    std::fs::write(&key_path, &key_pem).expect("write key");
    (tmp, cert_path, key_path, cert_pem)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// HTTPS works end to end: health, setup, login, and the session cookie is
/// flagged `Secure` because TLS is active.
#[test]
fn byo_https_serves_and_marks_cookies_secure() {
    let (tmp, cert_path, key_path, cert_pem) = cert_files();
    let cert_ref = cert_path.display().to_string();
    let key_ref = key_path.display().to_string();

    let tls_port = free_port();
    let https = format!("https://127.0.0.1:{tls_port}");
    let https_client = tls_client(&cert_pem);
    let _server = start_server(
        tls_port,
        free_port(),
        Some((&cert_ref, &key_ref)),
        tls_client(&cert_pem),
        format!("{https}/health"),
    );

    // Health is reachable over HTTPS with the self-signed cert trusted.
    let resp = https_client
        .get(format!("{https}/health"))
        .send()
        .expect("https health");
    assert_eq!(resp.status().as_u16(), 200, "https /health");
    assert_eq!(
        resp.json::<Value>().expect("json"),
        json!({ "status": "ok" })
    );

    // Setup over HTTPS sets a Secure session cookie.
    let resp = https_client
        .post(format!("{https}/api/setup"))
        .json(&json!({
            "email": "alice@example.com",
            "password": PASSWORD,
            "display_name": "Alice",
        }))
        .send()
        .expect("https setup");
    assert_eq!(resp.status().as_u16(), 200, "setup succeeds over https");
    let set_cookie = resp.headers().get(SET_COOKIE).expect("session cookie");
    let set_cookie = set_cookie.to_str().expect("cookie header");
    assert!(
        set_cookie.contains("Secure"),
        "session cookie is Secure under TLS: {set_cookie}"
    );
    assert!(set_cookie.contains("HttpOnly"), "cookie is HttpOnly");

    // The cookie actually authenticates further HTTPS requests.
    let session = set_cookie.split(';').next().expect("cookie pair");
    let resp = https_client
        .get(format!("{https}/api/me"))
        .header(COOKIE, session)
        .send()
        .expect("https /api/me");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.json::<Value>().expect("json")["user"]["email"],
        "alice@example.com"
    );

    drop(tmp);
}

/// Plain HTTP (no TLS flags) does not mark session cookies Secure.
#[test]
fn plain_http_cookies_are_not_secure() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let http = plain_client();
    let _server = start_server(
        port,
        free_port(),
        None,
        http.clone(),
        format!("{base}/health"),
    );

    let resp = http
        .post(format!("{base}/api/setup"))
        .json(&json!({
            "email": "bob@example.com",
            "password": PASSWORD,
            "display_name": "Bob",
        }))
        .send()
        .expect("plain setup");
    assert_eq!(resp.status().as_u16(), 200);
    let set_cookie = resp
        .headers()
        .get(SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .expect("cookie header")
        .to_string();
    assert!(
        !set_cookie.contains("Secure"),
        "session cookie is not Secure over plain HTTP: {set_cookie}"
    );
}

/// The HTTP→HTTPS redirect listener 301s to the https:// URL, preserving the
/// path and query string.
#[test]
fn http_redirect_listener_301s_to_https() {
    let (tmp, cert_path, key_path, cert_pem) = cert_files();
    let cert_ref = cert_path.display().to_string();
    let key_ref = key_path.display().to_string();

    let tls_port = free_port();
    let redirect_port = free_port();
    let https = format!("https://127.0.0.1:{tls_port}");
    let _server = start_server(
        tls_port,
        redirect_port,
        Some((&cert_ref, &key_ref)),
        tls_client(&cert_pem),
        format!("{https}/health"),
    );
    let http = plain_client();

    let resp = http
        .get(format!("http://127.0.0.1:{redirect_port}/api/setup"))
        .send()
        .expect("redirected request");
    assert_eq!(resp.status().as_u16(), 301, "301 permanent redirect");
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .expect("location"),
        &format!("https://127.0.0.1:{tls_port}/api/setup"),
        "redirect preserves the path"
    );
    assert!(
        resp.text().expect("body").contains("Redirecting to HTTPS"),
        "redirect body is informative"
    );

    // Query strings survive the redirect too.
    let resp = http
        .get(format!(
            "http://127.0.0.1:{redirect_port}/articles/hello?utm_source=test"
        ))
        .send()
        .expect("redirected request");
    assert_eq!(resp.status().as_u16(), 301);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .expect("location"),
        &format!("https://127.0.0.1:{tls_port}/articles/hello?utm_source=test"),
        "redirect preserves the query string"
    );

    drop(tmp);
}

/// `--tls-cert` without `--tls-key` (and vice versa) is a startup error.
#[test]
fn tls_cert_without_key_is_rejected() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let (cert_pem, _) = make_cert();
    let cert_path = tmp.path().join("cert.pem");
    std::fs::write(&cert_path, &cert_pem).expect("write cert");

    let out = Command::new(env!("CARGO_BIN_EXE_forgepost"))
        .args([
            "serve",
            "--database-url",
            &format!("sqlite://{}", tmp.path().join("reject.db").display()),
            "--addr",
            &format!("127.0.0.1:{}", free_port()),
            "--tls-cert",
            &cert_path.display().to_string(),
        ])
        .output()
        .expect("run");
    assert!(
        !out.status.success(),
        "cert without key must fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("--tls-key") || stderr.to_lowercase().contains("tls key"),
        "error mentions the missing key flag: {stderr}"
    );

    // And the other way around.
    let (_, key_pem) = make_cert();
    let key_path = tmp.path().join("key.pem");
    std::fs::write(&key_path, &key_pem).expect("write key");
    let out = Command::new(env!("CARGO_BIN_EXE_forgepost"))
        .args([
            "serve",
            "--database-url",
            &format!("sqlite://{}", tmp.path().join("reject2.db").display()),
            "--addr",
            &format!("127.0.0.1:{}", free_port()),
            "--tls-key",
            &key_path.display().to_string(),
        ])
        .output()
        .expect("run");
    assert!(
        !out.status.success(),
        "key without cert must fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
