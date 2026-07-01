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
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub connections: u32,
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
            total_bytes: r.total_bytes.map(|v| v as u64),
            downloaded_bytes: r.downloaded_bytes as u64,
            connections: r.connections as u32,
        }
    }
}
