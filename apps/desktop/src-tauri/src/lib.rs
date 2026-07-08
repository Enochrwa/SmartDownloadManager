//! Tauri shell: wires the React UI directly into sdm-engine, no separate
//! `sdmd` daemon process (Sprint 6 scope per docs/SPRINT_PLAN.md).
//!
//! On launch: repair the database if it's corrupt, take a fresh backup
//! snapshot, then auto-resume any job left mid-flight by an unclean
//! previous shutdown (session restore). A backup is then taken on an
//! hourly interval for the rest of the app's life.

mod commands;
mod dto;
mod state;

use std::sync::Arc;

use sdm_engine::Engine;
use state::{AppPaths, AppState};
use tauri::Manager;

/// Fixed loopback port the embedded Sprint 11 extension API listens on.
/// Not user-configurable (yet) since the browser extension needs a fixed,
/// documented address to point at by default; `SDM_EXTENSION_API_PORT`
/// can override it for local dev/testing without a rebuild.
const EXTENSION_API_PORT: u16 = 7890;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::add_download,
            commands::resume_job,
            commands::pause_job,
            commands::cancel_job,
            commands::remove_job,
            commands::list_jobs,
            commands::get_settings,
            commands::set_setting,
            commands::default_download_dir,
            commands::repair_database,
            commands::backup_now,
            commands::cleanup_orphans,
            commands::pairing_status,
            commands::pairing_issue_token,
            commands::pairing_revoke_token,
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                let paths = AppPaths::discover().expect("could not determine app data directory");
                tokio::fs::create_dir_all(&paths.app_dir)
                    .await
                    .expect("failed to create app data directory");
                tokio::fs::create_dir_all(&paths.default_download_dir)
                    .await
                    .ok();

                // Recovery, in order: repair a corrupt database (falling
                // back to the latest backup, or a fresh DB if none
                // exists), *then* open the pool against the now-healthy
                // file, then auto-resume whatever was mid-flight.
                let repair = sdm_engine::recovery::repair_database_if_corrupt(
                    &paths.db_path,
                    &paths.backup_dir,
                )
                .await
                .expect("database repair check failed");
                if !repair.integrity_errors.is_empty() {
                    tracing::warn!(
                        errors = ?repair.integrity_errors,
                        action = ?repair.action,
                        "database was corrupt on launch and has been repaired"
                    );
                }

                let pool = sdm_storage::connect(&paths.db_path.to_string_lossy())
                    .await
                    .expect("failed to open (post-repair) database");

                let state = Arc::new(AppState {
                    engine: Arc::new(Engine::new(pool.clone())),
                    pool,
                    running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
                    speed_tracker: tokio::sync::Mutex::new(std::collections::HashMap::new()),
                    paths,
                    extension_api_port: EXTENSION_API_PORT,
                });
                app_handle.manage(state.clone());

                // Take an on-launch snapshot so there's always at least
                // one recent backup even if the app is closed before the
                // first scheduled one fires.
                if let Err(e) = sdm_engine::recovery::backup_database(
                    &state.pool,
                    &state.paths.backup_dir,
                    sdm_engine::recovery::DEFAULT_BACKUP_RETENTION,
                )
                .await
                {
                    tracing::warn!("startup backup failed: {e}");
                }

                commands::auto_resume_interrupted_jobs(app_handle.clone(), state.clone());
                spawn_periodic_backups(state.clone());
                spawn_extension_api(state);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running SmartDownloadManager");
}

/// Snapshot the database once an hour for the rest of the app's life, so a
/// crash never loses more than ~an hour of queue/settings history even if
/// the corrupted-database repair path has to fall back to a backup.
fn spawn_periodic_backups(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        interval.tick().await; // first tick fires immediately; skip it, we just backed up
        loop {
            interval.tick().await;
            if let Err(e) = sdm_engine::recovery::backup_database(
                &state.pool,
                &state.paths.backup_dir,
                sdm_engine::recovery::DEFAULT_BACKUP_RETENTION,
            )
            .await
            {
                tracing::warn!("periodic backup failed: {e}");
            }
        }
    });
}

/// Sprint 11: mount `sdm-server`'s exact same router (job CRUD, WebSocket
/// progress, capture/pairing endpoints) on a loopback port, sharing this
/// process's engine/pool instead of opening a second `sdmd` daemon and a
/// second database connection. This is what makes the browser extension
/// work against the desktop app directly, with no separate `sdmd`
/// process required — see `crates/server/src/lib.rs`'s module doc for why
/// `build_router` takes an already-open pool/engine specifically to
/// support this.
fn spawn_extension_api(state: Arc<AppState>) {
    let port = std::env::var("SDM_EXTENSION_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(EXTENSION_API_PORT);

    tokio::spawn(async move {
        let router = sdm_server::build_router(
            state.pool.clone(),
            state.engine.clone(),
            state.paths.default_download_dir.clone(),
        );
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    port,
                    "extension API port unavailable ({e}); browser extension pairing will not \
                     work until this is freed (another sdmd instance already running?)"
                );
                return;
            }
        };
        tracing::info!(
            port,
            "extension API listening (Sprint 11 pairing + capture endpoints)"
        );
        if let Err(e) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        {
            tracing::warn!("extension API server stopped: {e}");
        }
    });
}
