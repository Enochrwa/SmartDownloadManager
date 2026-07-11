//! yt-dlp-backed video/audio extraction (Sprint 10).
//!
//! Modeled the same way `crate::hls`/`crate::dash` are: this module owns
//! the "resolve to a concrete plan, then execute it" orchestration, while
//! the actual subprocess work is delegated — here to `sdm_media`'s
//! `YtDlpClient`/`FfmpegClient` rather than an HTTP client.
//!
//! ## Playlist expansion
//! A playlist/channel/album/podcast URL is detected via
//! `YtDlpClient::probe_playlist` returning more than one entry (a single
//! video also comes back as one flat-playlist entry, so "more than one"
//! is the actual playlist signal, not "any entries at all"). When it is
//! one, [`MediaEngine::start_download`] creates one parent Job plus N
//! child Jobs (`sdm_storage::insert_child_job_with_kind`,
//! `parent_job_id`) and processes each child in turn — see the migration
//! comment in `crates/storage/migrations/0005_sprint10.sql` for why this
//! is a direct self-referencing link rather than a dedicated queue/
//! category system (Sprint 5's queue/category system was never actually
//! built in this codebase).
//!
//! ## Quality selection and the merge step
//! A concrete `format_id` is always fetched (never a heuristic selector
//! string) so "fetch the requested format, not just the default" is
//! directly verifiable. If the chosen format lacks either an audio or
//! video stream (the common case for anything above yt-dlp's lowest
//! "already-muxed" tiers), a second fetch pulls the best matching
//! counterpart stream and `sdm_media::FfmpegClient::merge_audio_video`
//! muxes them — this is Sprint 10's actual "FFmpeg subprocess wrapper"
//! deliverable, not just yt-dlp's own internal merge.
//!
//! ## Livestreams
//! `VideoMetadata::is_livestream` routes the fetch through yt-dlp's
//! `--live-from-start` (an ongoing-capture download from stream start)
//! rather than treating the job as fixed-length.

use std::path::{Path, PathBuf};

use sdm_media::{
    FfmpegBinary, FfmpegClient, FormatInfo, SubtitleFormat, SubtitleTrack, VideoMetadata,
    YtDlpBinary, YtDlpClient, YtDlpEvent, YtDlpFetchRequest,
};
use sdm_storage::{JobKind, JobStatus, SqlitePool};

use crate::duplicate::DuplicatePolicy;
use crate::error::EngineError;
use crate::job::Job;
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};

/// Hostnames/URL substrings recognized as yt-dlp-capable media sources
/// without needing a live probe. This is the single authoritative list —
/// `crates/cli/src/main.rs`, `crates/server/src/routes`, and the desktop
/// app's Tauri commands all call [`looks_like_media_url`]/
/// [`detect_media_source`] rather than keeping their own copies, so
/// "paste a link, get the video" behaves identically everywhere the
/// engine is embedded.
///
/// Deliberately not exhaustive — yt-dlp itself supports thousands of
/// extractors we have no reliable way to enumerate or keep in sync with
/// here. Anything not on this list falls through to
/// [`probe_is_media`]'s live yt-dlp probe instead, which is what makes
/// "capture any link" actually mean *any* link rather than just this
/// shortlist.
pub const KNOWN_MEDIA_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "m.youtube.com",
    "music.youtube.com",
    "vimeo.com",
    "player.vimeo.com",
    "dailymotion.com",
    "dai.ly",
    "twitch.tv",
    "clips.twitch.tv",
    "soundcloud.com",
    "tiktok.com",
    "x.com/",
    "twitter.com/",
    "facebook.com/watch",
    "fb.watch",
    "instagram.com/reel",
    "instagram.com/p/",
    "instagram.com/tv/",
    "reddit.com/r/",
    "v.redd.it",
    "streamable.com",
    "bilibili.com",
    "bandcamp.com",
    "vk.com/video",
    "rumble.com",
    "odysee.com",
    "ted.com/talks",
    "vk.com/clip",
    "niconico.jp",
    "nicovideo.jp",
    "mixcloud.com",
    "crunchyroll.com",
    "pbs.org/video",
];

