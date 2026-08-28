//! Forgepost CLI entrypoint. Solo mode: single binary + embedded SQLite.
//!
//! `serve` can run plain HTTP or HTTPS, either with a certificate you supply
//! (`--tls-cert`/`--tls-key`) or with automatic Let's Encrypt issuance and
//! renewal (`--tls-domain`). When TLS is active an HTTP redirect listener is
//! started on port 80 unless `--no-http-redirect` is given.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    http::{StatusCode, Uri, header},
};
use axum_server::tls_rustls::RustlsConfig;
use clap::{Args, Parser, Subcommand};
use forgepost_analytics::RateLimiter;
use forgepost_application::ports::{ExportRepo, Repository};
use forgepost_application::services::backup::BackupService;
use forgepost_application::worker::BackgroundWorker;
use forgepost_infrastructure::backup::ArchiveBackup;
use forgepost_infrastructure::sqlite::SqliteRepository;
use ipnet::IpNet;
use rustls_acme::AcmeConfig;
use rustls_acme::caches::DirCache;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

/// Fixed login for the bundled demo blog (`forgepost demo`).
const DEMO_ADMIN_EMAIL: &str = "admin@example.com";
const DEMO_ADMIN_PASSWORD: &str = "demo-password";
const DEMO_ARTIFACT: &[u8] = include_bytes!("../../../demo/forgepost-demo.fpb");

#[derive(Parser)]
#[command(
    name = "forgepost",
    version,
    about = "Self-hosted block-level experimentation for creators"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the Forgepost server (default).
    Serve(ServeArgs),
    /// Export the entire database as JSON (backups / migration).
    Export(ExportArgs),
    /// Create, verify, or restore a `.fpb` backup archive.
    Backup(BackupArgs),
    /// Install the bundled demo blog into an empty database.
    Demo(DemoArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// SQLite database URL or file path.
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://forgepost.db")]
    database_url: String,
    /// Address to bind (plain HTTP by default, TLS when --tls-* is given).
    #[arg(long, env = "FORGEPOST_ADDR", default_value = "127.0.0.1:8080")]
    addr: String,
    /// Automate HTTPS via Let's Encrypt (TLS-ALPN-01). Takes precedence over
    /// --tls-cert/--tls-key.
    #[arg(long, env = "FORGEPOST_TLS_DOMAIN")]
    tls_domain: Option<String>,
    /// Path to a TLS certificate chain (PEM), bring-your-own HTTPS.
    #[arg(long, env = "FORGEPOST_TLS_CERT")]
    tls_cert: Option<PathBuf>,
    /// Path to the matching TLS private key (PEM).
    #[arg(long, env = "FORGEPOST_TLS_KEY")]
    tls_key: Option<PathBuf>,
    /// Directory for the Let's Encrypt ACME cache (tier 2 only).
    #[arg(long, env = "FORGEPOST_TLS_CACHE_DIR", default_value = "./tls")]
    tls_cache_dir: PathBuf,
    /// Do not start the HTTP→HTTPS redirect listener when TLS is active.
    #[arg(long)]
    no_http_redirect: bool,
    /// Port for the HTTP→HTTPS redirect listener (default 80).
    #[arg(long, env = "FORGEPOST_HTTP_REDIRECT_PORT", default_value_t = 80)]
    http_redirect_port: u16,
    /// Directory where uploaded media bytes are stored (served at /media).
    #[arg(long, env = "FORGEPOST_MEDIA_DIR")]
    media_dir: Option<PathBuf>,
    /// Public origin used for canonical/RSS/OG links when `site.url` is unset
    /// (e.g. `example.com`). Supplied automatically with `--tls-domain`.
    #[arg(long, env = "FORGEPOST_PUBLIC_HOST")]
    public_host: Option<String>,
    /// Reverse proxy whose `x-forwarded-for` may be trusted for rate limiting
    /// (IP or CIDR, repeatable, comma-separated via env).
    #[arg(long, env = "FORGEPOST_TRUSTED_PROXY", value_delimiter = ',')]
    trusted_proxy: Vec<String>,
    /// Explicitly allow plain HTTP on a non-loopback address. Session cookies
    /// then lack the `Secure` flag; only use behind a trusted TLS terminator
    /// reachable exclusively over loopback, or accept the risk on your LAN.
    #[arg(long, env = "FORGEPOST_INSECURE_HTTP")]
    insecure_http: bool,
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            database_url: "sqlite://forgepost.db".into(),
            addr: "127.0.0.1:8080".into(),
            tls_domain: None,
            tls_cert: None,
            tls_key: None,
            tls_cache_dir: "./tls".into(),
            no_http_redirect: false,
            http_redirect_port: 80,
            media_dir: None,
            public_host: None,
            trusted_proxy: Vec::new(),
            insecure_http: false,
        }
    }
}

