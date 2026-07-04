//! sdm-torrent: BitTorrent/magnet support, built on `librqbit`.
//!
//! Implemented in Sprint 7 — see `docs/SPRINT_PLAN_PHASE2.md`. Crate
//! decision locked in `docs/TECH_DECISIONS.md` §4 (v1 magnets + `.torrent`
//! files only; DHT/PEX/tracker announce all come for free from `librqbit`
//! itself, this crate is a thin adapter onto the engine's job model).
//!
//! Scope for this sprint, matching the DoD in the sprint plan:
//! - magnet URI + `.torrent` file parsing ([`magnet`] module)
//! - adding a torrent to a shared [`librqbit::Session`] and polling its
//!   progress in the shape `crates/engine` already knows how to persist
//! - a sequential-piece-priority *toggle*, wired through to `librqbit`'s
//!   own streaming-priority mechanism (`TorrentStateLive::stream`) — full
//!   in-order streaming playback support is a Phase 3 concern, not this
//!   sprint's.
//!
//! Multi-connection segmented transfer (Sprint 2's HTTP model) doesn't
//! apply here: `librqbit` already parallelizes piece downloads across
//! swarm peers internally.

pub mod magnet;

use std::path::PathBuf;
use std::sync::Arc;

use librqbit::{AddTorrent, AddTorrentOptions, ManagedTorrent, Session, SessionOptions};

pub use magnet::{looks_like_torrent_source, parse_magnet, MagnetInfo, MagnetParseError};

/// `librqbit` exposes this as `ManagedTorrentHandle`, but that alias lives
/// in a private module and isn't reachable outside the crate — `Arc<ManagedTorrent>`
/// (the type it's aliasing) is exactly equivalent and *is* reachable, since
/// `ManagedTorrent` itself is re-exported at the crate root.
type TorrentHandleInner = Arc<ManagedTorrent>;

#[derive(Debug, thiserror::Error)]
pub enum TorrentError {
    #[error("magnet parse error: {0}")]
    Magnet(#[from] MagnetParseError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Where the torrent's data comes from.
#[derive(Debug, Clone)]
pub enum TorrentSource {
    Magnet(String),
    /// Raw bytes of a `.torrent` file (already read off disk by the
    /// caller, so this crate stays storage-backend agnostic).
    TorrentFile(Vec<u8>),
}

/// Mirrors `librqbit::TorrentStatsState` so callers of this crate never
/// need `librqbit` as a direct dependency just to match on job state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentState {
    Initializing,
    Live,
    Paused,
    Finished,
    Error,
}

#[derive(Debug, Clone)]
pub struct TorrentProgress {
    pub state: TorrentState,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub uploaded_bytes: u64,
    pub peer_count: usize,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub info_hash: String,
    pub name: Option<String>,
    pub piece_count: u32,
    pub total_bytes: u64,
    pub file_count: usize,
}

/// A shared `librqbit` session. One per `sdm-engine` process — individual
/// torrents are added to it via [`TorrentEngine::add`], the same way one
/// `reqwest::Client` in `sdm-protocols::http` backs every HTTP job.
pub struct TorrentEngine {
    session: Arc<Session>,
}

impl TorrentEngine {
    /// `default_output_folder` is used when a per-job output folder isn't
    /// given to [`add`](Self::add). DHT persistence and session
    /// fastresume are left off: `crates/engine` is the source of truth for
    /// job state (see `docs/TECH_DECISIONS.md` on not duplicating state
    /// across storage layers), so we don't need `librqbit`'s own on-disk
    /// session snapshot.
    pub async fn new(default_output_folder: impl Into<PathBuf>) -> Result<Self, TorrentError> {
        let session = Session::new_with_opts(
            default_output_folder.into(),
            SessionOptions {
                disable_dht_persistence: true,
                fastresume: false,
                persistence: None,
                ..Default::default()
            },
        )
        .await?;
        Ok(Self { session })
    }