/// Cheap, synchronous check against [`KNOWN_MEDIA_HOSTS`] — no
/// subprocess involved. Used as the fast path before falling back to
/// [`probe_is_media`] for hosts this doesn't recognize.
pub fn looks_like_media_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    KNOWN_MEDIA_HOSTS.iter().any(|h| lower.contains(h))
}

/// True if the URL's final path segment already looks like an ordinary
/// filename (`report.pdf`, `archive.zip`, `data.bin`, `firmware.abc123`
/// — anything with a short alphanumeric suffix after the last dot).
///
/// This is deliberately broad rather than matching a fixed extension
/// list: the whole point is to keep [`probe_is_media`]'s yt-dlp
/// subprocess off the hot path for ordinary downloads. A modern video
/// page's URL essentially never has an extension in its final path
/// segment (`/watch?v=…`, `/reel/…`, `/video/123`); a direct file link
/// —including ones with an extension this crate has never heard of—
/// essentially always does. Treating "has *some* short extension" as
/// "definitely not worth probing" is far cheaper and, in practice, no
/// less accurate than maintaining an enumerated list, and it means
/// ordinary large-file downloads with an unusual extension never pay a
/// multi-second subprocess-probe tax before starting.
fn has_direct_file_extension(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last_segment = path.rsplit('/').next().unwrap_or(path);
    match last_segment.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && (1..=8).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            true
        }
        _ => false,
    }
}

/// Best-effort universal fallback for hosts not on [`KNOWN_MEDIA_HOSTS`]:
/// a lightweight yt-dlp metadata probe (`--dump-json`, no actual
/// download) with a short timeout. This is what lets "capture any link"
/// genuinely mean any link rather than a fixed shortlist — yt-dlp itself
/// recognizes several thousand sites we can't enumerate up front.
///
/// Returns `false` — never blocks a regular HTTP download — on timeout,
/// probe failure (yt-dlp exits non-zero because the site isn't
/// supported, which is the overwhelmingly common case for a random
/// non-video URL), or a response that reports no downloadable formats at
/// all.
pub async fn probe_is_media(url: &str, ytdlp: &YtDlpBinary) -> bool {
    let client = YtDlpClient::new(ytdlp.clone());
    match tokio::time::timeout(std::time::Duration::from_secs(10), client.probe(url)).await {
        Ok(Ok(meta)) => !meta.formats.is_empty(),
        _ => false,
    }
}

/// The combined "is this any-link a capturable video/audio source"
/// check: known-host fast path, then (for anything with no obvious
/// direct-file extension) a live yt-dlp probe. Callers that already know
/// they want the probe path unconditionally (e.g. an explicit
/// "via yt-dlp" override in a UI) can skip straight to
/// [`probe_is_media`] instead.
pub async fn detect_media_source(url: &str, ytdlp: &YtDlpBinary) -> bool {
    if looks_like_media_url(url) {
        return true;
    }
    if has_direct_file_extension(url) {
        return false;
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    probe_is_media(url, ytdlp).await
}

/// Which format to fetch.
#[derive(Debug, Clone)]
pub enum QualitySelector {
    /// Highest-resolution video format available (ties broken by
    /// bitrate), matched with the best available audio if the chosen
    /// format doesn't already carry one.
    Best,
    /// An exact `format_id` from a prior probe's
    /// [`sdm_media::VideoMetadata::formats`] — the caller (CLI/UI)
    /// showed the user a concrete list and this is what they picked.
    FormatId(String),
}

pub struct MediaDownloadRequest {
    pub url: String,
    pub destination_dir: PathBuf,
    pub quality: QualitySelector,
    /// Subtitle language codes to fetch and embed (e.g. `["en", "rw"]`);
    /// empty means none.
    pub subtitle_langs: Vec<String>,
    pub embed_thumbnail: bool,
    pub duplicate_policy: DuplicatePolicy,
    pub ytdlp: YtDlpBinary,
    pub ffmpeg: FfmpegBinary,
}

pub struct MediaEngine<'a> {
    pool: &'a SqlitePool,
}

