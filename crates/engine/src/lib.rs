//! sdm-engine: the download orchestrator.
//!
//! - Sprint 1: single-stream downloads, job persistence.
//! - Sprint 2: segmented multi-connection downloads with segment stealing.
//! - Sprint 3: intelligent per-error-class retry, resume validation,
//!   segment-state journaling, automatic file renaming on conflict.

pub mod download;
pub mod error;
pub mod job;
pub mod naming;
pub mod progress;
pub mod resume;
pub mod retry;
pub mod segment;

pub use download::{DownloadRequest, Engine};
pub use error::EngineError;
pub use job::{Job, JobStatus};
pub use progress::{channel, ProgressEvent, ProgressReceiver, ProgressSender};
pub use segment::ConnectionsOption;
