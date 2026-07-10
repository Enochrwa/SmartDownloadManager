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
//! - Sprint 7: FTP/FTPS (Phase 1 carryover) and BitTorrent/magnet.
//! - Sprint 8: SFTP (multi-channel segmented), SCP, and WebDAV.
//! - Sprint 9: Metalink (mirrors + hash, reuses Sprint 4 machinery as-is),
//!   HLS, and MPEG-DASH.
//! - Sprint 10: yt-dlp + FFmpeg-backed extraction from yt-dlp's
//!   thousands of supported sites (`crates/engine::media`).

pub mod chunking;
pub mod dash;
pub mod download;
pub mod duplicate;
pub mod error;
pub mod ftp;
pub mod hls;
pub mod job;
pub mod media;
pub mod metalink;
pub mod mirrors;
pub mod naming;
pub mod progress;
pub mod recovery;
pub mod resume;
pub mod retry;
pub mod segment;
pub mod ssh;
pub mod torrent;
pub mod verify;
pub mod vpn;
pub mod webdav;

pub use chunking::{find_corrupt_chunks, repair_chunk, CorruptChunk};
pub use dash::{DashDownloadRequest, DashEngine};
pub use download::{DownloadRequest, Engine};
pub use duplicate::{DuplicateMatch, DuplicatePolicy, DuplicateReason};
pub use error::EngineError;
pub use ftp::FtpDownloadRequest;
pub use hls::{HlsDownloadRequest, HlsEngine};
pub use job::{Job, JobKind, JobStatus};
pub use media::{MediaDownloadRequest, MediaEngine, QualitySelector};
pub use metalink::MetalinkSource;
pub use mirrors::MirrorSet;
pub use progress::{channel, ProgressEvent, ProgressReceiver, ProgressSender};
pub use recovery::{RepairAction, RepairReport};
pub use segment::ConnectionsOption;
pub use ssh::{ScpDownloadRequest, SftpDownloadRequest, SshConnectionOptions, SshEngine};
pub use torrent::TorrentDownloadRequest;
pub use verify::{verify_file, ChecksumAlgorithm, ExpectedChecksum};
pub use vpn::{VpnEvent, VpnMonitor};
pub use webdav::{WebDavDownloadRequest, WebDavEngine};