impl<'a> MediaEngine<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Start a new media download: expands a playlist into child jobs if
    /// `req.url` is one, otherwise processes it as a single video. Always
    /// returns the top-level [`Job`] the caller should track — for a
    /// playlist that's the parent (see `sdm_storage::list_child_jobs` to
    /// enumerate its children).
    pub async fn start_download(
        &self,
        req: MediaDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let ytdlp = YtDlpClient::new(req.ytdlp.clone());

        let entries = ytdlp.probe_playlist(&req.url).await.map_err(media_err)?;

        if entries.len() <= 1 {
            let job_id = uuid::Uuid::new_v4().to_string();
            return self
                .process_single_video(job_id, None, &req, &ytdlp, progress, None)
                .await;
        }

        // Playlist: one parent Job (a container — no media_meta of its
        // own) plus N child Jobs, one per entry.
        let parent_id = uuid::Uuid::new_v4().to_string();
        let parent_dest = req.destination_dir.join(format!("playlist-{parent_id}"));
        sdm_storage::insert_job_with_kind(
            self.pool,
            &parent_id,
            &req.url,
            &parent_dest.to_string_lossy(),
            JobKind::Media,
        )
        .await
        .map_err(EngineError::from)?;
        sdm_storage::set_job_status(self.pool, &parent_id, JobStatus::Downloading)
            .await
            .map_err(EngineError::from)?;

        for entry in &entries {
            let child_id = uuid::Uuid::new_v4().to_string();
            let child_url = entry
                .url
                .clone()
                .or_else(|| entry.id.clone())
                .ok_or_else(|| {
                    EngineError::Other(
                        "playlist entry had neither a url nor an id to fetch".to_string(),
                    )
                })?;
            sdm_storage::insert_child_job_with_kind(
                self.pool,
                &child_id,
                &child_url,
                &req.destination_dir.to_string_lossy(),
                JobKind::Media,
                &parent_id,
            )
            .await
            .map_err(EngineError::from)?;

            // Best-effort per child: one broken video in a 200-video
            // channel shouldn't abort the other 199. Failures are
            // recorded on that child's own job row (set_job_error) and
            // the loop continues.
            if let Err(e) = self
                .process_single_video(
                    child_id.clone(),
                    Some(child_url.clone()),
                    req_for_child(&req, &child_url),
                    &ytdlp,
                    progress.clone(),
                    None,
                )
                .await
            {
                let _ = sdm_storage::set_job_error(
                    self.pool,
                    &child_id,
                    "media_fetch_failed",
                    &e.to_string(),
                )
                .await;
            }
        }

        sdm_storage::set_job_status(self.pool, &parent_id, JobStatus::Completed)
            .await
            .map_err(EngineError::from)?;
        let record = sdm_storage::get_job(self.pool, &parent_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::JobNotFound(parent_id.clone()))?;
        Ok(record.into())
    }

    /// Resume a media job that was interrupted mid-download. Unlike
    /// [`Self::start_download`], this doesn't take a
    /// [`MediaDownloadRequest`] — it re-derives the exact original
    /// format/subtitle/thumbnail choices from `media_meta` (persisted by
    /// [`Self::process_single_video`] before the fetch itself begins, so
    /// they survive whatever interrupted the job) and reuses the job's
    /// exact recorded destination path rather than recomputing one, so
    /// this genuinely continues the same job instead of quietly
    /// producing a second, differently-named file alongside it.
    ///
    /// yt-dlp itself resumes/continues a partial single-file fetch by
    /// default; this re-drives that same fetch (and, for a
    /// video-only+audio-only pair, both fetches then the merge again —
    /// yt-dlp will skip already-complete bytes on each) rather than
    /// implementing byte-range resume logic of our own on top of it.
    pub async fn resume_download(
        &self,
        job_id: String,
        ytdlp_binary: YtDlpBinary,
        ffmpeg_binary: FfmpegBinary,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        if record.job_kind != JobKind::Media {
            return Err(EngineError::Other(format!(
                "job {job_id} is not a media (yt-dlp) job"
            )));
        }
        let meta = sdm_storage::get_media_meta(self.pool, &job_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| {
                EngineError::Other(format!(
                    "job {job_id} has no recorded media_meta (it may predate resumable \
                     media downloads, or is a playlist parent — resume its child jobs \
                     individually instead)"
                ))
            })?;
        let format_id = meta.selected_format_id.clone().ok_or_else(|| {
            EngineError::Other(format!(
                "job {job_id}'s media_meta has no recorded format_id to resume with"
            ))
        })?;
        let subtitle_langs: Vec<String> = meta
            .subtitle_langs_json
            .as_deref()
            .map(|s| serde_json::from_str(s).unwrap_or_default())
            .unwrap_or_default();
        let destination = PathBuf::from(&record.destination);
        let destination_dir = destination
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let req = MediaDownloadRequest {
            url: record.url.clone(),
            destination_dir,
            quality: QualitySelector::FormatId(format_id),
            subtitle_langs,
            embed_thumbnail: meta.embed_thumbnail,
            // Irrelevant here: destination_override below bypasses the
            // duplicate-policy naming logic entirely.
            duplicate_policy: DuplicatePolicy::Overwrite,
            ytdlp: ytdlp_binary,
            ffmpeg: ffmpeg_binary,
        };
        let ytdlp = YtDlpClient::new(req.ytdlp.clone());
        self.process_single_video(job_id, None, &req, &ytdlp, progress, Some(destination))
            .await
    }

    async fn process_single_video(
        &self,
        job_id: String,
        pre_existing_url: Option<String>,
        req: impl std::borrow::Borrow<MediaDownloadRequest>,
        ytdlp: &YtDlpClient,
        progress: ProgressSender,
        destination_override: Option<PathBuf>,
    ) -> Result<Job, EngineError> {
        let req = req.borrow();
        let url = pre_existing_url.unwrap_or_else(|| req.url.clone());
        let is_new_job = sdm_storage::get_job(self.pool, &job_id)
            .await
            .map_err(EngineError::from)?
            .is_none();

        let metadata = ytdlp.probe(&url).await.map_err(media_err)?;

        let title = metadata
            .title
            .clone()
            .unwrap_or_else(|| "video".to_string());
        let extension = metadata
            .formats
            .first()
            .and_then(|f| f.ext.clone())
            .unwrap_or_else(|| "mp4".to_string());
        let destination = if let Some(fixed) = destination_override {
            // Resuming an existing job: reuse its exact recorded
            // destination rather than recomputing one from a fresh
            // title/extension probe (which could theoretically differ
            // slightly and would otherwise duplicate-rename alongside
            // the original instead of continuing it).
            fixed
        } else {
            let candidate_destination = req
                .destination_dir
                .join(format!("{}.{extension}", sanitize_filename(&title)));
            if req.duplicate_policy == DuplicatePolicy::Skip && candidate_destination.exists() {
                return Err(EngineError::DuplicateSkipped {
                    existing_job_id: job_id,
                });
            }
            match req.duplicate_policy {
                DuplicatePolicy::Overwrite => candidate_destination,
                DuplicatePolicy::Skip | DuplicatePolicy::Rename => {
                    unique_destination(&candidate_destination)
                }
            }
        };

        if is_new_job {
            sdm_storage::insert_job_with_kind(
                self.pool,
                &job_id,
                &url,
                &destination.to_string_lossy(),
                JobKind::Media,
            )
            .await
            .map_err(EngineError::from)?;
        }
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Probing)
            .await
            .map_err(EngineError::from)?;
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let chosen = choose_format(&metadata, &req.quality)?;
        let is_livestream = metadata.is_livestream();

        persist_media_meta(self.pool, &job_id, &metadata, &chosen.format_id, req).await?;

        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading)
            .await
            .map_err(EngineError::from)?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: chosen.filesize.or(chosen.filesize_approx),
            connections: 1,
        });

        let ffmpeg = FfmpegClient::new(req.ffmpeg.clone());
        let work_dir = work_dir_for(&destination);
        tokio::fs::create_dir_all(&work_dir)
            .await
            .map_err(EngineError::from)?;

        let needs_merge = !(chosen.has_video() && chosen.has_audio());
        let primary_stem = work_dir.join("primary");
        let primary_req = YtDlpFetchRequest {
            url: url.clone(),
            format_id: chosen.format_id.clone(),
            output_stem: primary_stem.clone(),
            subtitle_langs: req.subtitle_langs.clone(),
            subtitle_format: SubtitleFormat::Srt,
            write_thumbnail: req.embed_thumbnail,
            live_from_start: is_livestream,
        };
        let primary_outcome = run_fetch(ytdlp, &primary_req, &job_id, &progress).await?;

        let mut current_path = primary_outcome.media_path.clone();

        if needs_merge {
            let audio_format = metadata
                .audio_formats()
                .into_iter()
                .filter(|f| f.format_id != chosen.format_id)
                .max_by(|a, b| {
                    a.tbr
                        .unwrap_or(0.0)
                        .partial_cmp(&b.tbr.unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .ok_or_else(|| {
                    EngineError::Other(format!(
                        "selected format {} has no audio and no separate audio format was found to merge",
                        chosen.format_id
                    ))
                })?
                .clone();

            let audio_stem = work_dir.join("audio");
            let audio_req = YtDlpFetchRequest {
                url: url.clone(),
                format_id: audio_format.format_id.clone(),
                output_stem: audio_stem,
                subtitle_langs: vec![],
                subtitle_format: SubtitleFormat::Srt,
                write_thumbnail: false,
                live_from_start: is_livestream,
            };
            let audio_outcome = run_fetch(ytdlp, &audio_req, &job_id, &progress).await?;

            let merged_path = work_dir.join(format!("merged.{extension}"));
            ffmpeg
                .merge_audio_video(&current_path, &audio_outcome.media_path, &merged_path)
                .await
                .map_err(media_err)?;
            current_path = merged_path;
        }

        if !req.subtitle_langs.is_empty() && !primary_outcome.subtitle_paths.is_empty() {
            let tracks: Vec<SubtitleTrack> = primary_outcome
                .subtitle_paths
                .iter()
                .map(|(lang, path)| SubtitleTrack {
                    lang: lang.clone(),
                    path: path.clone(),
                    format: SubtitleFormat::Srt,
                })
                .collect();
            let with_subs = work_dir.join(format!("with_subs.{extension}"));
            ffmpeg
                .embed_subtitles(&current_path, &tracks, &with_subs)
                .await
                .map_err(media_err)?;
            current_path = with_subs;
        }

        if req.embed_thumbnail {
            if let Some(thumb) = &primary_outcome.thumbnail_path {
                let with_thumb = work_dir.join(format!("with_thumb.{extension}"));
                ffmpeg
                    .embed_thumbnail(&current_path, thumb, &with_thumb)
                    .await
                    .map_err(media_err)?;
                current_path = with_thumb;
            }
        }

        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(EngineError::from)?;
            }
        }
        tokio::fs::rename(&current_path, &destination)
            .await
            .map_err(EngineError::from)?;
        let _ = tokio::fs::remove_dir_all(&work_dir).await;

        let final_size = tokio::fs::metadata(&destination)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        sdm_storage::update_job_downloaded_bytes(self.pool, &job_id, final_size as i64)
            .await
            .map_err(EngineError::from)?;
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Completed)
            .await
            .map_err(EngineError::from)?;
        let _ = progress.send(ProgressEvent::Completed {
            job_id: job_id.clone(),
            destination: destination.to_string_lossy().to_string(),
            total_bytes: final_size,
        });

        let record = sdm_storage::get_job(self.pool, &job_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        Ok(record.into())
    }
}

