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
    /// Sprint 11: the *configured* port the embedded extension API tries
    /// to bind first. Kept separate from `extension_api_status` (the
    /// actual live outcome) because a bind failure may fall back to a
    /// nearby port — see `lib.rs::spawn_extension_api`.
    pub extension_api_port: u16,
    /// Live status of the embedded extension API's TCP listener, updated
    /// by `lib.rs::spawn_extension_api` and read by the `pairing_status`
    /// command. This is what fixes the "Couldn't reach sdmd at this
    /// address" bug: the pairing panel used to always report
    /// `extension_api_port` as if it were listening, even when the bind
    /// had silently failed (e.g. the port still held in `TIME_WAIT` from
    /// a just-restarted previous instance). Now it reports what's
    /// actually true.
    pub extension_api_status: tokio::sync::watch::Receiver<ExtensionApiStatus>,
}

/// Outcome of the embedded extension API's attempt to start listening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionApiStatus {
    /// Still binding/retrying — not an error yet.
    Starting,
    /// Actually bound and serving on this port (may differ from the
    /// configured `extension_api_port` if a fallback port was used).
    Listening { port: u16 },
    /// Every retry and fallback port was exhausted; the extension API is
    /// not reachable at all this session.
    Failed { error: String },
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
