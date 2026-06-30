//! sdm-server: the long-running daemon (`sdmd`).
//!
//! Owns the engine and exposes it over REST (queue CRUD) + WebSocket (live
//! progress events) so the desktop app's browser extension, a future mobile
//! client, or a headless NAS install can all drive the same engine remotely.
//! See docs/ARCHITECTURE.md and docs/TECH_DECISIONS.md §6 for the rationale
//! behind splitting this out from the CLI.
//!
//! Sprint 5 scope per docs/SPRINT_PLAN.md.

use axum::{routing::get, Router};

pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> &'static str {
    "ok"
}