/// Owned copy of the parts of [`MediaDownloadRequest`] that vary per
/// playlist child (only the URL does — quality/subtitle/thumbnail
/// preferences apply uniformly across a whole playlist download).
fn req_for_child(req: &MediaDownloadRequest, child_url: &str) -> MediaDownloadRequest {
    MediaDownloadRequest {
        url: child_url.to_string(),
        destination_dir: req.destination_dir.clone(),
        quality: req.quality.clone(),
        subtitle_langs: req.subtitle_langs.clone(),
        embed_thumbnail: req.embed_thumbnail,
        duplicate_policy: req.duplicate_policy,
        ytdlp: req.ytdlp.clone(),
        ffmpeg: req.ffmpeg.clone(),
    }
}

async fn run_fetch(
    ytdlp: &YtDlpClient,
    req: &YtDlpFetchRequest,
    job_id: &str,
    progress: &ProgressSender,
) -> Result<sdm_media::YtDlpFetchOutcome, EngineError> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<YtDlpEvent>();
    let job_id_owned = job_id.to_string();
    let progress_clone = progress.clone();
    let reporter = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let YtDlpEvent::Downloading {
                downloaded_bytes: Some(downloaded),
                total_bytes,
                ..
            } = event
            {
                let _ = progress_clone.send(ProgressEvent::Progress {
                    job_id: job_id_owned.clone(),
                    downloaded_bytes: downloaded,
                    total_bytes,
                });
            }
        }
    });
    let outcome = ytdlp.fetch(req, tx).await.map_err(media_err);
    let _ = reporter.await;
    outcome
}

