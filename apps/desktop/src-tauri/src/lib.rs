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
                spawn_periodic_backups(state);
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
