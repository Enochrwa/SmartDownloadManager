//! sdm-protocols: protocol implementations.
//!
//! HTTP/HTTPS lands in Sprint 1. FTP/SFTP/WebDAV/Metalink land Phase 1-2.
//! HLS (via `m3u8-rs`) and MPEG-DASH (via `dash-mpd-rs`) land Phase 2,
//! Sprint 9 — see docs/SPRINT_PLAN.md and docs/TECH_DECISIONS.md §5.

pub mod http;
