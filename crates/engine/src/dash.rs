//! MPEG-DASH (`.mpd`) job orchestration (Sprint 9).
//!
//! A DASH manifest describes independent adaptation sets (video, audio,
//! sometimes subtitles) that a real player mixes together at playback
//! time. Per `docs/SPRINT_PLAN_PHASE2.md`, this sprint downloads the best
//! video representation and the best audio representation as two
//! separate files — muxing them into one container is Sprint 10's FFmpeg
//! remux step, same deferral as HLS's fMP4 case in `crate::hls`.
//!
//! One `jobs` row represents the whole logical download; `jobs.destination`
//! points at the video file, and the audio file lives alongside it with
//! `.video.` swapped for `.audio.` in the filename — see
//! [`audio_destination_for`]. This sprint's scope is VOD manifests with
//! `SegmentTemplate` addressing (`SegmentBase`/`SegmentList` and live
//! manifests aren't covered — see `sdm_protocols::dash`'s module docs).

use std::path::PathBuf;

use sdm_protocols::dash::{self, DashRepresentation, MediaKind, MPD};
use sdm_storage::{JobKind, JobStatus, ManifestSegmentRecord, SqlitePool};

use crate::duplicate::DuplicatePolicy;
use crate::error::EngineError;
use crate::job::Job;
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};

pub struct DashDownloadRequest {
    pub url: String,
    /// Directory the two output files are written into.
    pub destination_dir: PathBuf,
    /// Base filename (no extension) both outputs are derived from, e.g.
    /// `"movie"` -> `movie.video.mp4` / `movie.audio.mp4`.
    pub file_stem: String,
    pub duplicate_policy: DuplicatePolicy,
}

pub struct DashEngine<'a> {
    pool: &'a SqlitePool,
    client: &'a reqwest::Client,
}

/// Given the (possibly renamed-for-uniqueness) video destination, derive
/// the sibling audio destination by swapping the `.video.` filename
/// segment for `.audio.` — kept as a free function so resume can
/// recompute it from the persisted `jobs.destination` without needing a
/// second schema column.
pub fn audio_destination_for(video_destination: &std::path::Path) -> PathBuf {
    let name = video_destination
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let audio_name = if name.contains(".video.") {
        name.replacen(".video.", ".audio.", 1)
    } else {
        format!("{name}.audio")
    };
    video_destination
        .parent()
        .map(|p| p.join(&audio_name))
        .unwrap_or_else(|| PathBuf::from(&audio_name))
}

impl<'a> DashEngine<'a> {
    pub fn new(pool: &'a SqlitePool, client: &'a reqwest::Client) -> Self {
        Self { pool, client }
    }

