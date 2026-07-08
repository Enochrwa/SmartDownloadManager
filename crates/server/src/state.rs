//! Shared state for every `sdm-server` route: the engine + SQLite pool
//! (same shape as the desktop app's `AppState`, see
//! `apps/desktop/src-tauri/src/state.rs`), a registry of in-flight job
//! tasks so pause/cancel can reach them, and a broadcast channel every
//! `/ws/progress` WebSocket client subscribes to.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sdm_api_types::JobEvent;
use sdm_engine::Engine;
use sdm_storage::SqlitePool;
use tokio::sync::{broadcast, Mutex};

/// A currently-running download task, tracked so `POST /jobs/:id/pause`
/// and `DELETE /jobs/:id` can stop it — same rationale as the desktop
/// app's `RunningJob` (`apps/desktop/src-tauri/src/state.rs`).
pub struct RunningJob {
    pub abort: tokio::task::AbortHandle,
}

pub struct ServerState {
    pub pool: SqlitePool,
    pub engine: Arc<Engine>,
    pub running: Mutex<HashMap<String, RunningJob>>,
    pub speed_tracker: Mutex<HashMap<String, (std::time::Instant, u64)>>,
    /// Every `JobEvent` streamed to the frontend also goes out on this
    /// broadcast channel; `/ws/progress` subscribers get a fresh receiver
    /// each. Bounded so a slow/disconnected subscriber can't grow memory
    /// unboundedly — it'll see a `Lagged` error and can reconnect/re-sync
    /// via `GET /jobs` instead.
    pub progress_tx: broadcast::Sender<JobEvent>,
    /// A non-revoked pairing token seen within this many seconds counts
    /// as "connected" for `GET /pairing/status` (Sprint 11 DoD: a
    /// clipboard-copy round trip should complete within 2 seconds, so a
    /// generous but bounded window here keeps the indicator meaningful
    /// without flapping on every brief network hiccup).
    pub connected_window_secs: i64,
    /// Where a job with no explicit destination lands — mirrors
    /// `AppPaths::default_download_dir` in the desktop app.
    pub default_download_dir: PathBuf,
}

impl ServerState {
    pub fn new(pool: SqlitePool, engine: Arc<Engine>, default_download_dir: PathBuf) -> Arc<Self> {
        let (progress_tx, _rx) = broadcast::channel(1024);
        Arc::new(ServerState {
            pool,
            engine,
            running: Mutex::new(HashMap::new()),
            speed_tracker: Mutex::new(HashMap::new()),
            progress_tx,
            connected_window_secs: 120,
            default_download_dir,
        })
    }
}