#[derive(Args)]
struct ExportArgs {
    /// SQLite database URL or file path.
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://forgepost.db")]
    database_url: String,
    /// Write the export to this file instead of stdout.
    #[arg(long)]
    output: Option<String>,
}

#[derive(Args)]
struct BackupArgs {
    /// SQLite database URL or file path.
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://forgepost.db")]
    database_url: String,
    /// Directory where uploaded media bytes live (default: `media/` next to
    /// the database).
    #[arg(long, env = "FORGEPOST_MEDIA_DIR")]
    media_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: BackupCommand,
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Seal the database + media into a `.fpb` archive and verify the result.
    Create {
        /// Write the archive to this file.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Check an archive's manifest, checksums, and database integrity.
    Verify {
        /// Path to the `.fpb` archive.
        path: PathBuf,
    },
    /// Replace the live database with an archive (the pre-restore database is
    /// preserved as a rollback; media files are merged additively).
    Restore {
        /// Path to the `.fpb` archive.
        path: PathBuf,
        /// Validate the archive and report, but write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Confirm that the live database may be replaced.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
struct DemoArgs {
    /// SQLite database URL or file path to install the demo into.
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "sqlite://forgepost-demo.db"
    )]
    database_url: String,
    /// Directory where media files are written (default: `media/` next to the
    /// database).
    #[arg(long, env = "FORGEPOST_MEDIA_DIR")]
    media_dir: Option<PathBuf>,
}

enum TlsMode {
    None,
    Byo { cert: PathBuf, key: PathBuf },
    Acme { domain: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let args = match cli.command {
        Some(Command::Export(args)) => return export(args).await,
        Some(Command::Backup(args)) => return backup(args).await,
        Some(Command::Demo(args)) => return demo(args).await,
        Some(Command::Serve(args)) => args,
        None => ServeArgs::default(),
    };

    serve(args).await
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    // rustls needs a crypto provider installed exactly once, before any
    // `ServerConfig` is built.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let repo = SqliteRepository::connect(&args.database_url).await?;
    repo.migrate().await?;
    tracing::info!(database_url = %args.database_url, "database ready");
    forgepost_infrastructure::sqlite::backfill_search_index(&repo).await?;

    let repo: Arc<dyn Repository> = Arc::new(repo);

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    BackgroundWorker::new(repo.clone(), shutdown_rx.clone(), 60).spawn();

    let mode = tls_mode(&args)?;
    let secure = matches!(mode, TlsMode::Byo { .. } | TlsMode::Acme { .. });
    let media_dir = match &args.media_dir {
        Some(dir) => dir.clone(),
        None => default_media_dir(&args.database_url),
    };
    tokio::fs::create_dir_all(&media_dir).await?;
    tracing::info!(media_dir = %media_dir.display(), "media directory ready");
    let client_ip = forgepost_server::routes::ClientIpConfig {
        trusted_proxies: build_trusted_proxies(&args.trusted_proxy)?,
    };
    // `--tls-domain` implies the canonical public origin; BYO certs and plain
    // HTTP need the operator to configure `site.url` or `--public-host`.
    let public_host = args.public_host.clone().or_else(|| args.tls_domain.clone());
    let app = forgepost_server::app_with_security(
        repo,
        RateLimiter::new(RateLimiter::DEFAULT_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_LOGIN_MAX),
        RateLimiter::new(RateLimiter::DEFAULT_COMMENT_MAX),
        Arc::new(client_ip),
        public_host,
        secure,
        Some(media_dir),
    );
    let socket_addr: std::net::SocketAddr = args.addr.parse()?;

    spawn_shutdown_signal(shutdown_tx);

    match mode {
        TlsMode::None => {
            if !args.insecure_http && !socket_addr.ip().is_loopback() {
                anyhow::bail!(
                    "refusing to serve plain HTTP on a non-loopback address ({}): session \
                     cookies would lack the Secure flag. Bind a loopback address and use a TLS \
                     front, pass --tls-cert/--tls-key or --tls-domain for HTTPS, or confirm the \
                     risk with --insecure-http",
                    args.addr
                );
            }
            let listener = TcpListener::bind(socket_addr).await?;
            tracing::info!(addr = %args.addr, "Forgepost listening (http)");
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                shutdown_rx.changed().await.ok();
            })
            .await?;
        }
        TlsMode::Byo { cert, key } => {
            let config = RustlsConfig::from_pem_file(&cert, &key).await?;
            tracing::info!(
                addr = %args.addr,
                cert = %cert.display(),
                "Forgepost listening (https, custom certificate)"
            );
            if !args.no_http_redirect {
                spawn_redirect_listener(
                    redirect_host(&args),
                    args.http_redirect_port,
                    socket_addr.port(),
                    shutdown_rx.clone(),
                );
            }
            spawn_cert_reloader(config.clone(), cert, key);
            let handle = axum_server::Handle::new();
            spawn_handle_shutdown(shutdown_rx, handle.clone());
            axum_server::bind_rustls(socket_addr, config.clone())
                .handle(handle)
                .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await?;
        }
        TlsMode::Acme { domain } => {
            let mut state = AcmeConfig::new([domain.clone()])
                .cache_option(Some(DirCache::new(args.tls_cache_dir.clone())))
                .directory_lets_encrypt(true)
                .state();
            let acceptor = state.axum_acceptor(state.default_rustls_config());
            tokio::spawn(async move {
                use futures::StreamExt;
                loop {
                    match state.next().await {
                        Some(Ok(event)) => tracing::info!(?event, "acme event"),
                        Some(Err(err)) => tracing::error!(?err, "acme error"),
                        None => break,
                    }
                }
            });
            tracing::info!(
                addr = %args.addr,
                domain = %domain,
                "Forgepost listening (https, automatic Let's Encrypt)"
            );
            if !args.no_http_redirect {
                spawn_redirect_listener(
                    domain.clone(),
                    args.http_redirect_port,
                    socket_addr.port(),
                    shutdown_rx.clone(),
                );
            }
            let handle = axum_server::Handle::new();
            spawn_handle_shutdown(shutdown_rx, handle.clone());
            axum_server::bind(socket_addr)
                .handle(handle)
                .acceptor(acceptor)
                .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await?;
        }
    }
    Ok(())
}

