//! `POST /capture` and `POST /capture/batch` — the browser extension's
//! entry point into the engine (Sprint 11). Distinct from `routes::jobs`
//! because every capture path (download interception, context menu,
//! clipboard detection, drag-and-drop, batch paste) needs the same
//! "is this already in the queue/history?" dedup check
//! (docs/SPRINT_PLAN_PHASE2.md Sprint 11: "deduplicated against the
//! existing-queue/history check from Sprint 4") before starting a new
//! job, which plain `POST /jobs` deliberately doesn't impose (a CLI/API
//! caller may well want to force a second copy).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use sdm_api_types::{
    BatchCaptureRequest, BatchCaptureResponse, BatchCaptureResult, CaptureRequest, CaptureResponse,
    ErrorResponse, JobResponse,
};
use sdm_engine::{ConnectionsOption, DownloadRequest};

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

/// Shared by both `/capture` and `/capture/batch`: check for an existing
/// duplicate first; if none, start a fresh download and return a
/// best-effort in-flight `JobResponse` (id + Queued status) since the
/// full download itself continues asynchronously, streaming further
/// progress over `/ws/progress`.
async fn capture_one(state: &Arc<ServerState>, url: &str) -> anyhow::Result<(JobResponse, bool)> {
    let parsed = util::parse_create_request(state, url, None, None, None)?;

    if let Some(existing) =
        util::find_existing_duplicate(&state.pool, url, &parsed.destination, None).await?
    {
        return Ok((existing, true));
    }

    let download_req = DownloadRequest {
        url: url.to_string(),
        mirrors: Vec::new(),
        destination: parsed.destination,
        connections: ConnectionsOption::Auto,
        expected_checksum: parsed.expected_checksum,
        duplicate_policy: parsed.duplicate_policy,
    };

    // `start_download` mints the job id itself and only inserts the row
    // once it does — `spawn_job_runner_awaiting_id` hands that id back as
    // soon as the first progress event (which can only fire after the
    // insert) arrives, so by the time we read it back below the row is
    // guaranteed to exist.
    let id_rx =
        crate::runner::spawn_job_runner_awaiting_id(state.clone(), move |engine, tx| async move {
            engine.start_download(download_req, tx).await
        });

    let job_id = id_rx
        .await
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("download failed to start"))?;

    let job = sdm_storage::get_job(&state.pool, &job_id)
        .await?
        .map(JobResponse::from)
        .ok_or_else(|| anyhow::anyhow!("job vanished immediately after insert"))?;
    Ok((job, false))
}

pub async fn capture(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<CaptureRequest>,
) -> axum::response::Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "url must not be empty");
    }
    tracing::info!(url = %req.url, source = %req.source, "extension capture");
    match capture_one(&state, &req.url).await {
        Ok((job, deduplicated)) => (
            StatusCode::ACCEPTED,
            Json(CaptureResponse { job, deduplicated }),
        )
            .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

pub async fn capture_batch(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<BatchCaptureRequest>,
) -> axum::response::Response {
    // Dedup the input list itself first (a pasted block of text often
    // repeats the same URL — e.g. an <a> tag and its visible href) before
    // touching storage at all, per Sprint 11's "extracts and queues all
    // of them, deduplicated" scope note.
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::with_capacity(req.urls.len());

    for url in req.urls {
        let url = url.trim().to_string();
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        match capture_one(&state, &url).await {
            Ok((job, deduplicated)) => results.push(BatchCaptureResult {
                url,
                job: Some(job),
                deduplicated,
                error: None,
            }),
            Err(e) => results.push(BatchCaptureResult {
                url,
                job: None,
                deduplicated: false,
                error: Some(e.to_string()),
            }),
        }
    }

    Json(BatchCaptureResponse { results }).into_response()
}
