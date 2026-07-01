//! The download orchestrator: ties probing, segment planning, the
//! segment-stealing allocator, the retry FSM, and SQLite journaling
//! together into one `Engine::download` call.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sdm_protocols::ErrorClass;
use sdm_storage::SqlitePool;
use tokio::fs::File;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::EngineError;
use crate::job::Job;
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};
use crate::resume::{decide_resume, ResumeDecision};
use crate::retry;
use crate::segment::{
    choose_connection_count, plan_segments, ConnectionsOption, SegmentAllocator, SegmentRuntime,
};

pub struct DownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    pub connections: ConnectionsOption,
}

pub struct Engine {
    pool: SqlitePool,
    client: reqwest::Client,
}

impl Engine {
    pub fn new(pool: SqlitePool) -> Self {
        Engine {
            pool,
            client: sdm_protocols::build_client(),
        }
    }

    /// Start a brand-new download.
    pub async fn start_download(
        &self,
        req: DownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let job_id = uuid::Uuid::new_v4().to_string();
        let destination = unique_destination(&req.destination);
        let dest_str = destination.to_string_lossy().to_string();
        sdm_storage::insert_job(&self.pool, &job_id, &req.url, &dest_str).await?;
        self.run(
            job_id,
            req.url,
            destination,
            req.connections,
            false,
            progress,
        )
        .await
    }

