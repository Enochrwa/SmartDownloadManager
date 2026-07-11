//! `GET/POST /jobs`, `GET/DELETE /jobs/:id`, `POST /jobs/:id/pause`,
//! `POST /jobs/:id/resume` — the general-purpose job CRUD surface. The
//! extension-specific "capture" flow (dedup-aware, browser-context-aware)
//! lives in `routes::capture` instead; this module is the plain create/
//! list/manage API any REST/WS client (including `sdm-cli`, eventually)
//! can use.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use sdm_api_types::{CreateJobRequest, ErrorResponse, JobResponse};
use sdm_engine::DownloadRequest;

use crate::state::ServerState;
use crate::util;

fn err(status: StatusCode, message: impl Into<String>) -> axum::response::Response {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

/// Kick off a "capture any link" media (yt-dlp) download in the
/// background, mirroring the shape of `util::start_download` but for
/// `Engine::start_media_download` instead of the plain HTTP/protocol
/// path. Destination is always a directory here (yt-dlp/FFmpeg decide
/// the final filename from the site's own title metadata), unlike
/// `CreateJobRequest::destination`'s file-path convention for ordinary
/// downloads.
async fn start_media_job(
    state: &Arc<ServerState>,
    url: String,
    destination_dir: PathBuf,
    options: Option<sdm_api_types::MediaJobOptions>,
) {
    let (quality, subtitle_langs, embed_thumbnail) = match options {
        Some(o) => (
            o.quality
                .filter(|q| !q.eq_ignore_ascii_case("best"))
                .map(sdm_engine::QualitySelector::FormatId)
                .unwrap_or(sdm_engine::QualitySelector::Best),
            o.subtitle_langs,
            o.embed_thumbnail,
        ),
        None => (sdm_engine::QualitySelector::Best, Vec::new(), true),
    };

    let req = sdm_engine::MediaDownloadRequest {
        url,
        destination_dir,
        quality,
        subtitle_langs,
        embed_thumbnail,
        duplicate_policy: sdm_engine::DuplicatePolicy::Rename,
        ytdlp: sdm_media::YtDlpBinary::default(),
        ffmpeg: sdm_media::FfmpegBinary::default(),
    };
    let state = state.clone();
    crate::runner::spawn_job_runner(state, move |engine, tx| async move {
        engine.start_media_download(req, tx).await
    });
}

pub async fn create_job(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<CreateJobRequest>,
) -> axum::response::Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "url must not be empty");
    }

    // "Capture any link": an explicit `media.force` wins; otherwise a
    // known-host check plus a live yt-dlp probe fallback decides whether
    // this URL is a video/audio page (thousands of yt-dlp-supported
    // sites) rather than a direct file. See
    // `sdm_engine::detect_media_source` for the actual detection logic —
    // shared with the CLI and desktop app so all three agree.
    let force_media = req.media.as_ref().and_then(|m| m.force);
    let is_media = match force_media {
        Some(explicit) => explicit,
        None => {
            sdm_engine::detect_media_source(&req.url, &sdm_media::YtDlpBinary::default()).await
        }
    };

    if is_media {
        let destination_dir = match &req.destination {
            Some(d) if !d.trim().is_empty() => PathBuf::from(d),
            _ => state.default_download_dir.clone(),
        };
        start_media_job(&state, req.url.clone(), destination_dir, req.media).await;
        return (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"accepted": true, "media": true})),
        )
            .into_response();
    }

    let parsed = match util::parse_create_request(
        &state,
        &req.url,
        req.destination.as_deref(),
        req.checksum.as_deref(),
        req.on_duplicate.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let connections = match req.connections {
        Some(n) => sdm_engine::ConnectionsOption::Fixed(n as u32),
        None => sdm_engine::ConnectionsOption::Auto,
    };

    let download_req = DownloadRequest {
        url: req.url.clone(),
        mirrors: req.mirrors.clone(),
        destination: parsed.destination,
        connections,
        expected_checksum: parsed.expected_checksum,
        duplicate_policy: parsed.duplicate_policy,
    };

    if let Err(e) = util::start_download(&state, download_req).await {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"accepted": true, "media": false})),
    )
        .into_response()
}

/// Fill in `media_title`/`media_thumbnail` for media-kind jobs from the
/// `media_meta` side table — see `JobResponse`'s doc comment for why the
/// base `From<JobRecord>` conversion can't do this itself.
async fn enrich_media_fields(state: &ServerState, mut job: JobResponse) -> JobResponse {
    if job.job_kind == "media" {
        if let Ok(Some(meta)) = sdm_storage::get_media_meta(&state.pool, &job.id).await {
            job.media_title = meta.title;
            job.media_thumbnail = meta.thumbnail_url;
        }
    }
    job
}

pub async fn list_jobs(State(state): State<Arc<ServerState>>) -> axum::response::Response {
    match sdm_storage::list_jobs(&state.pool).await {
        Ok(jobs) => {
            let mut out = Vec::with_capacity(jobs.len());
            for job in jobs {
                out.push(enrich_media_fields(&state, JobResponse::from(job)).await);
            }
            Json(out).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn get_job(
    State(state): State<Arc<ServerState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    match sdm_storage::get_job(&state.pool, &job_id).await {
        Ok(Some(job)) => {
            let job = enrich_media_fields(&state, JobResponse::from(job)).await;
            Json(job).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "job not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn resume_job(
    State(state): State<Arc<ServerState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    let record = match sdm_storage::get_job(&state.pool, &job_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return err(StatusCode::NOT_FOUND, "job not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let id_for_run = job_id.clone();
    if record.job_kind == sdm_storage::JobKind::Media {
        crate::runner::spawn_job_runner(state.clone(), move |engine, tx| async move {
            engine.resume_media_download(id_for_run, tx).await
        });
    } else {
        crate::runner::spawn_job_runner(state.clone(), move |engine, tx| async move {
            engine.resume_download(&id_for_run, tx).await
        });
    }
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"accepted": true})),
    )
        .into_response()
}

pub async fn pause_job(
    State(state): State<Arc<ServerState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    {
        let mut running = state.running.lock().await;
        if let Some(job) = running.remove(&job_id) {
            job.abort.abort();
        }
    }
    match sdm_storage::set_job_status(&state.pool, &job_id, sdm_storage::JobStatus::Paused).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(serde::Deserialize)]
pub struct DeleteJobQuery {
    /// `?delete_file=true` also removes the downloaded file(s) from disk
    /// (see `sdm_storage::delete_job_with_file`), not just the queue/
    /// history row. Defaults to `false` — matching the previous
    /// row-only-delete behavior — so existing callers (extension,
    /// `sdm-cli`) that don't pass this keep working exactly as before.
    #[serde(default)]
    delete_file: bool,
}

pub async fn cancel_job(
    State(state): State<Arc<ServerState>>,
    Path(job_id): Path<String>,
    Query(query): Query<DeleteJobQuery>,
) -> axum::response::Response {
    {
        let mut running = state.running.lock().await;
        if let Some(job) = running.remove(&job_id) {
            job.abort.abort();
        }
    }
    match sdm_storage::delete_job_with_file(&state.pool, &job_id, query.delete_file).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
