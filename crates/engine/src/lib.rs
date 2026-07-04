//! sdm-engine: the download orchestrator.
//!
//! - Sprint 1: single-stream downloads, job persistence.
//! - Sprint 2: segmented multi-connection downloads with segment stealing.
//! - Sprint 3: intelligent per-error-class retry, resume validation,
//!   segment-state journaling, automatic file renaming on conflict.
//! - Sprint 4: checksum verification, per-chunk corruption detection +
//!   targeted repair, mirror support, duplicate detection.
//! - Sprint 6: recovery (corrupted-database repair, orphaned temp-file
//!   cleanup, automatic backups, session restore) for the desktop app.

pub mod chunking;
pub mod download;
pub mod duplicate;
pub mod error;
pub mod ftp;
pub mod job;
pub mod mirrors;
pub mod naming;
pub mod progress;
pub mod recovery;
pub mod resume;
pub mod retry;
pub mod segment;
pub mod torrent;
pub mod verify;

pub use chunking::{find_corrupt_chunks, repair_chunk, CorruptChunk};
pub use download::{DownloadRequest, Engine};
pub use duplicate::{DuplicateMatch, DuplicatePolicy, DuplicateReason};
pub use error::EngineError;
pub use ftp::FtpDownloadRequest;
pub use job::{Job, JobKind, JobStatus};
pub use mirrors::MirrorSet;
pub use progress::{channel, ProgressEvent, ProgressReceiver, ProgressSender};
pub use recovery::{RepairAction, RepairReport};
pub use segment::ConnectionsOption;
pub use torrent::TorrentDownloadRequest;
pub use verify::{verify_file, ChecksumAlgorithm, ExpectedChecksum};
