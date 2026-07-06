#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("storage error: {0}")]
    Storage(#[source] anyhow::Error),
    #[error(transparent)]
    Protocol(#[from] sdm_protocols::ProtoError),
    #[error(transparent)]
    Ftp(#[from] sdm_protocols::ftp::FtpProtoError),
    #[error(transparent)]
    Ssh(#[from] sdm_protocols::SshProtoError),
    #[error(transparent)]
    Torrent(#[from] sdm_torrent::TorrentError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("job not found: {0}")]
    JobNotFound(String),
    #[error("checksum mismatch: expected {expected} ({algorithm}), got {actual}")]
    ChecksumMismatch {
        algorithm: String,
        expected: String,
        actual: String,
    },
    #[error("download skipped: duplicate of existing job {existing_job_id}")]
    DuplicateSkipped { existing_job_id: String },
    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for EngineError {
    fn from(e: anyhow::Error) -> Self {
        EngineError::Storage(e)
    }
}
