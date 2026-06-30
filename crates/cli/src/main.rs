//! `sdm` — the SmartDownloadManager CLI.
//!
//! Drives sdm-engine in-process for one-shot commands (`sdm download <url>`
//! works with no daemon running). If a `sdmd` daemon is already up, queue
//! inspection commands talk to it over REST instead — see
//! docs/TECH_DECISIONS.md §6 for why the CLI and daemon are separate crates.
//!
//! Sprint 1 scope: `sdm download <url> [-o output]`.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "sdm", about = "SmartDownloadManager CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Download a single URL (runs the engine in-process — no daemon needed).
    Download {
        url: String,
        #[arg(short, long)]
        output: Option<String>,
        /// Number of connections, or "auto"
        #[arg(short, long, default_value = "auto")]
        connections: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Download { url, output, connections } => {
            tracing::info!(%url, ?output, %connections, "download requested (engine wiring lands end of Sprint 1)");
            println!("sdm: would download {url} (engine wiring in progress — see docs/SPRINT_PLAN.md Sprint 1)");
        }
    }

    Ok(())
}
