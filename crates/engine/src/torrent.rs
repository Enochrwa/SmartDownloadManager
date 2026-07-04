//! BitTorrent job orchestration (Sprint 7).
//!
//! `sdm_torrent` (a thin wrapper around `librqbit`) does the actual swarm
//! work — DHT, PEX, tracker announces, piece scheduling. This module's job
//! is to plug that into the same `Job`/SQLite/`ProgressEvent` contract
//! every other job kind uses: create the job row + `torrent_meta` row,
//! poll `librqbit`'s stats on an interval, and persist/emit progress the
//! same way the HTTP and FTP engines do.

use std::path::PathBuf;

use sdm_storage::{JobKind, JobStatus, SqlitePool};
use sdm_torrent::{TorrentEngine as RqbitEngine, TorrentSource, TorrentState};

use crate::error::EngineError;
use crate::job::Job;
use crate::progress::{ProgressEvent, ProgressSender};

/// How often to poll `librqbit` for updated stats and persist them.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

pub struct TorrentDownloadRequest {
    /// A `magnet:?...` URI or a filesystem path to a `.torrent` file —
    /// disambiguated the same way the CLI does, via
    /// `sdm_torrent::looks_like_torrent_source`.
    pub source: String,
    pub destination_folder: PathBuf,
    /// Only download these file indices (from the torrent's file list);
    /// `None` downloads everything.
    pub only_files: Option<Vec<usize>>,
    /// Best-effort in-order piece priority for file 0 (see
    /// `sdm_torrent::TorrentEngine::add` docs on what this does and
    /// doesn't guarantee).
    pub sequential: bool,
}

pub struct TorrentJobRunner<'a> {
    pool: &'a SqlitePool,
    rqbit: &'a RqbitEngine,
}

impl<'a> TorrentJobRunner<'a> {
    pub fn new(pool: &'a SqlitePool, rqbit: &'a RqbitEngine) -> Self {
        Self { pool, rqbit }
    }

    pub async fn start_download(
        &self,
        req: TorrentDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let (source, magnet_uri, torrent_file_path) = if req.source.starts_with("magnet:?") {
            (
                TorrentSource::Magnet(req.source.clone()),
                Some(req.source.clone()),
                None,
            )
        } else {
            let bytes = tokio::fs::read(&req.source).await?;
            (
                TorrentSource::TorrentFile(bytes),
                None,
                Some(req.source.clone()),
            )
        };

        let info_from_magnet = magnet_uri
            .as_deref()
            .and_then(|m| sdm_torrent::parse_magnet(m).ok());

        // Skip starting a duplicate download of a magnet we already know
        // the info-hash for (the torrent analogue of Sprint 4's URL/hash
        // duplicate detection). `.torrent` files resolve their info-hash
        // only after `librqbit` parses them below, so this check only
        // fires for magnets.
        if let Some(info) = &info_from_magnet {
            if let Some(existing) =
                sdm_storage::find_job_by_info_hash(self.pool, &info.info_hash).await?
            {
                return Ok(existing.into());
            }
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = req.destination_folder.to_string_lossy().to_string();
        sdm_storage::insert_job_with_kind(
            self.pool,
            &job_id,
            &req.source,
            &dest_str,
            JobKind::Torrent,
        )
        .await?;

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: None,
            connections: 1,
        });

        let handle = self
            .rqbit
            .add(
                source,
                Some(req.destination_folder.clone()),
                req.only_files.clone(),
                req.sequential,
            )
            .await?;

        // Only known for certain once `librqbit` has parsed the torrent
        // (immediately for `.torrent` files; magnets already gave it to us
        // via `info_from_magnet`, but we trust the resolved handle as the
        // source of truth either way).
        let meta = handle.meta();
        let only_files_json = req
            .only_files
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        sdm_storage::insert_torrent_meta(
            self.pool,
            &job_id,
            &handle.info_hash(),
            magnet_uri.as_deref(),
            torrent_file_path.as_deref(),
            meta.name.as_deref().or(info_from_magnet
                .as_ref()
                .and_then(|i| i.display_name.as_deref())),
            req.sequential,
            only_files_json.as_deref(),
        )
        .await?;

        self.poll_until_done(&job_id, &handle, &req.destination_folder, progress)
            .await
    }

    /// Resume a previously-started torrent job. BitTorrent resume is
    /// mostly free: re-adding the same magnet/`.torrent` points
    /// `librqbit` at the same output folder, where it re-hashes whatever
    /// pieces already landed on disk and only fetches what's missing —
    /// unlike HTTP/FTP, there's no manual byte-offset bookkeeping needed.
    pub async fn resume_download(
        &self,
        job_id: String,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let meta = sdm_storage::get_torrent_meta(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let destination_folder = PathBuf::from(&record.destination);

        let source_str = meta
            .magnet_uri
            .clone()
            .or(meta.torrent_file_path.clone())
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let only_files: Option<Vec<usize>> = meta
            .only_files
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let source = if source_str.starts_with("magnet:?") {
            TorrentSource::Magnet(source_str)
        } else {
            let bytes = tokio::fs::read(&source_str).await?;
            TorrentSource::TorrentFile(bytes)
        };

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;
        let handle = self
            .rqbit
            .add(
                source,
                Some(destination_folder.clone()),
                only_files,
                meta.sequential,
            )
            .await?;

        self.poll_until_done(&job_id, &handle, &destination_folder, progress)
            .await
    }

    async fn poll_until_done(
        &self,
        job_id: &str,
        handle: &sdm_torrent::TorrentHandle,
        destination_folder: &std::path::Path,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        loop {
            let p = handle.progress();
            sdm_storage::update_job_downloaded_bytes(self.pool, job_id, p.downloaded_bytes as i64)
                .await?;

            let meta = handle.meta();
            sdm_storage::update_torrent_swarm_info(
                self.pool,
                job_id,
                Some(meta.piece_count as i64),
                Some(meta.file_count as i64),
                p.peer_count as i64,
            )
            .await?;

            let _ = progress.send(ProgressEvent::Progress {
                job_id: job_id.to_string(),
                downloaded_bytes: p.downloaded_bytes,
                total_bytes: Some(p.total_bytes).filter(|&t| t > 0),
            });

            match p.state {
                TorrentState::Finished => {
                    sdm_storage::set_job_status(self.pool, job_id, JobStatus::Completed).await?;
                    let _ = progress.send(ProgressEvent::Completed {
                        job_id: job_id.to_string(),
                        destination: destination_folder.to_string_lossy().to_string(),
                        total_bytes: p.total_bytes,
                    });
                    break;
                }
                TorrentState::Error => {
                    let message = p
                        .error
                        .unwrap_or_else(|| "unknown torrent error".to_string());
                    sdm_storage::set_job_error(self.pool, job_id, "torrent_error", &message)
                        .await?;
                    let _ = progress.send(ProgressEvent::Failed {
                        job_id: job_id.to_string(),
                        error_class: "torrent_error".to_string(),
                        message,
                    });
                    break;
                }
                _ => {
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }

        let record = sdm_storage::get_job(self.pool, job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.to_string()))?;
        Ok(record.into())
    }
}
