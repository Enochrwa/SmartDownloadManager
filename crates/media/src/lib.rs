//! sdm-media: `yt-dlp` + FFmpeg subprocess wrappers for video/audio
//! extraction.
//!
//! Sprint 10 (`docs/SPRINT_PLAN_PHASE2.md`): extends reach from "sites
//! with direct manifests" (Sprint 9's HLS/DASH/Metalink) to "thousands of
//! video sites," matching yt-dlp's extractor coverage without
//! reimplementing it. Both `yt-dlp` and `ffmpeg` are invoked as external
//! subprocesses, never linked — see `docs/LICENSING.md`.
//!
//! - [`ytdlp`]: metadata extraction (`--dump-json`) and the actual fetch,
//!   with progress parsed as structured events.
//! - [`ffmpeg`]: audio+video merge, subtitle embedding, thumbnail
//!   embedding, plus LGPL-build enforcement for the binary we ship.
//! - [`metadata`]: the deserialized subset of yt-dlp's JSON output this
//!   crate cares about (title, thumbnail, duration, chapters, formats,
//!   live status).
//!
//! Playlist/channel/album/podcast expansion into child jobs and
//! quality-selection UI live in `crates/engine::media`, one layer up —
//! this crate only knows how to talk to the two subprocesses.

pub mod error;
pub mod ffmpeg;
pub mod metadata;
pub mod ytdlp;

pub use error::MediaError;
pub use ffmpeg::{FfmpegBinary, FfmpegClient, SubtitleTrack};
pub use metadata::{ChapterInfo, FormatInfo, PlaylistEntry, VideoMetadata};
pub use ytdlp::{
    SubtitleFormat, UpdateOutcome, YtDlpBinary, YtDlpClient, YtDlpEvent, YtDlpEventSender,
    YtDlpFetchOutcome, YtDlpFetchRequest,
};
