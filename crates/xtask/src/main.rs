//! `cargo xtask` — cross-platform dev/release automation.
//!
//! Why this exists instead of shell scripts: this project ships on Windows,
//! macOS, and Linux, and shell scripts don't run identically (or at all)
//! across those. `cargo xtask <command>` is just another Rust binary, so it
//! runs everywhere `cargo` does. Convention from the Rust community
//! (matklad's "cargo xtask" pattern) — no new tool dependency required.
//!
//! Run with: `cargo xtask <command>` (aliased in .cargo/config.toml).

use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Xtask {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the same checks CI runs: fmt, clippy, nextest.
    Check,
    /// Generate THIRD_PARTY_NOTICES.md from cargo-license + pnpm licenses output.
    Notices,
}

fn main() -> anyhow::Result<()> {
    let xtask = Xtask::parse();
    match xtask.command {
        Command::Check => {
            println!("xtask check: wiring lands alongside CI in Sprint 1 (docs/SPRINT_PLAN.md)");
        }
        Command::Notices => {
            println!("xtask notices: wiring lands before first release per docs/LICENSING.md");
        }
    }
    Ok(())
}