fn choose_format<'m>(
    metadata: &'m VideoMetadata,
    quality: &QualitySelector,
) -> Result<&'m FormatInfo, EngineError> {
    match quality {
        QualitySelector::FormatId(id) => metadata
            .formats
            .iter()
            .find(|f| &f.format_id == id)
            .ok_or_else(|| {
                EngineError::Other(format!(
                    "requested format_id {id:?} is not among the formats this source offers"
                ))
            }),
        QualitySelector::Best => metadata
            .video_formats()
            .into_iter()
            .max_by(|a, b| {
                let key = |f: &&FormatInfo| (f.height.unwrap_or(0), f.tbr.unwrap_or(0.0));
                key(a).0.cmp(&key(b).0).then(
                    key(a)
                        .1
                        .partial_cmp(&key(b).1)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
            })
            .or_else(|| metadata.formats.first())
            .ok_or_else(|| {
                EngineError::Other("source reported no downloadable formats at all".to_string())
            }),
    }
}

async fn persist_media_meta(
    pool: &SqlitePool,
    job_id: &str,
    metadata: &VideoMetadata,
    selected_format_id: &str,
    req: &MediaDownloadRequest,
) -> Result<(), EngineError> {
    let chapters_json = if metadata.chapters.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&metadata.chapters)
                .map_err(|e| EngineError::Other(e.to_string()))?,
        )
    };
    let formats_json = if metadata.formats.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&metadata.formats)
                .map_err(|e| EngineError::Other(e.to_string()))?,
        )
    };
    let subtitle_langs_json = if req.subtitle_langs.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&req.subtitle_langs)
                .map_err(|e| EngineError::Other(e.to_string()))?,
        )
    };

    sdm_storage::insert_media_meta(
        pool,
        job_id,
        metadata.title.as_deref(),
        metadata.thumbnail.as_deref(),
        metadata.duration,
        chapters_json.as_deref(),
        formats_json.as_deref(),
        metadata.is_livestream(),
        Some(selected_format_id),
        subtitle_langs_json.as_deref(),
        req.embed_thumbnail,
    )
    .await
    .map_err(EngineError::from)
}

