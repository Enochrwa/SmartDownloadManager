//! SFTP/SCP job orchestration (Sprint 8).
//!
//! Mirrors `crate::ftp`'s shape (free functions over `&SqlitePool` rather
//! than a persistent connection owned by `Engine`, since each attempt
//! opens a fresh SSH session) with one addition: SFTP gets a real
//! multi-channel *segmented* download — unlike FTP, which has no
//! standard concurrent-range primitive at all, SFTP's `SSH_FXP_OPEN` can
//! be issued on any number of channels within one authenticated session,
//! so `docs/SPRINT_PLAN_PHASE2.md`'s "segmented SFTP over N channels"
//! requirement gets the same segment-planning helpers HTTP uses
//! (`crate::segment::{choose_connection_count, plan_segments}`), just
//! without HTTP's dynamic segment-stealing — a straggler channel here
//! just finishes on its own rather than getting its tail end stolen by
//! an idle sibling, which keeps this module a fraction of the size of
//! `crate::download`'s segment allocator for a protocol pair
//! (SFTP/SCP) that's a much smaller slice of expected traffic than HTTP.
//! SCP, wired through the same session, stays single-stream and
//! non-resumable (see `sdm_protocols::scp` module docs for why).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sdm_protocols::sftp;
use sdm_protocols::ssh::{HostKeyPolicy, SshAuth, SshSession, SshUrl};
use sdm_storage::{JobKind, JobStatus, SqlitePool};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::EngineError;
use crate::job::Job;
use crate::naming::unique_destination;
use crate::progress::{ProgressEvent, ProgressSender};
use crate::retry;
use crate::segment::{choose_connection_count, plan_segments, ConnectionsOption};

/// How to authenticate and verify the host for an SFTP/SCP job — plumbed
/// straight from the CLI's `--ssh-key`/`--ssh-password`/
/// `--accept-new-hostkey` flags.
#[derive(Clone)]
pub struct SshConnectionOptions {
    pub auth: SshAuth,
    pub known_hosts_path: PathBuf,
    pub host_key_policy: HostKeyPolicy,
}

impl SshConnectionOptions {
    /// Build from a URL's own `user:pass@` credentials with the default
    /// `known_hosts` path and strict host-key policy — used when the CLI
    /// gets neither `--ssh-key` nor `--ssh-password` and the URL itself
    /// carries a password.
    pub fn from_url_password(url: &SshUrl) -> Option<Self> {
        url.password.as_ref().map(|p| SshConnectionOptions {
            auth: SshAuth::Password(p.clone()),
            known_hosts_path: sdm_protocols::ssh::default_known_hosts_path(),
            host_key_policy: HostKeyPolicy::Strict,
        })
    }
}

pub struct SftpDownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    pub connections: ConnectionsOption,
    pub connection: SshConnectionOptions,
}

pub struct ScpDownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    pub connection: SshConnectionOptions,
}

