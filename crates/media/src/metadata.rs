//! Structured metadata extracted from `yt-dlp --dump-json` output.
//!
//! We deserialize only the subset of fields Sprint 10 needs (title,
//! thumbnail, duration, chapters, formats, live status) rather than
//! modeling yt-dlp's entire (large, extractor-dependent) JSON shape 1:1.
//! Every field is permissive (`Option`/`#[serde(default)]`) because
//! different extractors populate different subsets — e.g. the `generic`
//! extractor (used by direct-file URLs, and by this crate's own
//! integration tests) supplies almost none of the rich fields a YouTube
//! extraction does — and unknown JSON fields are ignored automatically by
//! serde rather than causing a hard deserialize error, since yt-dlp adds
//! extractor-specific fields we deliberately don't model.

use serde::{Deserialize, Serialize};

/// One entry of `yt-dlp --dump-json`'s `"formats"` array: a single
/// selectable quality/codec combination for a video.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FormatInfo {
    pub format_id: String,
    #[serde(default)]
    pub ext: Option<String>,
    #[serde(default)]
    pub vcodec: Option<String>,
    #[serde(default)]
    pub acodec: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Total bitrate in kbps, when known.
    #[serde(default)]
    pub tbr: Option<f64>,
    #[serde(default)]
    pub filesize: Option<u64>,
    #[serde(default)]
    pub filesize_approx: Option<u64>,
    #[serde(default)]
    pub format_note: Option<String>,
}

impl FormatInfo {
    /// A "144p"-"8K"-style label for display in a quality picker, falling
    /// back to whatever descriptive text yt-dlp did supply for
    /// audio-only formats (which have no `height`).
    pub fn quality_label(&self) -> String {
        match self.height {
            Some(h) => format!("{h}p"),
            None => self
                .format_note
                .clone()
                .unwrap_or_else(|| self.format_id.clone()),
        }
    }

    /// A format has a video stream unless yt-dlp explicitly says
    /// `vcodec: "none"` (its convention for "this format is audio-only").
    pub fn has_video(&self) -> bool {
        !matches!(self.vcodec.as_deref(), Some("none"))
    }

    /// Mirror of [`FormatInfo::has_video`] for the audio side.
    pub fn has_audio(&self) -> bool {
        !matches!(self.acodec.as_deref(), Some("none"))
    }
}

/// One chapter marker, as surfaced through the Job model per Sprint 10's
/// "site metadata surfaced through the Job model" scope item.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChapterInfo {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub start_time: Option<f64>,
    #[serde(default)]
    pub end_time: Option<f64>,
}

/// One entry of a `--flat-playlist --dump-json` listing: cheap metadata
/// (no per-video format resolution — that would mean N full extractions
/// up front) used to expand a playlist/channel/album/podcast URL into N
/// child jobs before probing each child individually.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    #[serde(default)]
    pub id: Option<String>,
    /// The entry's own watchable URL — usually present for flat-playlist
    /// listings; falls back to `id` if a caller needs a stable key when
    /// it's absent (rare, extractor-dependent).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub ie_key: Option<String>,
}

/// Full metadata for a single video/audio item, as returned by
/// `yt-dlp --dump-json --no-playlist <url>` — always describes exactly
/// one playable item (see [`crate::YtDlpClient::probe`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoMetadata {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    /// Seconds, when known. `None` for an ongoing livestream.
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub chapters: Vec<ChapterInfo>,
    #[serde(default)]
    pub formats: Vec<FormatInfo>,
    #[serde(default)]
    pub is_live: bool,
    /// yt-dlp's more granular field: `"is_live"`, `"is_upcoming"`,
    /// `"was_live"`, `"post_live"`, or `"not_live"`.
    #[serde(default)]
    pub live_status: Option<String>,
    #[serde(default)]
    pub webpage_url: Option<String>,
    #[serde(default)]
    pub extractor: Option<String>,
}

impl VideoMetadata {
    /// True if yt-dlp's live-status fields indicate this is an ongoing or
    /// not-yet-started livestream — Sprint 10 scope: routed to yt-dlp's
    /// live-from-start / ongoing-capture mode rather than treated as a
    /// fixed-length Job (see [`crate::YtDlpFetchRequest::live_from_start`]).
    pub fn is_livestream(&self) -> bool {
        self.is_live
            || matches!(
                self.live_status.as_deref(),
                Some("is_live") | Some("is_upcoming") | Some("post_live")
            )
    }

    /// Formats that carry a video stream, in the order yt-dlp listed
    /// them (ascending quality, by convention).
    pub fn video_formats(&self) -> Vec<&FormatInfo> {
        self.formats.iter().filter(|f| f.has_video()).collect()
    }

    /// Formats that carry an audio stream.
    pub fn audio_formats(&self) -> Vec<&FormatInfo> {
        self.formats.iter().filter(|f| f.has_audio()).collect()
    }
}
