//! Progress events emitted by the engine as a job runs, so a UI (CLI bar,
//! desktop app, HTTP/WS API in later sprints) can render live progress.

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Probing {
        job_id: String,
    },
    Started {
        job_id: String,
        total_bytes: Option<u64>,
        connections: u32,
    },
    Progress {
        job_id: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Retrying {
        job_id: String,
        error_class: String,
        attempt: u32,
        delay_ms: u64,
    },
    Completed {
        job_id: String,
        destination: String,
        total_bytes: u64,
    },
    Failed {
        job_id: String,
        error_class: String,
        message: String,
    },
}

pub type ProgressSender = tokio::sync::mpsc::UnboundedSender<ProgressEvent>;
pub type ProgressReceiver = tokio::sync::mpsc::UnboundedReceiver<ProgressEvent>;

pub fn channel() -> (ProgressSender, ProgressReceiver) {
    tokio::sync::mpsc::unbounded_channel()
}
