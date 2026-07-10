//! sdm-server: the long-running daemon (`sdmd`).
//!
//! Owns the engine and exposes it over REST (queue CRUD) + WebSocket (live
//! progress events) so the desktop app's browser extension, a future
//! mobile client, or a headless NAS install can all drive the same engine
//! remotely. See docs/ARCHITECTURE.md and docs/TECH_DECISIONS.md §6 for
//! the rationale behind splitting this out from the CLI.
//!
//! - Sprint 5 scope (per docs/SPRINT_PLAN.md): originally just `/health`.
//! - Sprint 11 scope (per docs/SPRINT_PLAN_PHASE2.md): the browser
//!   extension needs somewhere real to talk to, so this crate now wires
//!   the router up to the actual engine — job CRUD, a WebSocket progress
//!   stream, extension "capture" endpoints (single + batch, both
//!   dedup-aware), and the pairing-token flow that authenticates the
//!   extension to `sdmd` in the first place.
//!
//! [`build_router`] takes an already-open `SqlitePool` + `Engine` so the
//! desktop app can mount the exact same router in-process (no second
//! daemon, no second database) alongside its existing Tauri commands —
//! see `apps/desktop/src-tauri/src/lib.rs`. The standalone `sdmd` binary
//! (`src/main.rs`) instead opens its own pool the same way `sdm-cli` does.

pub mod auth;
pub mod routes;
pub mod runner;
pub mod state;
pub mod util;

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use state::ServerState;

async fn health() -> &'static str {
    "ok"
}

/// Build the full router against an already-constructed engine/pool —
/// used by both the standalone `sdmd` binary and the desktop app's
/// in-process server (see module docs above).
pub fn build_router(
    pool: sdm_storage::SqlitePool,
    engine: Arc<sdm_engine::Engine>,
    default_download_dir: PathBuf,
) -> Router {
    let state = ServerState::new(pool, engine, default_download_dir);

    // CORS: browser extensions call this API from a `chrome-extension://`
    // / `moz-extension://` origin, which isn't a `http(s)://` origin at
    // all — tower-http's `AllowOrigin::mirror_request` reflects whatever
    // `Origin` header is present, which is what makes an extension's
    // `fetch()` calls work at all here. This is safe precisely *because*
    // every non-trivial route additionally requires a bearer pairing
    // token (or is loopback-gated) — CORS alone grants no access.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let protected = Router::new()
        .route(
            "/jobs",
            get(routes::jobs::list_jobs).post(routes::jobs::create_job),
        )
        .route(
            "/jobs/:id",
            get(routes::jobs::get_job).delete(routes::jobs::cancel_job),
        )
        .route("/jobs/:id/pause", post(routes::jobs::pause_job))
        .route("/jobs/:id/resume", post(routes::jobs::resume_job))
        .route("/capture", post(routes::capture::capture))
        .route("/capture/batch", post(routes::capture::capture_batch))
        .route("/search", get(routes::search::search))
        .route("/auth/cookies", post(routes::auth::import_cookies))
        .route("/ws/progress", get(routes::ws::ws_progress))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_pairing_token,
        ));

    let loopback_only = Router::new()
        .route("/pairing/tokens", post(routes::pairing::issue_token))
        .route("/pairing/revoke", post(routes::pairing::revoke_token))
        .route_layer(axum::middleware::from_fn(auth::require_loopback));

    let open = Router::new()
        .route("/health", get(health))
        .route("/pairing/verify", post(routes::pairing::verify_token))
        .route("/pairing/status", get(routes::pairing::status));

    Router::new()
        .merge(open)
        .merge(loopback_only)
        .merge(protected)
        .layer(cors)
        .with_state(state)
}

/// Standalone-binary entry point: discovers `$SDM_HOME` the same way
/// `sdm-cli` does, opens its own database, and builds the router — used
/// by `src/main.rs`. Kept separate from [`build_router`] so the desktop
/// app's embedded server never opens a second connection to the same
/// SQLite file.
pub async fn router_from_env() -> anyhow::Result<Router> {
    let home = sdm_home();
    tokio::fs::create_dir_all(&home).await.ok();
    let db_path = home.join("jobs.db");
    let pool = sdm_storage::connect(&db_path.to_string_lossy()).await?;
    let engine = Arc::new(sdm_engine::Engine::new(pool.clone()));
    let default_download_dir = download_dir_fallback(&home);
    tokio::fs::create_dir_all(&default_download_dir).await.ok();

    // Sprint 12: `sdmd` (unlike the in-process `sdm` CLI, which has no
    // long enough lifetime for interface-polling to be useful) runs the
    // VPN-detection monitor for as long as the daemon is up — see
    // `sdm_engine::vpn` for what "detection" means here and why it only
    // ever pauses, never auto-resumes.
    tokio::spawn(sdm_engine::VpnMonitor::new(pool.clone()).run());

    Ok(build_router(pool, engine, default_download_dir))
}

fn download_dir_fallback(home: &std::path::Path) -> PathBuf {
    // No `dirs` crate dependency here (it's only pulled in by the desktop
    // app) — a same-machine daemon defaults to `$SDM_HOME/downloads`,
    // which the CLI already treats as a sane default destination root.
    home.join("downloads")
}

fn sdm_home() -> PathBuf {
    if let Ok(p) = std::env::var("SDM_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".sdm")
}
