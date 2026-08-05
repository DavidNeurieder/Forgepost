//! OpenPublish CLI entrypoint. Solo mode: single binary + embedded SQLite.

use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use openpublish_server::experiments;
use openpublish_server::repository::{Repository, SqliteRepository};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "openpublish",
    version,
    about = "Self-hosted block-level experimentation for creators"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the OpenPublish server (default).
    Serve(ServeArgs),
    /// Export the entire database as JSON (backups / migration).
    Export(ExportArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// SQLite database URL or file path.
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://openpublish.db")]
    database_url: String,
    /// Address to bind.
    #[arg(long, env = "OPENPUBLISH_ADDR", default_value = "127.0.0.1:8080")]
    addr: String,
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            database_url: "sqlite://openpublish.db".into(),
            addr: "127.0.0.1:8080".into(),
        }
    }
}

#[derive(Args)]
struct ExportArgs {
    /// SQLite database URL or file path.
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://openpublish.db")]
    database_url: String,
    /// Write the export to this file instead of stdout.
    #[arg(long)]
    output: Option<String>,
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
        Some(Command::Serve(args)) => args,
        None => ServeArgs::default(),
    };

    let repo = SqliteRepository::connect(&args.database_url).await?;
    repo.migrate().await?;
    tracing::info!(database_url = %args.database_url, "database ready");

    let repo = Arc::new(repo);
    spawn_auto_decider(repo.clone());

    let app = openpublish_server::app(repo);
    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    tracing::info!(addr = %args.addr, "OpenPublish listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Periodically evaluate running experiments and apply decisions (promote a
/// clear winner or conclude "no improvement"). The engine's stopping rules are
/// spending-bound corrected and gated by the per-experiment min-sample guard,
/// so an interval poll is safe to run forever.
fn spawn_auto_decider(repo: Arc<dyn Repository>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let ids = match repo.running_experiments().await {
                Ok(list) => list.into_iter().map(|e| e.id).collect::<Vec<_>>(),
                Err(err) => {
                    tracing::warn!(error = %err, "auto-decider could not list experiments");
                    continue;
                }
            };
            for id in ids {
                match experiments::decide_experiment(&*repo, id).await {
                    Ok(Some(outcome)) => tracing::info!(
                        experiment_id = %outcome.experiment_id,
                        decision = %outcome.decision,
                        "auto-decider applied experiment decision"
                    ),
                    Ok(None) => {}
                    Err(err) => {
                        tracing::warn!(experiment_id = %id, error = %err, "auto-decider failed");
                    }
                }
            }
        }
    });
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
