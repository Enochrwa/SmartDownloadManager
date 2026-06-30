use serde::{Deserialize, Serialize};

/// A single user-requested download (one URL, magnet, or playlist entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub status: JobStatus,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
}
