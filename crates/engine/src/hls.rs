//! HLS (`.m3u8`) job orchestration (Sprint 9).
//!
//! Modeled the same way `crate::ftp` is: HLS has no byte-range concurrency
//! model the way HTTP Range requests do (each segment is its own
//! complete HTTP resource), so a job here is "resolve the playlist to an
//! ordered segment list, download each one, concatenate." For MPEG-TS
//! segments (the overwhelmingly common case) straight concatenation
//! produces a directly playable file; for fMP4-segmented playlists the
//! init segment is prepended the same way, which is correct per-segment
//! but doesn't re-mux/repair timestamps across segment boundaries — that
//! polish is deferred to Sprint 10's FFmpeg remux step, same as DASH.
//!
//! Segment-level resume state (which segments already landed on disk)
//! is persisted via `sdm_storage::manifest_segments` rather than probed
//! from the filesystem, so a resumed job picks up exactly where it left
//! off even if the process was killed mid-segment.

use std::path::{Path, PathBuf};

use sdm_protocols::hls::{self, VariantSelector};
use sdm_storage::{JobKind, JobStatus, SqlitePool};

use crate::duplicate::DuplicatePolicy;
use crate::error::EngineError;
use crate::job::Job;
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};
use crate::verify::ExpectedChecksum;

/// A safety cap on how many times a still-live playlist gets re-polled,
/// so a job against a genuinely never-ending live stream doesn't run
/// forever unattended. Each poll waits roughly one target-duration
/// interval, so this is on the order of tens of minutes of continuous
/// capture — plenty for VOD-shaped Sprint 9 scope; a dedicated live-DVR
/// feature would replace this with an explicit stop condition.
const DEFAULT_MAX_LIVE_POLLS: u32 = 120;

pub struct HlsDownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    pub variant: VariantSelector,
    pub expected_checksum: Option<ExpectedChecksum>,
    pub duplicate_policy: DuplicatePolicy,
    /// Overrides [`DEFAULT_MAX_LIVE_POLLS`] — mainly so tests against a
    /// mock live playlist can bound the loop deterministically.
    pub max_live_polls: Option<u32>,
}

pub struct HlsEngine<'a> {
    pool: &'a SqlitePool,
    client: &'a reqwest::Client,
}

impl<'a> HlsEngine<'a> {
    pub fn new(pool: &'a SqlitePool, client: &'a reqwest::Client) -> Self {
        Self { pool, client }
    }

    pub async fn start_download(
        &self,
        req: HlsDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let destination = unique_destination(&req.destination);
        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = destination.to_string_lossy().to_string();
        sdm_storage::insert_job_with_kind(self.pool, &job_id, &req.url, &dest_str, JobKind::Hls)
            .await?;
        if let Some(expected) = &req.expected_checksum {
            sdm_storage::set_job_expected_checksum(
                self.pool,
                &job_id,
                expected.algorithm.as_str(),
                &expected.hex,
            )
            .await?;
        }

        self.run(job_id, req, destination, progress).await
    }

    /// Resume a job whose segment plan was already resolved and persisted
    /// — re-fetching the manifest is deliberately skipped so a resume
    /// doesn't reshuffle segment numbering out from under already
    /// downloaded parts; if the remote playlist truly changed, starting a
    /// fresh job is the correct move (same philosophy as
    /// `crate::torrent`'s resume, which reattaches to persisted piece
    /// state rather than re-negotiating from scratch).
    pub async fn resume_download(
        &self,
        job_id: String,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let destination = PathBuf::from(&record.destination);

        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;

        let segments = sdm_storage::get_manifest_segments(self.pool, &job_id, "single").await?;
        self.download_and_finish(
            job_id,
            destination,
            segments,
            record
                .checksum_algorithm
                .zip(record.checksum_expected)
                .map(|(algo, hex)| -> Result<ExpectedChecksum, EngineError> {
                    Ok(ExpectedChecksum {
                        algorithm: crate::verify::ChecksumAlgorithm::parse(&algo)
                            .map_err(EngineError::from)?,
                        hex,
                    })
                })
                .transpose()?,
            progress,
        )
        .await
    }

