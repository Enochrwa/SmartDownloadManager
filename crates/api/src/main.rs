//! `sdm` — the SmartDownloadManager CLI, driving the headless engine.
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
    /// Download a single URL.
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
