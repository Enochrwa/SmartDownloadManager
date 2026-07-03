//! The download orchestrator: ties probing, segment planning, the
//! segment-stealing allocator, the retry FSM, mirror rotation, and SQLite
//! journaling together into one `Engine::download` call. Sprint 4 adds
//! post-download checksum verification, per-chunk corruption detection,
//! and duplicate-detection policy handling.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sdm_protocols::ErrorClass;
use sdm_storage::SqlitePool;
use tokio::fs::File;
use tokio::sync::Mutex as AsyncMutex;

use crate::chunking::{self, DEFAULT_CHUNK_SIZE};
use crate::duplicate::{self, DuplicatePolicy};
use crate::error::EngineError;
use crate::job::Job;
use crate::mirrors::{self, MirrorSet};
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};
use crate::resume::{decide_resume, ResumeDecision};
use crate::retry;
use crate::segment::{
    choose_connection_count, plan_segments, ConnectionsOption, SegmentAllocator, SegmentRuntime,
};
use crate::verify::{self, ChecksumAlgorithm, ExpectedChecksum};

pub struct DownloadRequest {
    pub url: String,
    /// Additional mirror URLs serving the same content (Sprint 4). The
    /// primary `url` is always tried first at probe time; these widen the
    /// pool of servers segments can fail over to.
    pub mirrors: Vec<String>,
    pub destination: PathBuf,
    pub connections: ConnectionsOption,
    /// Optional pre-supplied checksum to verify the finished download
    /// against (Sprint 4). Always computed and stored even if `None`.
    pub expected_checksum: Option<ExpectedChecksum>,
    /// What to do if this looks like a duplicate of an existing job
    /// (Sprint 4). Defaults to `Rename`, matching the Sprint 3 behavior.
    pub duplicate_policy: DuplicatePolicy,
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

    /// Check whether a prospective download looks like a duplicate of an
    /// existing job (same URL, same destination filename, or same
    /// checksum). Exposed so callers (e.g. the CLI) can prompt the user
    /// interactively before deciding on a [`DuplicatePolicy`].
    pub async fn check_duplicates(
        &self,
        url: &str,
        destination: &std::path::Path,
        expected_checksum: Option<&str>,
    ) -> Result<Vec<duplicate::DuplicateMatch>, EngineError> {
        duplicate::find_duplicates(&self.pool, url, destination, expected_checksum).await
    }

    /// Start a brand-new download.
    pub async fn start_download(
        &self,
        req: DownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let duplicates = duplicate::find_duplicates(
            &self.pool,
            &req.url,
            &req.destination,
            req.expected_checksum.as_ref().map(|c| c.hex.as_str()),
        )
        .await?;

        if req.duplicate_policy == DuplicatePolicy::Skip {
            if let Some(existing) = duplicates.first() {
                return Err(EngineError::DuplicateSkipped {
                    existing_job_id: existing.job.id.clone(),
                });
            }
        }

        let destination = match req.duplicate_policy {
            DuplicatePolicy::Overwrite => req.destination.clone(),
            DuplicatePolicy::Skip | DuplicatePolicy::Rename => unique_destination(&req.destination),
        };

        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = destination.to_string_lossy().to_string();
        sdm_storage::insert_job(&self.pool, &job_id, &req.url, &dest_str).await?;

        if let Some(expected) = &req.expected_checksum {
            sdm_storage::set_job_expected_checksum(
                &self.pool,
                &job_id,
                expected.algorithm.as_str(),
                &expected.hex,
            )
            .await?;
        }

        let mirror_set = self
            .probe_and_persist_mirrors(&job_id, &req.url, &req.mirrors)
            .await?;

        self.run(
            job_id,
            mirror_set,
            destination,
            req.connections,
            false,
            progress,
            req.expected_checksum,
        )
        .await
    }

    /// Probe every candidate mirror's latency, rank fastest-first, and
    /// persist the ranking so a later resume can reuse it without
    /// re-probing.
    async fn probe_and_persist_mirrors(
        &self,
        job_id: &str,
        primary_url: &str,
        extra_mirrors: &[String],
    ) -> Result<MirrorSet, EngineError> {
        let mut all_urls = vec![primary_url.to_string()];
        for m in extra_mirrors {
            if !all_urls.contains(m) {
                all_urls.push(m.clone());
            }
        }

        let ranked = if all_urls.len() > 1 {
            let probes = mirrors::probe_mirrors(&self.client, &all_urls).await;
            mirrors::rank_by_latency(probes)
        } else {
            vec![mirrors::MirrorProbe {
                url: all_urls[0].clone(),
                latency_ms: None,
            }]
        };

        let ranked_urls: Vec<String> = ranked.iter().map(|p| p.url.clone()).collect();
        let latencies: Vec<Option<i64>> = ranked
            .iter()
            .map(|p| p.latency_ms.map(|v| v as i64))
            .collect();
        sdm_storage::replace_mirrors(&self.pool, job_id, &ranked_urls, &latencies).await?;

        Ok(MirrorSet::new(ranked_urls))
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
        let destination = PathBuf::from(&record.destination);
        let connections = ConnectionsOption::Fixed(record.connections.max(1) as u32);

        let persisted_mirrors = sdm_storage::get_mirrors(&self.pool, job_id).await?;
        let mirror_set = if persisted_mirrors.is_empty() {
            MirrorSet::new(vec![record.url.clone()])
        } else {
            MirrorSet::new(persisted_mirrors.into_iter().map(|m| m.url).collect())
        };

        let expected_checksum = match (&record.checksum_algorithm, &record.checksum_expected) {
            (Some(algo), Some(hex)) => Some(ExpectedChecksum {
                algorithm: ChecksumAlgorithm::parse(algo)?,
                hex: hex.clone(),
            }),
            _ => None,
        };

        self.run(
            job_id.to_string(),
            mirror_set,
            destination,
            connections,
            true,
            progress,
            expected_checksum,
        )
        .await
    }