    async fn run(
        &self,
        job_id: String,
        req: HlsDownloadRequest,
        destination: PathBuf,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let (media_playlist_url, media, selected_desc) =
            self.resolve_media_playlist(&req.url, req.variant).await?;

        let is_live = hls::is_live(&media);
        let max_polls = req.max_live_polls.unwrap_or(DEFAULT_MAX_LIVE_POLLS);
        let (final_media_url, segments_meta) = if is_live {
            self.poll_until_ended(media_playlist_url, media, max_polls)
                .await?
        } else {
            (media_playlist_url, media)
        };

        let init_url = hls::init_segment_url(&final_media_url, &segments_meta)?;
        let media_segments = hls::media_segments(&final_media_url, &segments_meta)?;

        sdm_storage::insert_manifest_meta(
            self.pool,
            &job_id,
            "hls",
            &req.url,
            Some(&final_media_url),
            Some(&selected_desc),
            None,
            None,
            is_live,
        )
        .await?;

        let mut rows: Vec<(String, i64, String)> = Vec::new();
        if let Some(init) = &init_url {
            rows.push(("init".to_string(), 0, init.clone()));
        }
        for (i, seg) in media_segments.iter().enumerate() {
            rows.push(("media".to_string(), i as i64, seg.url.clone()));
        }
        sdm_storage::replace_manifest_segments(self.pool, &job_id, "single", &rows).await?;

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: None,
            connections: 1,
        });

        let segments = sdm_storage::get_manifest_segments(self.pool, &job_id, "single").await?;
        self.download_and_finish(
            job_id,
            destination,
            segments,
            req.expected_checksum,
            progress,
        )
        .await
    }

    /// Resolve `url` to a concrete media playlist: fetch it, and if it
    /// turns out to be a master playlist, select a variant and fetch
    /// *that* media playlist instead.
    async fn resolve_media_playlist(
        &self,
        url: &str,
        variant: VariantSelector,
    ) -> Result<(String, m3u8_rs::MediaPlaylist, String), EngineError> {
        let (fetched_url, bytes) = self.fetch(url).await?;
        let playlist = hls::parse(&bytes)?;
        match playlist {
            m3u8_rs::Playlist::MediaPlaylist(media) => {
                Ok((fetched_url, media, "(single quality)".to_string()))
            }
            m3u8_rs::Playlist::MasterPlaylist(master) => {
                let variants = hls::master_variants(&master);
                let chosen = hls::select_variant(&variants, variant).ok_or_else(|| {
                    EngineError::Other("HLS master playlist has no playable variants".to_string())
                })?;
                let desc = format!(
                    "{}kbps{}",
                    chosen.bandwidth / 1000,
                    chosen
                        .resolution
                        .map(|(w, h)| format!(" {w}x{h}"))
                        .unwrap_or_default()
                );
                let variant_url = hls::resolve_url(&fetched_url, &chosen.uri)?;
                let (media_url, media_bytes) = self.fetch(&variant_url).await?;
                let m3u8_rs::Playlist::MediaPlaylist(media) = hls::parse(&media_bytes)? else {
                    return Err(EngineError::Other(
                        "HLS variant playlist was itself a master playlist".to_string(),
                    ));
                };
                Ok((media_url, media, desc))
            }
        }
    }

    /// Keep re-fetching a live media playlist (waiting roughly one
    /// target-duration between polls) until `#EXT-X-ENDLIST` appears or
    /// `max_polls` is exhausted, returning the final snapshot.
    async fn poll_until_ended(
        &self,
        media_playlist_url: String,
        mut media: m3u8_rs::MediaPlaylist,
        max_polls: u32,
    ) -> Result<(String, m3u8_rs::MediaPlaylist), EngineError> {
        let mut polls = 0u32;
        while hls::is_live(&media) && polls < max_polls {
            let wait = std::time::Duration::from_secs(media.target_duration.max(1));
            tokio::time::sleep(wait).await;
            let (_, bytes) = self.fetch(&media_playlist_url).await?;
            let m3u8_rs::Playlist::MediaPlaylist(refreshed) = hls::parse(&bytes)? else {
                return Err(EngineError::Other(
                    "live HLS media playlist unexpectedly became a master playlist".to_string(),
                ));
            };
            media = refreshed;
            polls += 1;
        }
        Ok((media_playlist_url, media))
    }

    async fn fetch(&self, url: &str) -> Result<(String, Vec<u8>), EngineError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("failed to fetch {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(EngineError::Other(format!(
                "failed to fetch {url}: HTTP {}",
                resp.status()
            )));
        }
        let final_url = resp.url().to_string();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| EngineError::Other(format!("failed to read {url}: {e}")))?;
        Ok((final_url, bytes.to_vec()))
    }

    /// Download every not-yet-downloaded segment (in order), concatenate
    /// into `destination`, verify a checksum if one was supplied, and
    /// mark the job complete.
    async fn download_and_finish(
        &self,
        job_id: String,
        destination: PathBuf,
        segments: Vec<sdm_storage::ManifestSegmentRecord>,
        expected_checksum: Option<ExpectedChecksum>,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let parts_dir = parts_dir_for(&destination);
        tokio::fs::create_dir_all(&parts_dir).await?;

        let mut downloaded_bytes = 0u64;

        for seg in &segments {
            let part_path = parts_dir.join(format!("{}-{:010}", seg.kind, seg.seq));
            if !seg.downloaded {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
                let job_id_owned = job_id.clone();
                let progress_clone = progress.clone();
                let running_total =
                    std::sync::Arc::new(std::sync::atomic::AtomicU64::new(downloaded_bytes));
                let running_total_task = running_total.clone();
                let reporter = tokio::spawn(async move {
                    while let Some(n) = rx.recv().await {
                        let total = running_total_task
                            .fetch_add(n, std::sync::atomic::Ordering::Relaxed)
                            + n;
                        let _ = progress_clone.send(ProgressEvent::Progress {
                            job_id: job_id_owned.clone(),
                            downloaded_bytes: total,
                            total_bytes: None,
                        });
                    }
                });

                sdm_protocols::download_single(self.client, &seg.url, &part_path, Some(tx))
                    .await
                    .map_err(EngineError::from)?;
                // The channel's sender was moved into `download_single` and
                // is dropped when it returns, so awaiting the reporter task
                // here (rather than just dropping the handle) guarantees
                // every queued progress message has been drained into
                // `running_total` before we read it below.
                let _ = reporter.await;
                downloaded_bytes = running_total.load(std::sync::atomic::Ordering::Relaxed);

                sdm_storage::mark_manifest_segment_downloaded(self.pool, &seg.id).await?;
                sdm_storage::update_job_downloaded_bytes(
                    self.pool,
                    &job_id,
                    downloaded_bytes as i64,
                )
                .await?;
            } else if let Ok(meta) = tokio::fs::metadata(&part_path).await {
                downloaded_bytes += meta.len();
            }
        }

        concatenate_parts(&parts_dir, &segments, &destination).await?;
        let _ = tokio::fs::remove_dir_all(&parts_dir).await;

        if let Some(expected) = &expected_checksum {
            let _ = progress.send(ProgressEvent::Verifying {
                job_id: job_id.clone(),
            });
            let (actual, matches) = crate::verify::verify_file(&destination, expected).await?;
            sdm_storage::set_job_checksum_result(
                self.pool,
                &job_id,
                expected.algorithm.as_str(),
                &actual,
                matches,
            )
            .await?;
            if !matches {
                sdm_storage::set_job_error(
                    self.pool,
                    &job_id,
                    "checksum_mismatch",
                    "checksum mismatch",
                )
                .await?;
                let _ = progress.send(ProgressEvent::Failed {
                    job_id: job_id.clone(),
                    error_class: "checksum_mismatch".to_string(),
                    message: format!("expected {}, got {actual}", expected.hex),
                });
                return Err(EngineError::ChecksumMismatch {
                    algorithm: expected.algorithm.as_str().to_string(),
                    expected: expected.hex.clone(),
                    actual,
                });
            }
        }

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Completed).await?;
        let _ = progress.send(ProgressEvent::Completed {
            job_id: job_id.clone(),
            destination: destination.to_string_lossy().to_string(),
            total_bytes: downloaded_bytes,
        });

        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        Ok(record.into())
    }
}

