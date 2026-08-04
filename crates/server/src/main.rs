//! OpenPublish CLI entrypoint. Solo mode: single binary + embedded SQLite.

use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use openpublish_server::repository::SqliteRepository;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let args = match cli.command {
        Some(Command::Serve(args)) => args,
        None => ServeArgs::default(),
    };

    let repo = SqliteRepository::connect(&args.database_url).await?;
    repo.migrate().await?;
    tracing::info!(database_url = %args.database_url, "database ready");

    let app = openpublish_server::app(Arc::new(repo));
    let listener = tokio::net::TcpListener::bind(&args.addr).await?;
    tracing::info!(addr = %args.addr, "OpenPublish listening");
    axum::serve(listener, app).await?;
    Ok(())
}