pub struct SshEngine<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SshEngine<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    // ---------------------------------------------------------------
    // SFTP
    // ---------------------------------------------------------------

    pub async fn start_sftp_download(
        &self,
        req: SftpDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let destination = unique_destination(&req.destination);
        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = destination.to_string_lossy().to_string();
        sdm_storage::insert_job_with_kind(self.pool, &job_id, &req.url, &dest_str, JobKind::Sftp)
            .await?;

        self.run_sftp(
            job_id,
            req.url,
            destination,
            req.connections,
            req.connection,
            progress,
        )
        .await
    }

    /// Resume: SFTP's `SSH_FXP_OPEN` takes an explicit offset, so —
    /// unlike SCP — this genuinely continues from `resume_from` rather
    /// than restarting, the same recovery model Sprint 3 established for
    /// HTTP. Connection count is fixed to whatever the original job used
    /// (mirroring `Engine::resume_download`'s treatment of `connections`).
    pub async fn resume_sftp_download(
        &self,
        job_id: String,
        connection: SshConnectionOptions,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let destination = PathBuf::from(&record.destination);
        let connections = ConnectionsOption::Fixed(record.connections.max(1) as u32);
        self.run_sftp(
            job_id,
            record.url,
            destination,
            connections,
            connection,
            progress,
        )
        .await
    }

    async fn run_sftp(
        &self,
        job_id: String,
        url: String,
        destination: PathBuf,
        connections_opt: ConnectionsOption,
        connection: SshConnectionOptions,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });

        let ssh_url = SshUrl::parse(&url, "sftp")?;
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self
                .try_sftp_once(
                    &job_id,
                    &ssh_url,
                    &destination,
                    connections_opt,
                    &connection,
                    &progress,
                )
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
                    if !e.is_retryable() || attempt >= retry::max_attempts(&class) {
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

    async fn try_sftp_once(
        &self,
        job_id: &str,
        ssh_url: &SshUrl,
        destination: &std::path::Path,
        connections_opt: ConnectionsOption,
        connection: &SshConnectionOptions,
        progress: &ProgressSender,
    ) -> Result<u64, sdm_protocols::SshProtoError> {
        let session = SshSession::connect(
            ssh_url,
            &connection.auth,
            connection.known_hosts_path.clone(),
            connection.host_key_policy,
        )
        .await?;
        let session = Arc::new(session);

        let remote_path = ssh_url.path.clone();
        let total_bytes = sftp::stat_size(&session, &remote_path).await?;
        let resume_from = tokio::fs::metadata(destination)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
            .min(total_bytes);

        let connections = choose_connection_count(Some(total_bytes), true, connections_opt);
        sdm_storage::update_job_probe(
            self.pool,
            job_id,
            Some(total_bytes as i64),
            true,
            None,
            None,
            connections as i64,
        )
        .await
        .map_err(|e| sdm_protocols::SshProtoError::Io(std::io::Error::other(e.to_string())))?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.to_string(),
            total_bytes: Some(total_bytes),
            connections,
        });

        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let job_id_owned = job_id.to_string();
        let progress_clone = progress.clone();
        let downloaded_so_far = Arc::new(AtomicU64::new(resume_from));
        let downloaded_for_task = downloaded_so_far.clone();
        let reporter = tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                let total = downloaded_for_task.fetch_add(n, Ordering::Relaxed) + n;
                let _ = progress_clone.send(ProgressEvent::Progress {
                    job_id: job_id_owned.clone(),
                    downloaded_bytes: total,
                    total_bytes: Some(total_bytes),
                });
            }
        });

        let result = if connections <= 1 || total_bytes == 0 {
            sftp::download_single(&session, &remote_path, destination, resume_from, Some(tx))
                .await
                .map(|written| resume_from + written)
        } else {
            self.download_segmented(
                &session,
                &remote_path,
                destination,
                total_bytes,
                resume_from,
                connections,
                tx,
            )
            .await
        };

        drop(reporter);
        result
    }

    /// Fixed-partition multi-channel download: split
    /// `[resume_from, total_bytes)` into `connections` byte ranges (same
    /// planner HTTP uses) and open one SFTP channel per range on the
    /// shared `session`, all writing into the same preallocated file at
    /// their own offsets. No segment stealing (see module docs) — this
    /// is deliberately the simpler static-partition half of what
    /// `crate::segment` offers HTTP.
    #[allow(clippy::too_many_arguments)]
    async fn download_segmented(
        &self,
        session: &Arc<SshSession>,
        remote_path: &str,
        destination: &std::path::Path,
        total_bytes: u64,
        resume_from: u64,
        connections: u32,
        progress_tx: tokio::sync::mpsc::UnboundedSender<u64>,
    ) -> Result<u64, sdm_protocols::SshProtoError> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(destination)
            .await?;
        file.set_len(total_bytes).await?;
        let file = Arc::new(AsyncMutex::new(file));

        let remaining = total_bytes - resume_from;
        let segments = plan_segments(remaining, connections)
            .into_iter()
            .map(|(s, e)| (s + resume_from, e + resume_from));

        let mut tasks = Vec::new();
        for (start, end) in segments {
            let session_ref = session.clone();
            let remote_path = remote_path.to_string();
            let file = file.clone();
            let tx = progress_tx.clone();
            let end_atomic = Arc::new(AtomicU64::new(end));
            let position = Arc::new(AtomicU64::new(start));
            // Each task opens its own SFTP subsystem channel (inside
            // `sftp::download_range`) on the same `Arc`-shared,
            // authenticated SSH session/TCP connection — that's the
            // multi-channel-over-one-session model this module's docs
            // describe. Spawned as a real task (not just an async block)
            // so segments genuinely run concurrently rather than
            // round-robining on one task's poll.
            let task = tokio::spawn(async move {
                sftp::download_range(
                    &session_ref,
                    &remote_path,
                    start,
                    end_atomic,
                    file,
                    position,
                    Some(tx),
                )
                .await
            });
            tasks.push(task);
        }

        let mut total_written = resume_from;
        for task in tasks {
            let written = task.await.map_err(|e| {
                sdm_protocols::SshProtoError::Io(std::io::Error::other(format!(
                    "segment task panicked: {e}"
                )))
            })??;
            total_written += written;
        }
        Ok(total_written)
    }

    // ---------------------------------------------------------------
    // SCP
    // ---------------------------------------------------------------

    pub async fn start_scp_download(
        &self,
        req: ScpDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let destination = unique_destination(&req.destination);
        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = destination.to_string_lossy().to_string();
        sdm_storage::insert_job_with_kind(self.pool, &job_id, &req.url, &dest_str, JobKind::Scp)
            .await?;

        self.run_scp(job_id, req.url, destination, req.connection, progress)
            .await
    }

    /// SCP has no resume primitive (see `sdm_protocols::scp` module
    /// docs) — "resuming" an SCP job means re-running the whole transfer
    /// from byte 0, which is still useful after e.g. a killed process,
    /// just not bandwidth-efficient the way SFTP/HTTP resume is.
    pub async fn resume_scp_download(
        &self,
        job_id: String,
        connection: SshConnectionOptions,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let record = sdm_storage::get_job(self.pool, &job_id)
            .await?
            .ok_or_else(|| EngineError::JobNotFound(job_id.clone()))?;
        let destination = PathBuf::from(&record.destination);
        self.run_scp(job_id, record.url, destination, connection, progress)
            .await
    }

    async fn run_scp(
        &self,
        job_id: String,
        url: String,
        destination: PathBuf,
        connection: SshConnectionOptions,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let _ = progress.send(ProgressEvent::Probing {
            job_id: job_id.clone(),
        });
        let ssh_url = SshUrl::parse(&url, "scp")?;
        sdm_storage::set_job_status(self.pool, &job_id, JobStatus::Downloading).await?;
        let _ = progress.send(ProgressEvent::Started {
            job_id: job_id.clone(),
            total_bytes: None,
            connections: 1,
        });

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self
                .try_scp_once(&job_id, &ssh_url, &destination, &connection, &progress)
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
                    if !e.is_retryable() || attempt >= retry::max_attempts(&class) {
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

    async fn try_scp_once(
        &self,
        job_id: &str,
        ssh_url: &SshUrl,
        destination: &std::path::Path,
        connection: &SshConnectionOptions,
        progress: &ProgressSender,
    ) -> Result<u64, sdm_protocols::SshProtoError> {
        let session = SshSession::connect(
            ssh_url,
            &connection.auth,
            connection.known_hosts_path.clone(),
            connection.host_key_policy,
        )
        .await?;
        let remote_path = ssh_url.path.clone();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let job_id_owned = job_id.to_string();
        let progress_clone = progress.clone();
        let downloaded = Arc::new(AtomicU64::new(0));
        let downloaded_for_task = downloaded.clone();
        let reporter = tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                let total = downloaded_for_task.fetch_add(n, Ordering::Relaxed) + n;
                let _ = progress_clone.send(ProgressEvent::Progress {
                    job_id: job_id_owned.clone(),
                    downloaded_bytes: total,
                    total_bytes: None,
                });
            }
        });

        let result =
            sdm_protocols::scp::download(&session, &remote_path, destination, Some(tx)).await;
        drop(reporter);
        result
    }
}

async fn fetch_job(pool: &SqlitePool, job_id: &str) -> anyhow::Result<Job> {
    let record = sdm_storage::get_job(pool, job_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("job {job_id} vanished after completing"))?;
    Ok(record.into())
}
