//! Drives one engine call (`start_download` for a brand-new job,
//! `resume_download` for an existing one) to completion in the
//! background, converting each `sdm_engine::ProgressEvent` into the
//! wire-stable `sdm_api_types::JobEvent` and publishing it on
//! `ServerState::progress_tx` for every `/ws/progress` subscriber.
//! Deliberately mirrors `apps/desktop/src-tauri/src/commands.rs`'s
//! `spawn_job_runner` — same job-lifecycle shape, different transport
//! (broadcast channel + WebSocket instead of Tauri events).

use std::sync::Arc;
use std::time::Instant;

use sdm_api_types::JobEvent;
use sdm_engine::{Engine, ProgressEvent};

use crate::state::{RunningJob, ServerState};

fn job_id_of(event: &ProgressEvent) -> &str {
    match event {
        ProgressEvent::Probing { job_id }
        | ProgressEvent::Started { job_id, .. }
        | ProgressEvent::Progress { job_id, .. }
        | ProgressEvent::Verifying { job_id }
        | ProgressEvent::Retrying { job_id, .. }
        | ProgressEvent::Completed { job_id, .. }
        | ProgressEvent::Failed { job_id, .. } => job_id,
    }
}

async fn to_job_event(state: &ServerState, event: &ProgressEvent) -> JobEvent {
    match event {
        ProgressEvent::Probing { job_id } => JobEvent::Probing {
            job_id: job_id.clone(),
        },
        ProgressEvent::Started {
            job_id,
            total_bytes,
            connections,
        } => JobEvent::Started {
            job_id: job_id.clone(),
            total_bytes: *total_bytes,
            connections: *connections,
        },
        ProgressEvent::Progress {
            job_id,
            downloaded_bytes,
            total_bytes,
        } => {
            let now = Instant::now();
            let mut tracker = state.speed_tracker.lock().await;
            let speed_bps = match tracker.get(job_id) {
                Some((last_instant, last_bytes)) => {
                    let elapsed = now.duration_since(*last_instant).as_secs_f64();
                    if elapsed > 0.05 && *downloaded_bytes >= *last_bytes {
                        (*downloaded_bytes - *last_bytes) as f64 / elapsed
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            let should_update = tracker
                .get(job_id)
                .map(|(last_instant, _)| now.duration_since(*last_instant).as_secs_f64() > 0.15)
                .unwrap_or(true);
            if should_update {
                tracker.insert(job_id.clone(), (now, *downloaded_bytes));
            }
            JobEvent::Progress {
                job_id: job_id.clone(),
                downloaded_bytes: *downloaded_bytes,
                total_bytes: *total_bytes,
                speed_bps,
            }
        }
        ProgressEvent::Verifying { job_id } => JobEvent::Verifying {
            job_id: job_id.clone(),
        },
        ProgressEvent::Retrying {
            job_id,
            error_class,
            attempt,
            delay_ms,
        } => JobEvent::Retrying {
            job_id: job_id.clone(),
            error_class: error_class.clone(),
            attempt: *attempt,
            delay_ms: *delay_ms,
        },
        ProgressEvent::Completed {
            job_id,
            destination,
            total_bytes,
        } => JobEvent::Completed {
            job_id: job_id.clone(),
            destination: destination.clone(),
            total_bytes: Some(*total_bytes),
        },
        ProgressEvent::Failed {
            job_id,
            error_class,
            message,
        } => JobEvent::Failed {
            job_id: job_id.clone(),
            error_class: error_class.clone(),
            message: message.clone(),
        },
    }
}

/// Spawn a job runner, exactly as the desktop app does, but publish every
/// event onto the server's broadcast channel instead of a Tauri event.
pub fn spawn_job_runner<F, Fut>(state: Arc<ServerState>, run: F)
where
    F: FnOnce(Arc<Engine>, sdm_engine::ProgressSender) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<sdm_engine::job::Job, sdm_engine::error::EngineError>>
        + Send
        + 'static,
{
    drop(spawn_job_runner_awaiting_id(state, run));
}

/// Same as [`spawn_job_runner`], but also hands back a one-shot receiver
/// that resolves to the engine-minted job id as soon as the *first*
/// progress event arrives (i.e. as soon as the job row actually exists in
/// storage) — `Some(id)` normally, `None` if the engine call failed
/// before emitting anything at all (e.g. a synchronous validation error).
/// Used by `routes::capture`, which needs to hand the extension back a
/// real job id in its HTTP response rather than a bare "accepted".
pub fn spawn_job_runner_awaiting_id<F, Fut>(
    state: Arc<ServerState>,
    run: F,
) -> tokio::sync::oneshot::Receiver<Option<String>>
where
    F: FnOnce(Arc<Engine>, sdm_engine::ProgressSender) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<sdm_engine::job::Job, sdm_engine::error::EngineError>>
        + Send
        + 'static,
{
    let (id_tx, id_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (tx, mut rx) = sdm_engine::channel();
        let engine = state.engine.clone();
        let inner = tokio::spawn(async move { run(engine, tx).await });
        let abort_handle = inner.abort_handle();

        let mut registered_id: Option<String> = None;
        let mut id_tx = Some(id_tx);
        while let Some(event) = rx.recv().await {
            let job_id = job_id_of(&event).to_string();
            if registered_id.is_none() {
                state.running.lock().await.insert(
                    job_id.clone(),
                    RunningJob {
                        abort: abort_handle.clone(),
                    },
                );
                registered_id = Some(job_id.clone());
                if let Some(tx) = id_tx.take() {
                    let _ = tx.send(Some(job_id.clone()));
                }
            }

            let dto = to_job_event(&state, &event).await;
            let _ = state.progress_tx.send(dto);
        }

        // Channel closed with no event ever received: the engine call
        // failed before it could even insert the job row (e.g. a bad
        // duplicate-policy combination) — tell the waiter there's no id.
        if let Some(tx) = id_tx.take() {
            let _ = tx.send(None);
        }

        if let Some(id) = &registered_id {
            state.running.lock().await.remove(id);
            state.speed_tracker.lock().await.remove(id);
        }
        let _ = inner.await;
    });
    id_rx
}