fn work_dir_for(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".to_string());
    name.push_str(".sdm-media-tmp");
    destination
        .parent()
        .map(|p| p.join(&name))
        .unwrap_or_else(|| PathBuf::from(&name))
}

/// Strip characters that are awkward/invalid in filenames on common
/// filesystems, collapsing to `_`. Not exhaustive Unicode normalization —
/// just enough that a yt-dlp video title (which can contain `/`, `:`,
/// emoji, etc.) becomes a safe filename stem.
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "video".to_string()
    } else {
        trimmed.chars().take(150).collect()
    }
}

fn media_err(e: sdm_media::MediaError) -> EngineError {
    EngineError::Other(e.to_string())
}

#[cfg(test)]
mod detect_tests {
    use super::*;

    #[test]
    fn recognizes_known_hosts_case_and_scheme_insensitively() {
        assert!(looks_like_media_url(
            "https://youtu.be/6hkG4-LgwvI?si=1N4LecanvIcoj-BX"
        ));
        assert!(looks_like_media_url(
            "HTTPS://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ"
        ));
        assert!(looks_like_media_url("https://vimeo.com/76979871"));
        assert!(looks_like_media_url(
            "https://www.tiktok.com/@user/video/123"
        ));
        assert!(!looks_like_media_url("https://example.com/report.pdf"));
    }