    /// Add a torrent (magnet or `.torrent` bytes) and start downloading.
    ///
    /// `sequential` requests best-effort in-order piece priority for file
    /// 0 via `librqbit`'s streaming API — the "priority knob" called out
    /// in `docs/SPRINT_PLAN_PHASE2.md`, not a hard guarantee of strictly
    /// sequential arrival.
    pub async fn add(
        &self,
        source: TorrentSource,
        output_folder: Option<PathBuf>,
        only_files: Option<Vec<usize>>,
        sequential: bool,
    ) -> Result<TorrentHandle, TorrentError> {
        let add = match source {
            TorrentSource::Magnet(uri) => AddTorrent::from_url(uri),
            TorrentSource::TorrentFile(bytes) => AddTorrent::from_bytes(bytes),
        };
        let opts = AddTorrentOptions {
            output_folder: output_folder.map(|p| p.to_string_lossy().into_owned()),
            only_files,
            overwrite: true,
            ..Default::default()
        };
        let response = self.session.add_torrent(add, Some(opts)).await?;
        let handle = response
            .into_handle()
            .ok_or_else(|| anyhow::anyhow!("torrent add returned a list-only response"))?;

        if sequential {
            try_enable_sequential_priority(&handle);
        }

        Ok(TorrentHandle {
            session: self.session.clone(),
            handle,
        })
    }
}

/// Best-effort: register a streaming read-window over file 0, which nudges
/// `librqbit`'s piece scheduler to prefer that file's pieces in order.
/// Failure just means the torrent downloads in `librqbit`'s normal
/// rarest-first order instead — never fatal to the job.
fn try_enable_sequential_priority(handle: &TorrentHandleInner) {
    let Some(live) = handle.live() else {
        // Torrent hasn't reached the Live state yet (still fetching
        // metadata from a magnet link); nothing to prioritize yet.
        return;
    };
    match live.stream(0) {
        Ok(stream) => {
            // Registering the stream is what affects piece ordering; we
            // don't need to read from it ourselves. Leak it for the
            // lifetime of the process rather than threading a keep-alive
            // handle through storage — this sprint's scope is the
            // priority knob, not full streaming playback (Phase 3).
            std::mem::forget(stream);
        }
        Err(e) => {
            tracing::debug!(error = %e, "could not enable sequential piece priority");
        }
    }
}

/// A handle to one in-progress (or finished) torrent download.
pub struct TorrentHandle {
    session: Arc<Session>,
    handle: TorrentHandleInner,
}

impl TorrentHandle {
    pub fn info_hash(&self) -> String {
        self.handle.info_hash().as_string()
    }

    /// Metadata resolved from the swarm (name, piece/file counts, total
    /// size). `None` fields mean the torrent hasn't finished resolving
    /// metadata yet — normal right after adding a magnet link, before any
    /// peer has sent us the `.torrent` info dict.
    pub fn meta(&self) -> TorrentMeta {
        let metadata = self.handle.metadata.load();
        let Some(metadata) = metadata.as_ref() else {
            return TorrentMeta {
                info_hash: self.info_hash(),
                name: None,
                piece_count: 0,
                total_bytes: 0,
                file_count: 0,
            };
        };
        TorrentMeta {
            info_hash: self.info_hash(),
            name: metadata.name.clone(),
            piece_count: metadata.lengths.total_pieces(),
            total_bytes: metadata.lengths.total_length(),
            file_count: metadata
                .info
                .iter_file_details()
                .map(|files| files.count())
                .unwrap_or(0),
        }
    }

    pub fn progress(&self) -> TorrentProgress {
        let stats = self.handle.stats();
        let state = match stats.state {
            librqbit::TorrentStatsState::Initializing => TorrentState::Initializing,
            librqbit::TorrentStatsState::Live => TorrentState::Live,
            librqbit::TorrentStatsState::Paused => TorrentState::Paused,
            librqbit::TorrentStatsState::Error => TorrentState::Error,
        };
        let state = if stats.finished {
            TorrentState::Finished
        } else {
            state
        };
        let peer_count = stats
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live)
            .unwrap_or(0);
        TorrentProgress {
            state,
            downloaded_bytes: stats.progress_bytes,
            total_bytes: stats.total_bytes,
            uploaded_bytes: stats.uploaded_bytes,
            peer_count,
            finished: stats.finished,
            error: stats.error,
        }
    }

    /// Resolves once the torrent has fully downloaded all selected files.
    pub async fn wait_until_completed(&self) -> Result<(), TorrentError> {
        self.handle.wait_until_completed().await?;
        Ok(())
    }

    pub async fn pause(&self) -> Result<(), TorrentError> {
        self.session.pause(&self.handle).await?;
        Ok(())
    }

    pub async fn unpause(&self) -> Result<(), TorrentError> {
        self.session.unpause(&self.handle).await?;
        Ok(())
    }
}
