//! FTP/FTPS job orchestration (Sprint 7, "Phase 1 carryover").
//!
//! FTP has no standard mechanism for concurrent byte-range segments across
//! independent connections the way HTTP Range requests do, so an FTP job
//! is modeled as a single-stream download with `REST`-based resume — the
//! FTP analogue of Sprint 1's HTTP path, before Sprint 2 added segmented
//! downloads. `sdm_protocols::ftp` owns the wire protocol; this module's
//! job is purely to plug it into the same `Job`/SQLite/`ProgressEvent`
//! contract every other job kind already uses, so the CLI (and later the
//! desktop app) doesn't need protocol-specific UI code.

use std::path::PathBuf;

use sdm_protocols::ftp::{FtpSession, FtpUrl};
use sdm_storage::{JobKind, JobStatus, SqlitePool};

use crate::error::EngineError;
use crate::job::Job;
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};
use crate::retry;

pub struct FtpDownloadRequest {
    pub url: String,
    pub destination: PathBuf,
}

/// FTP-specific half of [`crate::download::Engine`]. Kept as free functions
/// taking `&SqlitePool` rather than a second engine struct, since (unlike
/// the HTTP engine) there's no persistent per-job connection or client to
/// own between calls — each attempt opens a fresh FTP session.
pub struct FtpEngine<'a> {
    pool: &'a SqlitePool,
}

impl<'a> FtpEngine<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn start_download(
        &self,
        req: FtpDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let destination = unique_destination(&req.destination);
        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = destination.to_string_lossy().to_string();
        sdm_storage::insert_job_with_kind(self.pool, &job_id, &req.url, &dest_str, JobKind::Ftp)
            .await?;

        self.run(job_id, req.url, destination, progress).await
    }

    /// Resume a previously-started FTP job (e.g. after the process was
    /// killed mid-transfer) from however many bytes already landed on
    /// disk, the same recovery model Sprint 3/6 use for HTTP.
    pub async fn resume_download(
        &self,
        job_id: String,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let destination = PathBuf::from(&record.destination);
        self.run(job_id, record.url, destination, progress).await
    }

    async fn run(
        &self,
        job_id: String,
        url: String,
        destination: PathBuf,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let ftp_url = FtpUrl::parse(&url)?;
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: None,
            connections: 1,
        });

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let resume_from = tokio::fs::metadata(&destination)
                .await
                .map(|m| m.len())
                .unwrap_or(0);

            match self
                .try_download_once(&job_id, &ftp_url, &destination, resume_from, &progress)
                .await
            {
                Ok(total_bytes) => {
                    sdm_storage::update_job_downloaded_bytes(
                        self.pool,
                        &job_id,
                        total_bytes as i64,
                    )
                    .await?;
                    sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Completed).await?;
                    let _ = progress.send(ProgressEvent::Completed {
                        job_id: job_id.clone(),
                        destination: destination.to_string_lossy().to_string(),
                        total_bytes,
                    });
                    return Ok(fetch_job(self.pool, &job_id).await?);
                }
                Err(e) => {
                    let class = e.class();
                    if !class.is_retryable() || attempt >= retry::max_attempts(&class) {
                        sdm_storage::set_job_error(
                            self.pool,
                            &job_id,
                            class.as_str(),
                            &e.to_string(),
                        )
                        .await?;
                        let _ = progress.send(ProgressEvent::Failed {
                            job_id: job_id.clone(),
                            error_class: class.as_str().to_string(),
                            message: e.to_string(),
                        });
                        return Err(EngineError::from(e));
                    }
                    let delay = retry::backoff_delay(&class, attempt);
                    let _ = progress.send(ProgressEvent::Retrying {
                        job_id: job_id.clone(),
                        error_class: class.as_str().to_string(),
                        attempt,
                        delay_ms: delay.as_millis() as u64,
                    });
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn try_download_once(
        &self,
        job_id: &str,
        ftp_url: &FtpUrl,
        destination: &std::path::Path,
        resume_from: u64,
        progress: &ProgressSender,
    ) -> Result<u64, sdm_protocols::ftp::FtpProtoError> {
        let mut session = FtpSession::connect(ftp_url).await?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let job_id_owned = job_id.to_string();
        let progress_clone = progress.clone();
        let downloaded_so_far = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(resume_from));
        let downloaded_for_task = downloaded_so_far.clone();
        let reporter = tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                let total =
                    downloaded_for_task.fetch_add(n, std::sync::atomic::Ordering::Relaxed) + n;
                let _ = progress_clone.send(ProgressEvent::Progress {
                    job_id: job_id_owned.clone(),
                    downloaded_bytes: total,
                    total_bytes: None,
                });
            }
        });

        let result = session
            .download(&ftp_url.path, destination, resume_from, Some(tx))
            .await;
        drop(reporter); // reporter task ends on its own once the sender drops

        let _ = session.quit().await;
        let written = result?;
        Ok(resume_from + written)
    }
}

async fn fetch_job(pool: &SqlitePool, job_id: &str) -> anyhow::Result<Job> {
    let record = sdm_storage::get_job(pool, job_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("job {job_id} vanished after completing"))?;
    Ok(record.into())
}