    /// Resume a previously created job (Sprint 3). Re-probes the URL and
    /// either continues from the persisted segment state or, if the
    /// server-side resource changed, restarts from scratch.
    pub async fn resume_download(
        &self,
        job_id: &str,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(&self.pool, job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.to_string()))?;
        let url = record.url.clone();
        let destination = PathBuf::from(&record.destination);
        let connections = ConnectionsOption::Fixed(record.connections.max(1) as u32);
        self.run(
            job_id.to_string(),
            url,
            destination,
            connections,
            true,
            progress,
        )
        .await
    }

    async fn run(
        &self,
        job_id: String,
        url: String,
        destination: PathBuf,
        connections_opt: ConnectionsOption,
        is_resume: bool,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let probe_result = sdm_protocols::probe(&self.client, &url).await;
        if let Err(e) = &probe_result {
            let _ = progress.send(ProgressEvent::Failed {
                job_id: job_id.clone(),
                error_class: e.class().as_str().to_string(),
                message: e.to_string(),
            });
        }
        let probe_info = probe_result?;

        let stored = if is_resume {
            sdm_storage::get_job(&self.pool, &job_id).await?
        } else {
            None
        };

        let resume_decision = match &stored {
            Some(record) => decide_resume(record, &probe_info),
            None => ResumeDecision::Restart, // fresh job: nothing to resume
        };

        let connections = choose_connection_count(
            probe_info.total_bytes,
            probe_info.supports_range,
            connections_opt,
        );

        sdm_storage::update_job_probe(
            &self.pool,
            &job_id,
            probe_info.total_bytes.map(|v| v as i64),
            probe_info.supports_range,
            probe_info.etag.as_deref(),
            probe_info.last_modified.as_deref(),
            connections as i64,
        )
        .await?;

        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: probe_info.total_bytes,
            connections,
        });

        let result = if let (true, Some(total_bytes)) =
            (probe_info.supports_range, probe_info.total_bytes)
        {
            self.run_segmented(
                &job_id,
                &url,
                &destination,
                total_bytes,
                connections,
                resume_decision,
                &progress,
            )
            .await
        } else {
            self.run_single_stream(&job_id, &url, &destination, &progress)
                .await
        };

        match result {
            Ok(total) => {
                sdm_storage::set_job_status(&self.pool, &job_id, sdm_storage::JobStatus::Completed)
                    .await?;
                let _ = progress.send(ProgressEvent::Completed {
                    job_id: job_id.clone(),
                    destination: destination.to_string_lossy().to_string(),
                    total_bytes: total,
                });
                let record = sdm_storage::get_job(&self.pool, &job_id)
                    .await?
                    .expect("job exists");
                Ok(record.into())
            }
            Err(e) => {
                let class = match &e {
                    EngineError::Protocol(p) => p.class(),
                    _ => ErrorClass::Other,
                };
                sdm_storage::set_job_error(&self.pool, &job_id, class.as_str(), &e.to_string())
                    .await?;
                let _ = progress.send(ProgressEvent::Failed {
                    job_id: job_id.clone(),
                    error_class: class.as_str().to_string(),
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }

    /// Sprint 1 fallback path: the server doesn't support byte ranges (or
    /// we don't know the size), so we stream the whole thing sequentially.
    /// Not resumable mid-stream; a retry restarts from byte 0.
    async fn run_single_stream(
        &self,
        job_id: &str,
        url: &str,
        destination: &std::path::Path,
        progress: &ProgressSender,
    ) -> Result<u64, EngineError> {
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let total_downloaded = Arc::new(AtomicU64::new(0));

        let aggregator = {
            let pool = self.pool.clone();
            let job_id = job_id.to_string();
            let progress = progress.clone();
            let total_downloaded = total_downloaded.clone();
            tokio::spawn(async move {
                while let Some(delta) = delta_rx.recv().await {
                    let total = total_downloaded.fetch_add(delta, Ordering::SeqCst) + delta;
                    let _ = sdm_storage::update_job_downloaded_bytes(&pool, &job_id, total as i64)
                        .await;
                    let _ = progress.send(ProgressEvent::Progress {
                        job_id: job_id.clone(),
                        downloaded_bytes: total,
                        total_bytes: None,
                    });
                }
            })
        };

        let mut attempt = 0u32;
        let result = loop {
            attempt += 1;
            match sdm_protocols::download_single(
                &self.client,
                url,
                destination,
                Some(delta_tx.clone()),
            )
            .await
            {
                Ok(total) => break Ok(total),
                Err(e) => {
                    let class = e.class();
                    if !class.is_retryable() || attempt >= retry::max_attempts(&class) {
                        break Err(EngineError::Protocol(e));
                    }
                    let delay = retry::backoff_delay(&class, attempt);
                    let _ = progress.send(ProgressEvent::Retrying {
                        job_id: job_id.to_string(),
                        error_class: class.as_str().to_string(),
                        attempt,
                        delay_ms: delay.as_millis() as u64,
                    });
                    total_downloaded.store(0, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                }
            }
        };

        drop(delta_tx);
        let _ = aggregator.await;
        result
    }

    /// Sprint 2+3 path: byte-range segmented download with segment
    /// stealing and per-segment retry/resume.
    async fn run_segmented(
        &self,
        job_id: &str,
        url: &str,
        destination: &std::path::Path,
        total_bytes: u64,
        connections: u32,
        resume_decision: ResumeDecision,
        progress: &ProgressSender,
    ) -> Result<u64, EngineError> {
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let file = if resume_decision == ResumeDecision::Resume
            && tokio::fs::metadata(destination).await.is_ok()
        {
            // Genuine resume: open without truncating so already-downloaded
            // bytes survive. set_len is a no-op if the file is already the
            // right size, and extends it (with zero-fill) if it's short.
            tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(destination)
                .await?
        } else {
            tokio::fs::File::create(destination).await?
        };
        file.set_len(total_bytes).await?;
        let file = Arc::new(AsyncMutex::new(file));

        let existing_segments = if resume_decision == ResumeDecision::Resume {
            sdm_storage::get_segments(&self.pool, job_id).await?
        } else {
            vec![]
        };

        let initial_runtimes: Vec<SegmentRuntime> = if !existing_segments.is_empty() {
            existing_segments
                .iter()
                .map(|s| {
                    let position = match s.status {
                        sdm_storage::SegmentStatus::Completed => s.end_byte as u64 + 1,
                        _ => s.start_byte as u64, // restart in-progress/failed segments from their own start
                    };
                    let rt = SegmentRuntime::new(
                        s.id.clone(),
                        s.seq,
                        s.start_byte as u64,
                        s.end_byte as u64,
                    );
                    rt.position.store(position, Ordering::SeqCst);
                    rt.done.store(
                        s.status == sdm_storage::SegmentStatus::Completed,
                        Ordering::SeqCst,
                    );
                    rt
                })
                .collect()
        } else {
            let plan = plan_segments(total_bytes, connections);
            let plan_i64: Vec<(i64, i64, i64)> = plan
                .iter()
                .enumerate()
                .map(|(i, (s, e))| (i as i64, *s as i64, *e as i64))
                .collect();
            let records = sdm_storage::replace_segments(&self.pool, job_id, &plan_i64).await?;
            records
                .iter()
                .map(|r| {
                    SegmentRuntime::new(r.id.clone(), r.seq, r.start_byte as u64, r.end_byte as u64)
                })
                .collect()
        };

        let already_downloaded: u64 = initial_runtimes
            .iter()
            .filter(|s| s.done.load(Ordering::SeqCst))
            .map(|s| s.end.load(Ordering::SeqCst) - s.start + 1)
            .sum();

        let work_queue: Arc<AsyncMutex<VecDeque<SegmentRuntime>>> = Arc::new(AsyncMutex::new(
            initial_runtimes
                .iter()
                .filter(|s| !s.done.load(Ordering::SeqCst))
                .cloned()
                .collect(),
        ));
        let allocator = Arc::new(SegmentAllocator::new(initial_runtimes.clone()));

        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let total_downloaded = Arc::new(AtomicU64::new(already_downloaded));

        let aggregator = {
            let pool = self.pool.clone();
            let job_id = job_id.to_string();
            let progress = progress.clone();
            let total_downloaded = total_downloaded.clone();
            tokio::spawn(async move {
                while let Some(delta) = delta_rx.recv().await {
                    let total = total_downloaded.fetch_add(delta, Ordering::SeqCst) + delta;
                    let _ = sdm_storage::update_job_downloaded_bytes(&pool, &job_id, total as i64)
                        .await;
                    let _ = progress.send(ProgressEvent::Progress {
                        job_id: job_id.clone(),
                        downloaded_bytes: total,
                        total_bytes: Some(total_bytes),
                    });
                }
            })
        };

        let worker_count = connections.max(1);
        let mut handles = Vec::with_capacity(worker_count as usize);
        for _ in 0..worker_count {
            let client = self.client.clone();
            let url = url.to_string();
            let file = file.clone();
            let pool = self.pool.clone();
            let work_queue = work_queue.clone();
            let allocator = allocator.clone();
            let delta_tx = delta_tx.clone();
            let progress = progress.clone();
            let job_id = job_id.to_string();

            handles.push(tokio::spawn(async move {
                loop {
                    let seg = {
                        let mut q = work_queue.lock().await;
                        q.pop_front()
                    };
                    let seg = match seg {
                        Some(s) => s,
                        None => match allocator.try_steal().await {
                            Some(s) => {
                                let _ = sdm_storage::insert_segment(
                                    &pool,
                                    &sdm_storage::SegmentRecord {
                                        id: s.record_id.clone(),
                                        job_id: job_id.clone(),
                                        seq: s.seq,
                                        start_byte: s.start as i64,
                                        end_byte: s.end.load(Ordering::SeqCst) as i64,
                                        downloaded_bytes: 0,
                                        status: sdm_storage::SegmentStatus::Pending,
                                        retry_count: 0,
                                        last_error_class: None,
                                    },
                                )
                                .await;
                                s
                            }
                            None => break,
                        },
                    };

                    run_segment_with_retry(
                        &client, &url, &seg, &file, &pool, &delta_tx, &progress, &job_id,
                    )
                    .await?;
                }
                Ok(())
            }));
        }

        let mut first_error = None;
        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) if first_error.is_none() => first_error = Some(e),
                Err(join_err) if first_error.is_none() => {
                    first_error = Some(EngineError::Io(std::io::Error::other(join_err.to_string())))
                }
                _ => {}
            }
        }

        drop(delta_tx);
        let _ = aggregator.await;

        if let Some(e) = first_error {
            return Err(e);
        }

        Ok(total_bytes)
    }
}

