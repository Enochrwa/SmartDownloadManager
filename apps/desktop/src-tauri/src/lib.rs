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
use state::{AppPaths, AppState, ExtensionApiStatus};
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

                let (extension_api_status_tx, extension_api_status_rx) =
                    tokio::sync::watch::channel(ExtensionApiStatus::Starting);

                let state = Arc::new(AppState {
                    engine: Arc::new(Engine::new(pool.clone())),
                    pool,
                    running: tokio::sync::Mutex::new(std::collections::HashMap::new()),
                    speed_tracker: tokio::sync::Mutex::new(std::collections::HashMap::new()),
                    paths,
                    extension_api_port: EXTENSION_API_PORT,
                    extension_api_status: extension_api_status_rx,
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
                spawn_extension_api(state, extension_api_status_tx);
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

/// Number of times to retry binding the *configured* port (with backoff)
/// before falling back to scanning nearby ports. Covers the common case
/// that caused the "Couldn't reach sdmd at this address" bug: a just-
/// restarted previous instance's socket still sitting in `TIME_WAIT`,
/// which normally clears within a couple of seconds.
const BIND_RETRY_ATTEMPTS: u32 = 5;
/// How many ports above the configured one to try if the configured port
/// is genuinely unavailable (not just a transient `TIME_WAIT`) — e.g.
/// another application is permanently squatting on 7890.
const FALLBACK_PORT_SCAN_RANGE: u16 = 20;

/// Bind a loopback TCP listener with `SO_REUSEADDR` set, so a quick
/// app restart doesn't get rejected just because the previous process's
/// socket is still draining through `TIME_WAIT` — this was the root
/// cause of the pairing bug: `tokio::net::TcpListener::bind` alone does
/// not set this on all platforms, so a restart during development (or a
/// crash-and-relaunch) could leave the extension API silently dead for
/// up to a couple of minutes with no indication anywhere in the UI.
fn bind_reuseaddr(port: u16) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Socket, Type};
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    let socket = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

/// Sprint 11: mount `sdm-server`'s exact same router (job CRUD, WebSocket
/// progress, capture/pairing endpoints) on a loopback port, sharing this
/// process's engine/pool instead of opening a second `sdmd` daemon and a
/// second database connection. This is what makes the browser extension
/// work against the desktop app directly, with no separate `sdmd`
/// process required — see `crates/server/src/lib.rs`'s module doc for why
/// `build_router` takes an already-open pool/engine specifically to
/// support this.
///
/// Sprint 12 fix: previously a single failed bind attempt silently gave
/// up forever, and the settings panel always reported the *configured*
/// port as if it were live. Now: retry the configured port with
/// backoff + `SO_REUSEADDR` (covers the common `TIME_WAIT`-after-restart
/// case), fall back to scanning nearby ports if it's genuinely taken,
/// and publish the real outcome through `state.extension_api_status` so
/// `pairing_status` (and therefore the UI) reflects reality instead of
/// an optimistic constant.
fn spawn_extension_api(
    state: Arc<AppState>,
    status_tx: tokio::sync::watch::Sender<ExtensionApiStatus>,
) {
    let configured_port = std::env::var("SDM_EXTENSION_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(EXTENSION_API_PORT);

    tokio::spawn(async move {
        let router = sdm_server::build_router(
            state.pool.clone(),
            state.engine.clone(),
            state.paths.default_download_dir.clone(),
        );

        let mut last_err: Option<std::io::Error> = None;
        let mut std_listener: Option<std::net::TcpListener> = None;

        // First: retry the configured port with backoff. This is the
        // path that fixes the common case — a socket still draining
        // TIME_WAIT from a just-restarted previous instance.
        for attempt in 0..BIND_RETRY_ATTEMPTS {
            match bind_reuseaddr(configured_port) {
                Ok(l) => {
                    std_listener = Some(l);
                    break;
                }
                Err(e) => {
                    tracing::warn!(
                        port = configured_port,
                        attempt = attempt + 1,
                        "extension API bind failed, retrying: {e}"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(
                        200 * 2u64.pow(attempt),
                    ))
                    .await;
                }
            }
        }

        // Second: if the configured port is genuinely unavailable (not
        // just transient TIME_WAIT), scan a small range of nearby ports
        // rather than leaving the extension permanently unreachable.
        let mut bound_port = configured_port;
        if std_listener.is_none() {
            for offset in 1..=FALLBACK_PORT_SCAN_RANGE {
                let candidate = configured_port.saturating_add(offset);
                if let Ok(l) = bind_reuseaddr(candidate) {
                    tracing::warn!(
                        configured_port,
                        fallback_port = candidate,
                        "configured extension API port was unavailable; \
                         bound a fallback port instead"
                    );
                    std_listener = Some(l);
                    bound_port = candidate;
                    break;
                }
            }
        }

        let Some(std_listener) = std_listener else {
            let message = last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "no available port found".to_string());
            tracing::error!(
                port = configured_port,
                "extension API could not bind after retries and fallback \
                 scan ({message}); browser extension pairing will not work \
                 this session"
            );
            let _ = status_tx.send(ExtensionApiStatus::Failed { error: message });
            return;
        };

        let listener = match tokio::net::TcpListener::from_std(std_listener) {
            Ok(l) => l,
            Err(e) => {
                let message = format!("failed to hand bound socket to tokio: {e}");
                tracing::error!("{message}");
                let _ = status_tx.send(ExtensionApiStatus::Failed { error: message });
                return;
            }
        };

        tracing::info!(
            port = bound_port,
            "extension API listening (Sprint 11 pairing + capture endpoints)"
        );
        let _ = status_tx.send(ExtensionApiStatus::Listening { port: bound_port });

        if let Err(e) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        {
            let message = format!("extension API server stopped: {e}");
            tracing::warn!("{message}");
            let _ = status_tx.send(ExtensionApiStatus::Failed { error: message });
        }
    });
}
