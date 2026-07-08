//! Shared application state for the Tauri backend: the engine, the SQLite
//! pool, a registry of currently-running job tasks (so pause/cancel can
//! reach them), and the well-known filesystem paths the recovery routines
//! operate on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sdm_engine::Engine;
use sdm_storage::SqlitePool;
use tokio::sync::Mutex;

/// A currently-running download task, tracked so the frontend can pause or
/// cancel it. Aborting the handle stops the outer engine call; segment
/// state already journaled to SQLite (Sprint 3) is what makes the
/// subsequent resume correct, not any in-memory state here.
pub struct RunningJob {
    pub abort: tokio::task::AbortHandle,
}

pub struct AppState {
    pub pool: SqlitePool,
    pub engine: Arc<Engine>,
    pub running: Mutex<HashMap<String, RunningJob>>,
    /// Last (instant, downloaded_bytes) sample per job, used to compute an
    /// instantaneous speed for the `Progress` events streamed to the
    /// frontend (Sprint 6 live speed graph).
    pub speed_tracker: Mutex<HashMap<String, (std::time::Instant, u64)>>,
    pub paths: AppPaths,
    /// Sprint 11: the port the embedded extension API (`sdm-server`'s
    /// router, mounted in-process — see `lib.rs::spawn_extension_api`) is
    /// listening on, so Tauri commands can report it back to the
    /// settings panel's pairing UI.
    pub extension_api_port: u16,
}

#[derive(Clone)]
pub struct AppPaths {
    /// Directory holding jobs.db, backups/, and logs — `$SDM_HOME` for the
    /// desktop app (mirrors the CLI's convention in crates/cli).
    pub app_dir: PathBuf,
    pub db_path: PathBuf,
    pub backup_dir: PathBuf,
    /// Default destination directory for new downloads when the caller
    /// doesn't specify one.
    pub default_download_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> anyhow::Result<Self> {
        let app_dir = if let Ok(p) = std::env::var("SDM_HOME") {
            PathBuf::from(p)
        } else {
            dirs::data_dir()
                .ok_or_else(|| anyhow::anyhow!("could not determine app data directory"))?
                .join("SmartDownloadManager")
        };
        let default_download_dir =
            dirs::download_dir().unwrap_or_else(|| app_dir.join("downloads"));
        Ok(AppPaths {
            db_path: app_dir.join("jobs.db"),
            backup_dir: app_dir.join("backups"),
            default_download_dir,
            app_dir,
        })
    }
}