    async fn run(
        &self,
        job_id: String,
        mirror_set: MirrorSet,
        destination: PathBuf,
        connections_opt: ConnectionsOption,
        is_resume: bool,
        progress: ProgressSender,
        expected_checksum: Option<ExpectedChecksum>,
    ) -> Result<Job, EngineError> {
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let primary_url = mirror_set.primary().to_string();
        let probe_result = sdm_protocols::probe(&self.client, &primary_url).await;
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

        let segmented = matches!(
            (probe_info.supports_range, probe_info.total_bytes),
            (true, Some(_))
        );

        let result = if let (true, Some(total_bytes)) =
            (probe_info.supports_range, probe_info.total_bytes)
        {
            self.run_segmented(
                &job_id,
                &mirror_set,
                &destination,
                total_bytes,
                connections,
                resume_decision,
                &progress,
            )
            .await
        } else {
            self.run_single_stream(&job_id, &mirror_set, &destination, &progress)
                .await
        };

        // Post-download verification (Sprint 4): compute (and optionally
        // compare) a whole-file checksum, and — for segmented downloads,
        // where a specific corrupted byte range can actually be
        // identified — record per-chunk hashes for later corruption
        // repair. A checksum mismatch turns a successful transfer into a
        // failed job, same as any other terminal error.
        let result = match result {
            Ok(total) => {
                let _ = progress.send(ProgressEvent::Verifying {
                    job_id: job_id.clone(),
                });
                match self
                    .verify_after_download(
                        &job_id,
                        &destination,
                        total,
                        segmented,
                        &expected_checksum,
                    )
                    .await
                {
                    Ok(()) => Ok(total),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
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
                    EngineError::ChecksumMismatch { .. } => ErrorClass::Other,
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

    /// Compute (and, if the caller supplied one, compare against) the
    /// whole-file checksum, and record per-chunk hashes for segmented
    /// downloads so a future `verify_and_repair` can localize corruption.
    async fn verify_after_download(
        &self,
        job_id: &str,
        destination: &std::path::Path,
        total_bytes: u64,
        segmented: bool,
        expected_checksum: &Option<ExpectedChecksum>,
    ) -> Result<(), EngineError> {
        if segmented {
            chunking::hash_and_record_file(
                &self.pool,
                job_id,
                destination,
                total_bytes,
                DEFAULT_CHUNK_SIZE,
            )
            .await?;
        }

        let algorithm = expected_checksum
            .as_ref()
            .map(|c| c.algorithm)
            .unwrap_or(ChecksumAlgorithm::Sha256);
        let actual = verify::compute_file_checksum(destination, algorithm).await?;

        if let Some(expected) = expected_checksum {
            let matches = actual.eq_ignore_ascii_case(&expected.hex);
            sdm_storage::set_job_checksum_result(
                &self.pool,
                job_id,
                algorithm.as_str(),
                &actual,
                matches,
            )
            .await?;
            if !matches {
                return Err(EngineError::ChecksumMismatch {
                    algorithm: algorithm.as_str().to_string(),
                    expected: expected.hex.clone(),
                    actual,
                });
            }
        } else {
            sdm_storage::set_job_checksum_result(
                &self.pool,
                job_id,
                algorithm.as_str(),
                &actual,
                false,
            )
            .await?;
        }

        Ok(())
    }

    /// Re-verify a completed job's chunk hashes and re-fetch only the
    /// chunks that no longer match (Sprint 4 targeted corruption repair).
    /// Returns the number of chunks that were found corrupt and repaired.
    pub async fn verify_and_repair(&self, job_id: &str) -> Result<usize, EngineError> {
        let record = sdm_storage::get_job(&self.pool, job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.to_string()))?;
        let destination = PathBuf::from(&record.destination);

        let corrupt = chunking::find_corrupt_chunks(&self.pool, job_id, &destination).await?;
        if corrupt.is_empty() {
            return Ok(0);
        }

        let persisted_mirrors = sdm_storage::get_mirrors(&self.pool, job_id).await?;
        let mirror_set = if persisted_mirrors.is_empty() {
            MirrorSet::new(vec![record.url.clone()])
        } else {
            MirrorSet::new(persisted_mirrors.into_iter().map(|m| m.url).collect())
        };

        let file = Arc::new(AsyncMutex::new(
            tokio::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .open(&destination)
                .await?,
        ));

        let repaired = corrupt.len();
        for chunk in &corrupt {
            chunking::repair_chunk(
                &self.client,
                mirror_set.primary(),
                file.clone(),
                &self.pool,
                chunk,
            )
            .await?;
        }
        Ok(repaired)
    }

    /// Sprint 1 fallback path: the server doesn't support byte ranges (or
    /// we don't know the size), so we stream the whole thing sequentially.
    /// Not resumable mid-stream; a retry restarts from byte 0.
    async fn run_single_stream(
        &self,
        job_id: &str,
        mirror_set: &MirrorSet,
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
            let url = mirror_set.pick_for_attempt(attempt);
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
        mirror_set: &MirrorSet,
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
            let mirror_set = mirror_set.clone();
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
                        &client,
                        &mirror_set,
                        &seg,
                        &file,
                        &pool,
                        &delta_tx,
                        &progress,
                        &job_id,
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
    mirror_set: &MirrorSet,
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
        let url = mirror_set.pick_for_attempt(attempt);
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
                // If there's more than one mirror, the delay above is
                // skipped in favor of an immediate switch to the next
                // mirror — no point sleeping when a different server might
                // just work.
                if mirror_set.len() <= 1 {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}
