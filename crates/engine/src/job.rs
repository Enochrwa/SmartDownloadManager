use serde::{Deserialize, Serialize};

/// A single user-requested download (one URL, magnet, or playlist entry).
///
/// This is the in-process/API-facing view; `crates/storage::JobRecord` is
/// the persisted view. `From` impls below keep them in sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub status: JobStatus,
    pub job_kind: JobKind,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub connections: u32,
    pub checksum_algorithm: Option<String>,
    pub checksum_expected: Option<String>,
    pub checksum_actual: Option<String>,
    pub checksum_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Probing,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
}

/// Which engine drives this job. HTTP keeps using the Sprint 1-6
/// segmented/single-stream path; FTP and Torrent are Sprint 7 additions;
/// SFTP/SCP/WebDAV are Sprint 8; HLS/DASH are Sprint 9 — see
/// `crates/engine::ftp`, `crates/engine::torrent`, `crates/engine::ssh`,
/// `crates/engine::webdav`, `crates/engine::hls`, `crates/engine::dash`.
/// (Metalink has no variant of its own — see `crates/engine::metalink`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Http,
    Ftp,
    Torrent,
    Sftp,
    Scp,
    WebDav,
    Hls,
    Dash,
}

impl From<sdm_storage::JobKind> for JobKind {
    fn from(k: sdm_storage::JobKind) -> Self {
        match k {
            sdm_storage::JobKind::Http => JobKind::Http,
            sdm_storage::JobKind::Ftp => JobKind::Ftp,
            sdm_storage::JobKind::Torrent => JobKind::Torrent,
            sdm_storage::JobKind::Sftp => JobKind::Sftp,
            sdm_storage::JobKind::Scp => JobKind::Scp,
            sdm_storage::JobKind::WebDav => JobKind::WebDav,
            sdm_storage::JobKind::Hls => JobKind::Hls,
            sdm_storage::JobKind::Dash => JobKind::Dash,
        }
    }
}

impl From<JobKind> for sdm_storage::JobKind {
    fn from(k: JobKind) -> Self {
        match k {
            JobKind::Http => sdm_storage::JobKind::Http,
            JobKind::Ftp => sdm_storage::JobKind::Ftp,
            JobKind::Torrent => sdm_storage::JobKind::Torrent,
            JobKind::Sftp => sdm_storage::JobKind::Sftp,
            JobKind::Scp => sdm_storage::JobKind::Scp,
            JobKind::WebDav => sdm_storage::JobKind::WebDav,
            JobKind::Hls => sdm_storage::JobKind::Hls,
            JobKind::Dash => sdm_storage::JobKind::Dash,
        }
    }
}

impl From<sdm_storage::JobStatus> for JobStatus {
    fn from(s: sdm_storage::JobStatus) -> Self {
        match s {
            sdm_storage::JobStatus::Queued => JobStatus::Queued,
            sdm_storage::JobStatus::Probing => JobStatus::Probing,
            sdm_storage::JobStatus::Downloading => JobStatus::Downloading,
            sdm_storage::JobStatus::Paused => JobStatus::Paused,
            sdm_storage::JobStatus::Verifying => JobStatus::Verifying,
            sdm_storage::JobStatus::Completed => JobStatus::Completed,
            sdm_storage::JobStatus::Failed => JobStatus::Failed,
        }
    }
}

impl From<sdm_storage::JobRecord> for Job {
    fn from(r: sdm_storage::JobRecord) -> Self {
        Job {
            id: r.id,
            url: r.url,
            destination: r.destination,
            status: r.status.into(),
            job_kind: r.job_kind.into(),
            total_bytes: r.total_bytes.map(|v| v as u64),
            downloaded_bytes: r.downloaded_bytes as u64,
            connections: r.connections as u32,
            checksum_algorithm: r.checksum_algorithm,
            checksum_expected: r.checksum_expected,
            checksum_actual: r.checksum_actual,
            checksum_verified: r.checksum_verified,
        }
    }
}