    #[test]
    fn direct_file_extensions_are_recognized() {
        assert!(has_direct_file_extension("https://example.com/archive.zip"));
        assert!(has_direct_file_extension(
            "https://example.com/installer.EXE?token=abc"
        ));
        assert!(!has_direct_file_extension(
            "https://example.com/watch?v=abc"
        ));
    }

    #[test]
    fn any_plausible_extension_counts_as_a_direct_file_not_just_the_enumerated_list() {
        // The whole point of generalizing past a fixed list: an
        // extension this crate has never heard of should still skip the
        // probe, since a probe is only worth its multi-second subprocess
        // cost for genuinely extensionless (page-like) URLs.
        assert!(has_direct_file_extension(
            "https://cdn.example.com/firmware.bin"
        ));
        assert!(has_direct_file_extension(
            "https://cdn.example.com/update.abc123"
        ));
        assert!(!has_direct_file_extension(
            "https://example.com/reel/somevideo"
        ));
        assert!(!has_direct_file_extension("https://example.com/video/123"));
    }

    #[tokio::test]
    async fn detect_media_source_short_circuits_on_known_host_without_probing() {
        // A bogus binary path would make any real probe attempt fail
        // immediately; a known host must return `true` without ever
        // reaching that probe, proving the fast path is taken.
        let bogus_ytdlp = YtDlpBinary::new("/nonexistent/yt-dlp-binary");
        assert!(detect_media_source("https://youtu.be/dQw4w9WgXcQ", &bogus_ytdlp).await);
    }

    #[tokio::test]
    async fn detect_media_source_short_circuits_on_direct_file_extension() {
        let bogus_ytdlp = YtDlpBinary::new("/nonexistent/yt-dlp-binary");
        assert!(!detect_media_source("https://example.com/dataset.zip", &bogus_ytdlp).await);
    }

    #[tokio::test]
    async fn detect_media_source_never_blocks_on_a_failing_probe() {
        // Not a known host and no direct-file extension, so this falls
        // through to `probe_is_media`, which must fail gracefully (not
        // panic, not hang) against a binary that doesn't exist and
        // report "not media" rather than propagating an error.
        let bogus_ytdlp = YtDlpBinary::new("/nonexistent/yt-dlp-binary");
        assert!(!detect_media_source("https://example.com/some/page", &bogus_ytdlp).await);
    }
}
