//! sdm-protocols: protocol implementations.
//!
//! HTTP/HTTPS lands in Sprint 1 (single-stream) and Sprint 2 (segmented,
//! via Range requests). FTP/FTPS lands Sprint 7 (Phase 1 carryover);
//! SFTP/SCP/WebDAV land Sprint 8 (this module set). HLS (via `m3u8-rs`)
//! and MPEG-DASH (via `dash-mpd-rs`) land Phase 2, Sprint 9 — see
//! docs/SPRINT_PLAN_PHASE2.md and docs/TECH_DECISIONS.md §5.

pub mod dash;
pub mod error;
pub mod ftp;
pub mod hls;
pub mod http;
pub mod metalink;
pub mod scp;
pub mod sftp;
pub mod ssh;
pub mod webdav;

pub use error::ErrorClass;
pub use http::{
    build_client, build_client_with_proxy, download_range, download_single, probe, ProbeInfo,
    ProtoError, ProxyConfig,
};
pub use ssh::{HostKeyPolicy, SshAuth, SshProtoError, SshSession, SshUrl};
