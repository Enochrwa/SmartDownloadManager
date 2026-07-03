//! sdm-engine: the download orchestrator.
//!
//! - Sprint 1: single-stream downloads, job persistence.
//! - Sprint 2: segmented multi-connection downloads with segment stealing.
//! - Sprint 3: intelligent per-error-class retry, resume validation,
//!   segment-state journaling, automatic file renaming on conflict.
//! - Sprint 4: checksum verification, per-chunk corruption detection +
//!   targeted repair, mirror support, duplicate detection.

pub mod chunking;
pub mod download;
pub mod duplicate;
pub mod error;
pub mod job;
pub mod mirrors;
pub mod naming;
pub mod progress;
pub mod resume;
pub mod retry;
pub mod segment;
pub mod verify;

pub use chunking::{find_corrupt_chunks, repair_chunk, CorruptChunk};
pub use download::{DownloadRequest, Engine};
pub use duplicate::{DuplicateMatch, DuplicatePolicy, DuplicateReason};
pub use error::EngineError;
pub use job::{Job, JobStatus};
pub use mirrors::MirrorSet;
pub use progress::{channel, ProgressEvent, ProgressReceiver, ProgressSender};
pub use segment::ConnectionsOption;
pub use verify::{verify_file, ChecksumAlgorithm, ExpectedChecksum};
