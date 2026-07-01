#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("storage error: {0}")]
    Storage(#[source] anyhow::Error),
    #[error(transparent)]
    Protocol(#[from] sdm_protocols::ProtoError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("job not found: {0}")]
    JobNotFound(String),
}

impl From<anyhow::Error> for EngineError {
    fn from(e: anyhow::Error) -> Self {
        EngineError::Storage(e)
    }
}