/// Parse the `--trusted-proxy` values (IP or CIDR) into `IpNet`s.
fn build_trusted_proxies(values: &[String]) -> anyhow::Result<Vec<IpNet>> {
    values
        .iter()
        .map(|v| {
            v.parse::<IpNet>()
                .map_err(|_| anyhow::anyhow!("invalid trusted proxy address: {v:?}"))
        })
        .collect()
}

/// Resolve the TLS mode with the documented precedence:
/// `--tls-domain` > `--tls-cert`/`--tls-key` > plain HTTP.
fn tls_mode(args: &ServeArgs) -> anyhow::Result<TlsMode> {
    if let Some(domain) = &args.tls_domain {
        return Ok(TlsMode::Acme {
            domain: domain.clone(),
        });
    }
    match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => Ok(TlsMode::Byo {
            cert: cert.clone(),
            key: key.clone(),
        }),
        (None, None) => Ok(TlsMode::None),
        _ => anyhow::bail!("--tls-cert and --tls-key must be provided together"),
    }
}

/// Default media directory: a `media/` folder next to the SQLite database.
fn default_media_dir(database_url: &str) -> PathBuf {
    let db_path = database_url
        .strip_prefix("sqlite://")
        .unwrap_or(database_url);
    let parent = Path::new(db_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    parent.join("media")
}

/// Host that HTTPS redirects should point at: the configured domain in ACME
/// mode, otherwise the host part of `--addr`.
fn redirect_host(args: &ServeArgs) -> String {
    if let Some(domain) = &args.tls_domain {
        return domain.clone();
    }
    args.addr
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(&args.addr)
        .to_string()
}

/// 301 every request to the matching `https://` URL.
fn http_redirect_app(host: String, tls_port: u16) -> Router {
    Router::new().fallback(move |uri: Uri| {
        let target = format!(
            "https://{host}:{tls_port}{path}",
            path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/")
        );
        async move {
            (
                StatusCode::MOVED_PERMANENTLY,
                [(header::LOCATION, target)],
                "Redirecting to HTTPS".to_string(),
            )
        }
    })
}

fn spawn_redirect_listener(
    host: String,
    port: u16,
    tls_port: u16,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let listener = match TcpListener::bind(format!("0.0.0.0:{port}")).await {
            Ok(l) => l,
            Err(err) => {
                tracing::warn!(port, error = %err, "HTTP→HTTPS redirect listener not started");
                return;
            }
        };
        tracing::info!(port, "HTTP→HTTPS redirect listening");
        let app = http_redirect_app(host, tls_port);
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.changed().await.ok();
            })
            .await
        {
            tracing::warn!(error = %err, "HTTP→HTTPS redirect listener stopped");
        }
    });
}

/// Watch the shutdown channel and drain axum-server's connections.
fn spawn_handle_shutdown(
    mut shutdown_rx: watch::Receiver<bool>,
    handle: axum_server::Handle<std::net::SocketAddr>,
) {
    tokio::spawn(async move {
        shutdown_rx.changed().await.ok();
        handle.shutdown();
    });
}

fn spawn_shutdown_signal(tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx.send(true);
    });
}

