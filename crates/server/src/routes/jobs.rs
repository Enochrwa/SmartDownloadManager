//! `GET/POST /jobs`, `GET/DELETE /jobs/:id`, `POST /jobs/:id/pause`,
//! `POST /jobs/:id/resume` — the general-purpose job CRUD surface. The
//! extension-specific "capture" flow (dedup-aware, browser-context-aware)
//! lives in `routes::capture` instead; this module is the plain create/
//! list/manage API any REST/WS client (including `sdm-cli`, eventually)
//! can use.

use std::sync::Arc;

use axum::extract::{Path, State};
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

pub async fn create_job(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<CreateJobRequest>,
) -> axum::response::Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "url must not be empty");
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
        Json(serde_json::json!({"accepted": true})),
    )
        .into_response()
}

pub async fn list_jobs(State(state): State<Arc<ServerState>>) -> axum::response::Response {
    match sdm_storage::list_jobs(&state.pool).await {
        Ok(jobs) => {
            let out: Vec<JobResponse> = jobs.into_iter().map(JobResponse::from).collect();
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
        Ok(Some(job)) => Json(JobResponse::from(job)).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "job not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn resume_job(
    State(state): State<Arc<ServerState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    if sdm_storage::get_job(&state.pool, &job_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return err(StatusCode::NOT_FOUND, "job not found");
    }
    let id_for_run = job_id.clone();
    crate::runner::spawn_job_runner(state.clone(), move |engine, tx| async move {
        engine.resume_download(&id_for_run, tx).await
    });
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

pub async fn cancel_job(
    State(state): State<Arc<ServerState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    {
        let mut running = state.running.lock().await;
        if let Some(job) = running.remove(&job_id) {
            job.abort.abort();
        }
    }
    match sdm_storage::delete_job(&state.pool, &job_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