fn parts_dir_for(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    name.push_str(".sdm-parts");
    destination
        .parent()
        .map(|p| p.join(&name))
        .unwrap_or_else(|| PathBuf::from(&name))
}

/// Concatenate init (if any) + media segment part files, in order, into
/// `destination`.
async fn concatenate_parts(
    parts_dir: &Path,
    segments: &[sdm_storage::ManifestSegmentRecord],
    destination: &Path,
) -> Result<(), EngineError> {
    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let mut out = tokio::fs::File::create(destination).await?;
    let mut ordered = segments.to_vec();
    // 'init' before 'media', then by seq — matches get_manifest_segments's
    // own ordering, re-sorted here defensively in case a caller passes an
    // already-filtered/reordered subset.
    ordered.sort_by(|a, b| {
        let kind_rank = |k: &str| if k == "init" { 0 } else { 1 };
        kind_rank(&a.kind)
            .cmp(&kind_rank(&b.kind))
            .then(a.seq.cmp(&b.seq))
    });
    for seg in &ordered {
        let part_path = parts_dir.join(format!("{}-{:010}", seg.kind, seg.seq));
        let mut part = tokio::fs::File::open(&part_path).await?;
        tokio::io::copy(&mut part, &mut out).await?;
    }
    Ok(())
}