    pub async fn start_download(
        &self,
        req: DashDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let video_destination = unique_destination(
            &req.destination_dir
                .join(format!("{}.video.mp4", req.file_stem)),
        );
        let audio_destination = audio_destination_for(&video_destination);

        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = video_destination.to_string_lossy().to_string();
        sdm_storage::insert_job_with_kind(self.pool, &job_id, &req.url, &dest_str, JobKind::Dash)
            .await?;

        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let manifest_text = self.fetch_text(&req.url).await?;
        let mpd = dash::parse_manifest(&manifest_text)?;
        let representations = dash::list_representations(&mpd);

        let video = dash::select_best(&representations, MediaKind::Video).ok_or_else(|| {
            EngineError::Other("DASH manifest has no video representation".to_string())
        })?;
        let audio = dash::select_best(&representations, MediaKind::Audio);

        let video_plan = dash::resolve_segments(
            &req.url,
            &mpd,
            video.period_index,
            video.adaptation_index,
            &video.representation_id,
        )?;
        let audio_plan = match audio {
            Some(a) => Some(dash::resolve_segments(
                &req.url,
                &mpd,
                a.period_index,
                a.adaptation_index,
                &a.representation_id,
            )?),
            None => None,
        };

        sdm_storage::insert_manifest_meta(
            self.pool,
            &job_id,
            "dash",
            &req.url,
            None,
            None,
            Some(&video.representation_id),
            audio.map(|a| a.representation_id.as_str()),
            false,
        )
        .await?;

        store_segment_plan(
            self.pool,
            &job_id,
            "video",
            &video_plan.init_url,
            &video_plan.media_urls,
        )
        .await?;
        if let Some(plan) = &audio_plan {
            store_segment_plan(
                self.pool,
                &job_id,
                "audio",
                &plan.init_url,
                &plan.media_urls,
            )
            .await?;
        }

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: None,
            connections: 1,
        });

        self.download_tracks(
            job_id,
            video_destination,
            audio_destination,
            audio.is_some(),
            progress,
        )
        .await
    }

    /// Resume: segment plans were already persisted, so this just
    /// re-drives the download loop against whatever's left un-downloaded
    /// (same "don't re-negotiate the manifest" philosophy as
    /// `crate::hls::HlsEngine::resume_download`).
    pub async fn resume_download(
        &self,
        job_id: String,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let meta = sdm_storage::get_manifest_meta(self.pool, &job_id)
            .await?
            .ok_or_else(|| {
                EngineError::Other(format!(
                    "job {job_id} has no manifest metadata to resume from"
                ))
            })?;

        let video_destination = PathBuf::from(&record.destination);
        let audio_destination = audio_destination_for(&video_destination);
        let has_audio = meta.audio_representation_id.is_some();

        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;

        self.download_tracks(
            job_id,
            video_destination,
            audio_destination,
            has_audio,
            progress,
        )
        .await
    }

    async fn fetch_text(&self, url: &str) -> Result<String, EngineError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("failed to fetch DASH manifest: {e}")))?;
        if !resp.status().is_success() {
            return Err(EngineError::Other(format!(
                "failed to fetch DASH manifest: HTTP {}",
                resp.status()
            )));
        }
        resp.text()
            .await
            .map_err(|e| EngineError::Other(format!("failed to read DASH manifest: {e}")))
    }

    async fn download_tracks(
        &self,
        job_id: String,
        video_destination: PathBuf,
        audio_destination: PathBuf,
        has_audio: bool,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let mut total_bytes = 0u64;

        let video_segments =
            sdm_storage::get_manifest_segments(self.pool, &job_id, "video").await?;
        total_bytes += self
            .download_and_concatenate(
                &job_id,
                "video",
                &video_segments,
                &video_destination,
                total_bytes,
                &progress,
            )
            .await?;

        if has_audio {
            let audio_segments =
                sdm_storage::get_manifest_segments(self.pool, &job_id, "audio").await?;
            total_bytes += self
                .download_and_concatenate(
                    &job_id,
                    "audio",
                    &audio_segments,
                    &audio_destination,
                    total_bytes,
                    &progress,
                )
                .await?;
        }

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Completed).await?;
        let _ = progress.send(ProgressEvent::Completed {
            job_id: job_id.clone(),
            destination: video_destination.to_string_lossy().to_string(),
            total_bytes,
        });

        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        Ok(record.into())
    }

    /// Download every not-yet-downloaded segment of one track, concatenate
    /// into `destination`, and return the number of bytes fetched *in this
    /// call* (already-downloaded segments from a prior run aren't
    /// re-counted, since `total_bytes_so_far` seeds the running total for
    /// progress events, not the return value).
    async fn download_and_concatenate(
        &self,
        job_id: &str,
        track: &str,
        segments: &[ManifestSegmentRecord],
        destination: &std::path::Path,
        total_bytes_so_far: u64,
        progress: &ProgressSender,
    ) -> Result<u64, EngineError> {
        let parts_dir = {
            let mut name = destination
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| track.to_string());
            name.push_str(".sdm-parts");
            destination
                .parent()
                .map(|p| p.join(&name))
                .unwrap_or_else(|| PathBuf::from(&name))
        };
        tokio::fs::create_dir_all(&parts_dir).await?;

        let running_total =
            std::sync::Arc::new(std::sync::atomic::AtomicU64::new(total_bytes_so_far));
        let mut track_bytes = 0u64;

        for seg in segments {
            let part_path = parts_dir.join(format!("{}-{:010}", seg.kind, seg.seq));
            if !seg.downloaded {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
                let job_id_owned = job_id.to_string();
                let progress_clone = progress.clone();
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

                let result =
                    sdm_protocols::download_single(self.client, &seg.url, &part_path, Some(tx))
                        .await;
                // See the matching comment in `crate::hls`: awaiting the
                // reporter task (rather than just dropping the handle)
                // guarantees every queued progress message has been
                // drained into `running_total` before it's read below.
                let _ = reporter.await;
                let written = result.map_err(EngineError::from)?;
                track_bytes += written;

                sdm_storage::mark_manifest_segment_downloaded(self.pool, &seg.id).await?;
                sdm_storage::update_job_downloaded_bytes(
                    self.pool,
                    job_id,
                    running_total.load(std::sync::atomic::Ordering::Relaxed) as i64,
                )
                .await?;
            } else if let Ok(m) = tokio::fs::metadata(&part_path).await {
                track_bytes += m.len();
            }
        }

        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let mut out = tokio::fs::File::create(destination).await?;
        let mut ordered = segments.to_vec();
        ordered.sort_by_key(|s| (if s.kind == "init" { 0 } else { 1 }, s.seq));
        for seg in &ordered {
            let part_path = parts_dir.join(format!("{}-{:010}", seg.kind, seg.seq));
            let mut part = tokio::fs::File::open(&part_path).await?;
            tokio::io::copy(&mut part, &mut out).await?;
        }
        let _ = tokio::fs::remove_dir_all(&parts_dir).await;

        Ok(track_bytes)
    }
}

async fn store_segment_plan(
    pool: &SqlitePool,
    job_id: &str,
    track: &str,
    init_url: &Option<String>,
    media_urls: &[String],
) -> anyhow::Result<()> {
    let mut rows: Vec<(String, i64, String)> = Vec::new();
    if let Some(init) = init_url {
        rows.push(("init".to_string(), 0, init.clone()));
    }
    for (i, url) in media_urls.iter().enumerate() {
        rows.push(("media".to_string(), i as i64, url.clone()));
    }
    sdm_storage::replace_manifest_segments(pool, job_id, track, &rows).await
}

// Re-exported so callers (CLI) can list representations/pick overrides
// without importing `sdm_protocols::dash` directly.
pub use dash::{list_representations, DashSegmentPlan};

/// Convenience used by the CLI's `list-remote`-style inspection (not a
/// download): parse a manifest and describe its representations in one
/// call, without needing a running `DashEngine`.
pub async fn describe_manifest(
    client: &reqwest::Client,
    url: &str,
) -> Result<(MPD, Vec<DashRepresentation>), EngineError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| EngineError::Other(format!("failed to fetch DASH manifest: {e}")))?;
    let text = resp
        .text()
        .await
        .map_err(|e| EngineError::Other(format!("failed to read DASH manifest: {e}")))?;
    let mpd = dash::parse_manifest(&text)?;
    let reps = dash::list_representations(&mpd);
    Ok((mpd, reps))
}
