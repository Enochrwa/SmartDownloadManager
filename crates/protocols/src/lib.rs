//! sdm-protocols: protocol implementations.
//!
//! HTTP/HTTPS lands in Sprint 1 (single-stream) and Sprint 2 (segmented,
//! via Range requests). FTP/FTPS lands Sprint 7 (Phase 1 carryover);
//! SFTP/WebDAV/Metalink land Sprint 8. HLS (via `m3u8-rs`) and MPEG-DASH
//! (via `dash-mpd-rs`) land Phase 2, Sprint 9 — see docs/SPRINT_PLAN.md
//! and docs/TECH_DECISIONS.md §5.

pub mod error;
pub mod ftp;
pub mod http;

pub use error::ErrorClass;
pub use http::{build_client, download_range, download_single, probe, ProbeInfo, ProtoError};