/// Reload the TLS certificate/key whenever either file changes on disk
/// (e.g. after a renewal). Checks mtimes every 30 s.
fn spawn_cert_reloader(config: RustlsConfig, cert: PathBuf, key: PathBuf) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = file_mtimes(&cert, &key);
        loop {
            ticker.tick().await;
            let now = file_mtimes(&cert, &key);
            if now == last {
                continue;
            }
            last = now;
            match config.reload_from_pem_file(&cert, &key).await {
                Ok(()) => tracing::info!("TLS certificate reloaded"),
                Err(err) => tracing::error!(error = %err, "TLS certificate reload failed"),
            }
        }
    });
}

fn file_mtimes(
    cert: &Path,
    key: &Path,
) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let mtime = |p: &Path| std::fs::metadata(p).ok().and_then(|m| m.modified().ok());
    (mtime(cert), mtime(key))
}

async fn export(args: ExportArgs) -> anyhow::Result<()> {
    let repo = SqliteRepository::connect(&args.database_url).await?;
    repo.migrate().await?;
    let dump = repo.export_json().await?;
    let pretty = serde_json::to_string_pretty(&dump)?;
    match args.output {
        Some(path) => {
            std::fs::write(&path, pretty + "\n")?;
            tracing::info!(path = %path, "export written");
        }
        None => println!("{pretty}"),
    }
    Ok(())
}

/// Open a migrated repository handle (the CLI only manages one database).
async fn open_repo(database_url: &str) -> anyhow::Result<Arc<dyn Repository>> {
    let repo = SqliteRepository::connect(database_url).await?;
    repo.migrate().await?;
    Ok(Arc::new(repo))
}

/// Default archive name for `backup create` without `--output`.
fn default_backup_dest() -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PathBuf::from(format!("forgepost-{secs}.fpb"))
}

async fn backup(args: BackupArgs) -> anyhow::Result<()> {
    let media_dir = match &args.media_dir {
        Some(dir) => dir.clone(),
        None => default_media_dir(&args.database_url),
    };
    let repo = open_repo(&args.database_url).await?;
    let svc = BackupService::new(repo, Arc::new(ArchiveBackup));

    match args.command {
        BackupCommand::Create { output } => {
            let dest = output.unwrap_or_else(default_backup_dest);
            let report = svc.create(&args.database_url, &media_dir, &dest).await?;
            println!("created {}", report.path.display());
            for line in report.summary_lines() {
                println!("  {line}");
            }
            println!("  integrity: OK (each archive self-verifies on creation)");
        }
        BackupCommand::Verify { path } => {
            let report = svc.verify(&path).await?;
            for line in report.summary_lines() {
                println!("  {line}");
            }
            if report.ok {
                println!("verdict: OK — archive is intact and compatible");
            } else {
                println!("verdict: NOT COMPATIBLE — do not restore without the matching version");
            }
        }
        BackupCommand::Restore { path, dry_run, yes } => {
            if !dry_run && !yes {
                anyhow::bail!(
                    "refusing to replace the live database without --yes (inspect with --dry-run first)"
                );
            }
            let report = svc
                .restore(&path, &args.database_url, &media_dir, dry_run)
                .await?;
            for line in report.summary_lines() {
                println!("  {line}");
            }
            if dry_run {
                println!("verdict: OK — a real restore would replace the database");
            } else {
                println!(
                    "restored {} into {} (pre-restore database kept as a rollback file)",
                    report.path.display(),
                    args.database_url
                );
            }
        }
    }
    Ok(())
}

async fn demo(args: DemoArgs) -> anyhow::Result<()> {
    let media_dir = match &args.media_dir {
        Some(dir) => dir.clone(),
        None => default_media_dir(&args.database_url),
    };
    if !args.database_url.trim().starts_with("sqlite://") {
        anyhow::bail!("the demo needs a local SQLite database URL");
    }

    let tmp = std::env::temp_dir().join(format!(
        "forgepost-demo-{}.fpb",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, DEMO_ARTIFACT)?;

    let repo = open_repo(&args.database_url).await?;
    let svc = BackupService::new(repo, Arc::new(ArchiveBackup));
    let report = svc
        .restore(&tmp, &args.database_url, &media_dir, false)
        .await?;
    std::fs::remove_file(&tmp).ok();

    println!("demo installed into {}", args.database_url);
    println!("  articles: 6 demo posts with real content");
    println!(
        "  media:    4 bundled images restored to {}",
        media_dir.display()
    );
    println!("  action:   a live A/B experiment on 'Tracking Every Headline'");
    for line in report.summary_lines() {
        println!("  {line}");
    }
    println!(
        "login:     {DEMO_ADMIN_EMAIL} / {DEMO_ADMIN_PASSWORD} at http://127.0.0.1:8080/admin"
    );
    Ok(())
}
