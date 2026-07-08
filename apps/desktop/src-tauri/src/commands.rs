//! Tauri commands: the React UI's entire surface onto `sdm-engine`. No
//! separate daemon process — the desktop app drives the engine in-process,
//! same pattern as `crates/cli`, but streaming progress over Tauri events
//! instead of a terminal progress bar.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use sdm_engine::{
    ConnectionsOption, DownloadRequest, DuplicatePolicy, Engine, ExpectedChecksum, ProgressEvent,
};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_notification::NotificationExt;

use crate::dto::{JobDto, JobEventDto, RepairReportDto};
use crate::state::{AppState, RunningJob};

const JOB_EVENT: &str = "job-event";

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

fn default_destination_name(url: &str) -> String {
    let name = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let name = name.split(['?', '#']).next().unwrap_or(name);
    if name.is_empty() {
        "download".to_string()
    } else {
        name.to_string()
    }
}

/// Turn one `sdm_engine::ProgressEvent` into the frontend-facing DTO,
/// computing an instantaneous transfer speed for `Progress` events from
/// the last sample recorded for this job.
async fn to_dto(state: &AppState, event: &ProgressEvent) -> JobEventDto {
    match event {
        ProgressEvent::Probing { job_id } => JobEventDto::Probing {
            job_id: job_id.clone(),
        },
        ProgressEvent::Started {
            job_id,
            total_bytes,
            connections,
        } => JobEventDto::Started {
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
            // Only overwrite the sample once enough time has passed, so
            // speed doesn't get diluted by back-to-back events a few
            // milliseconds apart.
            let should_update = tracker
                .get(job_id)
                .map(|(last_instant, _)| now.duration_since(*last_instant).as_secs_f64() > 0.15)
                .unwrap_or(true);
            if should_update {
                tracker.insert(job_id.clone(), (now, *downloaded_bytes));
            }
            JobEventDto::Progress {
                job_id: job_id.clone(),
                downloaded_bytes: *downloaded_bytes,
                total_bytes: *total_bytes,
                speed_bps,
            }
        }
        ProgressEvent::Verifying { job_id } => JobEventDto::Verifying {
            job_id: job_id.clone(),
        },
        ProgressEvent::Retrying {
            job_id,
            error_class,
            attempt,
            delay_ms,
        } => JobEventDto::Retrying {
            job_id: job_id.clone(),
            error_class: error_class.clone(),
            attempt: *attempt,
            delay_ms: *delay_ms,
        },
        ProgressEvent::Completed {
            job_id,
            destination,
            total_bytes,
        } => JobEventDto::Completed {
            job_id: job_id.clone(),
            destination: destination.clone(),
            total_bytes: *total_bytes,
        },
        ProgressEvent::Failed {
            job_id,
            error_class,
            message,
        } => JobEventDto::Failed {
            job_id: job_id.clone(),
            error_class: error_class.clone(),
            message: message.clone(),
        },
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Drive one engine call (`start_download` for a brand-new job, or
/// `resume_download` for an existing one) to completion in the
/// background, streaming every progress event to the frontend as a
/// `job-event` Tauri event and firing a system notification on
/// completion/failure. Registers the task's abort handle in
/// `state.running` so `pause_job`/`cancel_job` can stop it, keyed by the
/// job ID — which, for brand-new downloads, isn't known until the first
/// event arrives (the engine mints it internally), so registration
/// happens on receipt of that first event rather than up front.
fn spawn_job_runner<F, Fut>(app: AppHandle, state: Arc<AppState>, run: F)
where
    F: FnOnce(Arc<Engine>, sdm_engine::ProgressSender) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<sdm_engine::job::Job, sdm_engine::error::EngineError>>
        + Send
        + 'static,
{
    tokio::spawn(async move {
        let (tx, mut rx) = sdm_engine::channel();
        let engine = state.engine.clone();
        let inner = tokio::spawn(async move { run(engine, tx).await });
        let abort_handle = inner.abort_handle();

        let mut registered_id: Option<String> = None;
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
            }

            if let ProgressEvent::Completed { destination, .. } = &event {
                notify(
                    &app,
                    "Download complete",
                    &format!("Saved to {destination}"),
                );
            }
            if let ProgressEvent::Failed { message, .. } = &event {
                notify(&app, "Download failed", message);
            }

            let dto = to_dto(&state, &event).await;
            let _ = app.emit(JOB_EVENT, dto);
        }

        if let Some(id) = &registered_id {
            state.running.lock().await.remove(id);
            state.speed_tracker.lock().await.remove(id);
        }
        // Drain the inner task; a `JoinError` here means it panicked or was
        // aborted (pause/cancel) — both already reflected in job status via
        // the events above (or, for abort, via the explicit status update
        // the pause/cancel commands make themselves), so there's nothing
        // further to do with the result.
        let _ = inner.await;
    });
}

#[tauri::command]
pub async fn add_download(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    url: String,
    destination: Option<String>,
    connections: Option<String>,
    mirrors: Option<Vec<String>>,
    checksum: Option<String>,
    on_duplicate: Option<String>,
) -> Result<(), String> {
    let state = state.inner().clone();

    let destination = match destination {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d),
        _ => state
            .paths
            .default_download_dir
            .join(default_destination_name(&url)),
    };
    let connections = ConnectionsOption::parse(connections.as_deref().unwrap_or("auto"))
        .map_err(|e| e.to_string())?;
    let expected_checksum = checksum
        .as_deref()
        .map(ExpectedChecksum::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    let duplicate_policy = DuplicatePolicy::parse(on_duplicate.as_deref().unwrap_or("rename"))
        .map_err(|e| e.to_string())?;

    let req = DownloadRequest {
        url,
        mirrors: mirrors.unwrap_or_default(),
        destination,
        connections,
        expected_checksum,
        duplicate_policy,
    };

    spawn_job_runner(app, state, move |engine, tx| async move {
        engine.start_download(req, tx).await
    });
    Ok(())
}

#[tauri::command]
pub async fn resume_job(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    job_id: String,
) -> Result<(), String> {
    let state = state.inner().clone();
    let id_for_run = job_id.clone();
    spawn_job_runner(app, state, move |engine, tx| async move {
        engine.resume_download(&id_for_run, tx).await
    });
    Ok(())
}

/// Pause a running job: stop its in-flight task and mark it `Paused` in
/// storage. Segment progress already journaled to SQLite (Sprint 3) is
/// what makes `resume_job` pick back up correctly — this command doesn't
/// need to (and doesn't) touch segment rows itself.
///
/// Note: aborting the outer task stops new reads/writes from being
/// scheduled, but any segment worker mid-`await` on a network read may
/// briefly keep running until its current chunk lands, since those are
/// separate tokio tasks the engine spawns internally. That's harmless
/// (the extra bytes are still journaled correctly) but means "paused"
/// isn't always instantaneous to the millisecond.
#[tauri::command]
pub async fn pause_job(state: State<'_, Arc<AppState>>, job_id: String) -> Result<(), String> {
    let mut running = state.running.lock().await;
    if let Some(job) = running.remove(&job_id) {
        job.abort.abort();
    }
    drop(running);
    sdm_storage::set_job_status(&state.pool, &job_id, sdm_storage::JobStatus::Paused)
        .await
        .map_err(|e| e.to_string())
}

/// Cancel a job outright: stop its task (if running) and mark it `Failed`
/// with a "cancelled by user" message. The row is kept (consistent with
/// how any other failure is handled) — use `remove_job` to clear it from
/// the queue view entirely.
#[tauri::command]
pub async fn cancel_job(state: State<'_, Arc<AppState>>, job_id: String) -> Result<(), String> {
    let mut running = state.running.lock().await;
    if let Some(job) = running.remove(&job_id) {
        job.abort.abort();
    }
    drop(running);
    sdm_storage::set_job_error(&state.pool, &job_id, "cancelled", "Cancelled by user")
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_job(state: State<'_, Arc<AppState>>, job_id: String) -> Result<(), String> {
    {
        let mut running = state.running.lock().await;
        if let Some(job) = running.remove(&job_id) {
            job.abort.abort();
        }
    }
    sdm_storage::delete_job(&state.pool, &job_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_jobs(state: State<'_, Arc<AppState>>) -> Result<Vec<JobDto>, String> {
    let jobs = sdm_storage::list_jobs(&state.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(jobs.into_iter().map(JobDto::from).collect())
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, Arc<AppState>>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let settings = sdm_storage::list_settings(&state.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(settings.into_iter().collect())
}

#[tauri::command]
pub async fn set_setting(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: String,
) -> Result<(), String> {
    sdm_storage::set_setting(&state.pool, &key, &value)
        .await
        .map_err(|e| e.to_string())
}

/// Default download directory the frontend can offer as a placeholder in
/// the add-download dialog before any explicit setting is chosen.
#[tauri::command]
pub fn default_download_dir(state: State<'_, Arc<AppState>>) -> String {
    state
        .paths
        .default_download_dir
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
pub async fn repair_database(state: State<'_, Arc<AppState>>) -> Result<RepairReportDto, String> {
    sdm_engine::recovery::repair_database_if_corrupt(&state.paths.db_path, &state.paths.backup_dir)
        .await
        .map(RepairReportDto::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn backup_now(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    sdm_engine::recovery::backup_database(
        &state.pool,
        &state.paths.backup_dir,
        sdm_engine::recovery::DEFAULT_BACKUP_RETENTION,
    )
    .await
    .map(|p| p.to_string_lossy().to_string())
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cleanup_orphans(
    state: State<'_, Arc<AppState>>,
    delete: bool,
) -> Result<Vec<String>, String> {
    let dirs = vec![state.paths.default_download_dir.clone()];
    sdm_engine::recovery::find_orphaned_files(&state.pool, &dirs, delete)
        .await
        .map(|paths| {
            paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// Called once from the Tauri `setup` hook (see `lib.rs`): finds jobs left
/// mid-flight by an unclean previous shutdown and resumes each one
/// automatically, streaming their progress the same as any other job.
pub fn auto_resume_interrupted_jobs(app: AppHandle, state: Arc<AppState>) {
    tokio::spawn(async move {
        let interrupted = match sdm_engine::recovery::restore_session(&state.pool).await {
            Ok(jobs) => jobs,
            Err(e) => {
                tracing::warn!("session restore query failed: {e}");
                return;
            }
        };
        for job in interrupted {
            tracing::info!(job_id = %job.id, "auto-resuming job interrupted by last shutdown");
            let id = job.id.clone();
            spawn_job_runner(app.clone(), state.clone(), move |engine, tx| async move {
                engine.resume_download(&id, tx).await
            });
        }
    });
}

/// Sprint 11: the settings panel's "Extension connected" indicator polls
/// this to learn whether any paired browser extension has been seen
/// recently, and to list every paired extension for the user to review
/// or revoke.
#[tauri::command]
pub async fn pairing_status(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::dto::PairingStatusDto, String> {
    // A token counts as "connected" if seen within the last two minutes —
    // generous enough that a popup that's merely idle (not polling every
    // second) still shows as connected, but tight enough to reflect an
    // uninstalled/quit browser fairly promptly.
    const CONNECTED_WINDOW_SECS: i64 = 120;
    let connected = sdm_storage::has_recent_pairing_activity(&state.pool, CONNECTED_WINDOW_SECS)
        .await
        .map_err(|e| e.to_string())?;
    let tokens = sdm_storage::list_pairing_tokens(&state.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(crate::dto::PairingStatusDto {
        connected,
        paired_extensions: tokens
            .into_iter()
            .map(|t| crate::dto::PairedExtensionDto {
                label: t.label,
                created_at: t.created_at,
                last_seen_at: t.last_seen_at,
            })
            .collect(),
        api_port: state.extension_api_port,
    })
}

/// Mint a brand-new pairing token for the first-run pairing flow: the UI
/// displays this token (and the `api_port` from [`pairing_status`]) for
/// the user to enter into the extension's options page.
#[tauri::command]
pub async fn pairing_issue_token(
    state: State<'_, Arc<AppState>>,
    label: Option<String>,
) -> Result<crate::dto::PairingTokenDto, String> {
    let token = {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    };
    let label = label
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| "Browser extension".to_string());
    sdm_storage::insert_pairing_token(&state.pool, &token, &label)
        .await
        .map_err(|e| e.to_string())?;
    Ok(crate::dto::PairingTokenDto {
        token,
        label,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Revoke a paired extension (e.g. the user uninstalled it, or wants to
/// re-pair from scratch).
#[tauri::command]
pub async fn pairing_revoke_token(
    state: State<'_, Arc<AppState>>,
    token: String,
) -> Result<(), String> {
    sdm_storage::revoke_pairing_token(&state.pool, &token)
        .await
        .map_err(|e| e.to_string())
}