/// Download one segment with retry: on a retryable failure, resume from
/// `seg.position` (not `seg.start`) on the next attempt, and journal every
/// state transition to SQLite.
#[allow(clippy::too_many_arguments)]
async fn run_segment_with_retry(
    client: &reqwest::Client,
    url: &str,
    seg: &SegmentRuntime,
    file: &Arc<AsyncMutex<File>>,
    pool: &SqlitePool,
    delta_tx: &tokio::sync::mpsc::UnboundedSender<u64>,
    progress: &ProgressSender,
    job_id: &str,
) -> Result<(), EngineError> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let resume_start = seg.position.load(Ordering::SeqCst);
        let _ = sdm_storage::update_segment(
            pool,
            &seg.record_id,
            (resume_start - seg.start) as i64,
            sdm_storage::SegmentStatus::Downloading,
            (attempt - 1) as i64,
            None,
        )
        .await;

        match sdm_protocols::download_range(
            client,
            url,
            resume_start,
            seg.end.clone(),
            file.clone(),
            seg.position.clone(),
            Some(delta_tx.clone()),
        )
        .await
        {
            Ok(_) => {
                seg.done.store(true, Ordering::SeqCst);
                let written = seg
                    .position
                    .load(Ordering::SeqCst)
                    .saturating_sub(seg.start);
                let _ = sdm_storage::update_segment(
                    pool,
                    &seg.record_id,
                    written as i64,
                    sdm_storage::SegmentStatus::Completed,
                    (attempt - 1) as i64,
                    None,
                )
                .await;
                return Ok(());
            }
            Err(e) => {
                let class = e.class();
                let written = seg
                    .position
                    .load(Ordering::SeqCst)
                    .saturating_sub(seg.start);
                let _ = sdm_storage::update_segment(
                    pool,
                    &seg.record_id,
                    written as i64,
                    sdm_storage::SegmentStatus::Failed,
                    attempt as i64,
                    Some(class.as_str()),
                )
                .await;

                if !class.is_retryable() || attempt >= retry::max_attempts(&class) {
                    return Err(EngineError::Protocol(e));
                }

                let delay = retry::backoff_delay(&class, attempt);
                let _ = progress.send(ProgressEvent::Retrying {
                    job_id: job_id.to_string(),
                    error_class: class.as_str().to_string(),
                    attempt,
                    delay_ms: delay.as_millis() as u64,
                });
                tokio::time::sleep(delay).await;
            }
        }
    }
}
