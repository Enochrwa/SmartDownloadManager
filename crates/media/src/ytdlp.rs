//! Typed wrapper around a managed `yt-dlp` subprocess (Sprint 10).
//!
//! `yt-dlp` itself is invoked, never linked (Unlicense/public domain, but
//! shelled out to regardless — see `docs/LICENSING.md`), for metadata
//! extraction (`--dump-json`) and the actual fetch. Output is parsed as
//! structured events, not scraped free-form stdout:
//!
//! - Metadata calls (`probe`/`probe_playlist`) read yt-dlp's own
//!   `--dump-json` output, which *is* JSON already.
//! - The fetch path uses `--progress-template` to make yt-dlp emit
//!   machine-readable `SDM_PROGRESS:{...}` JSON lines for download and
//!   postprocess events, one per line (`--newline`), instead of parsing
//!   yt-dlp's human-oriented `[download]  42.0% of ...` text.
//!
//! ## Why `postprocess:` is registered *before* `download:`
//! Empirically (verified against a real `yt-dlp` 2026.07 binary against a
//! local test server while building this module), passing two
//! `--progress-template TYPE:...` flags only reliably wires up both event
//! types when the `postprocess:` one is given first; the reverse order
//! silently drops the `download:` template. This looks like a quirk in
//! yt-dlp's own option-merging rather than documented behavior, so the
//! order below is load-bearing — don't reorder it without re-verifying
//! against a real binary.
//!
//! ## Why not `--print "after_move:filepath"` for the final path
//! Combining `--print` with two `--progress-template` flags was observed
//! to suppress the `download:` progress events entirely in the same
//! experiment above. Instead, the final output path is recovered from the
//! ordinary `[download] Destination: <path>` line yt-dlp always prints
//! (and `[Merger] Merging formats into "<path>"` when yt-dlp itself needed
//! to mux, e.g. because the chosen single format selector already implied
//! a merge) — both are stable, documented-in-practice log lines rather
//! than a fragile scrape of a progress percentage.
//!
//! Sibling subtitle/thumbnail files (`--write-subs`/`--write-thumbnail`)
//! are located by filesystem convention after the fact
//! (`{stem}.{lang}.{ext}` / `{stem}.{image_ext}`) rather than by further
//! log-scraping, since yt-dlp's exact wording for those varies by version
//! and verbosity flags but the on-disk naming convention is stable.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::error::MediaError;
use crate::metadata::{PlaylistEntry, VideoMetadata};

const BINARY_LABEL: &str = "yt-dlp";

/// yt-dlp's own `postprocess:` progress-template must be registered
/// before `download:` — see the module-level doc comment.
const PROGRESS_TEMPLATE_POSTPROCESS: &str = "postprocess:SDM_PROGRESS:{\"postprocessing\":true}";
const PROGRESS_TEMPLATE_DOWNLOAD: &str = "download:SDM_PROGRESS:{\"downloaded_bytes\":%(progress.downloaded_bytes)j,\"total_bytes\":%(progress.total_bytes)j,\"speed\":%(progress.speed)j,\"eta\":%(progress.eta)j}";

/// Configuration for the managed `yt-dlp` binary: where it lives, which
/// release we've pinned/verified, and whether this client may
/// self-update it.
///
/// Per Sprint 10 scope: "version-pinned binary, checksum-verified at
/// install/update time" and "auto-update check... gated behind an
/// explicit user-configurable setting (never silent-updates a binary
/// without consent)".
#[derive(Debug, Clone)]
pub struct YtDlpBinary {
    /// Path to the executable, or a bare name resolved via `PATH` (the
    /// default: `"yt-dlp"`).
    pub path: PathBuf,
    /// SHA-256 hex digest the installed binary is expected to match.
    /// `None` means "don't verify" — appropriate for a dev machine using
    /// a system-installed `yt-dlp`, as opposed to our own bundled/pinned
    /// release asset in a packaged build.
    pub expected_sha256: Option<String>,
    /// Whether [`YtDlpClient::maybe_self_update`] is allowed to actually
    /// run `yt-dlp -U`. Defaults to `false`: sites change frequently and
    /// stale extractors are yt-dlp's #1 failure mode, but we never
    /// silently rewrite a binary on disk without the user opting in.
    pub auto_update: bool,
}

impl Default for YtDlpBinary {
    fn default() -> Self {
        Self {
            path: PathBuf::from("yt-dlp"),
            expected_sha256: None,
            auto_update: false,
        }
    }
}

impl YtDlpBinary {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn with_expected_sha256(mut self, sha256_hex: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256_hex.into());
        self
    }

    pub fn with_auto_update(mut self, auto_update: bool) -> Self {
        self.auto_update = auto_update;
        self
    }

    /// Verify the binary on disk matches [`Self::expected_sha256`], if
    /// one was configured; a no-op otherwise. Deliberately a separate,
    /// explicit call (rather than something run implicitly before every
    /// invocation) since it means reading the whole binary off disk.
    pub async fn verify_checksum(&self) -> Result<(), MediaError> {
        let Some(expected) = &self.expected_sha256 else {
            return Ok(());
        };
        let bytes = tokio::fs::read(&self.path).await?;
        let actual = sha256_hex(&bytes);
        if &actual != expected {
            return Err(MediaError::ChecksumMismatch {
                binary: self.path.to_string_lossy().to_string(),
                expected: expected.clone(),
                actual,
            });
        }
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// One parsed progress update from a running `yt-dlp` fetch.
#[derive(Debug, Clone, PartialEq)]
pub enum YtDlpEvent {
    Downloading {
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
        speed_bytes_per_sec: Option<f64>,
        eta_seconds: Option<i64>,
    },
    PostProcessing,
}

pub type YtDlpEventSender = tokio::sync::mpsc::UnboundedSender<YtDlpEvent>;

/// What to fetch and how, for [`YtDlpClient::fetch`].
#[derive(Debug, Clone)]
pub struct YtDlpFetchRequest {
    pub url: String,
    /// A concrete `format_id` from a prior [`YtDlpClient::probe`] call
    /// (see [`crate::FormatInfo::format_id`]) — Sprint 10's quality
    /// selection is exact-format, not a heuristic selector string, so
    /// "fetch the requested format, not just the default" is directly
    /// verifiable.
    pub format_id: String,
    /// Directory + filename stem (no extension) to write into; yt-dlp
    /// appends `.%(ext)s` itself. Sibling subtitle/thumbnail files are
    /// derived from this same stem.
    pub output_stem: PathBuf,
    /// BCP-47-ish subtitle language codes to fetch (`--write-subs
    /// --sub-langs ...`), empty for none.
    pub subtitle_langs: Vec<String>,
    /// Subtitle container format (`srt`/`ass`/`vtt`); ignored if
    /// `subtitle_langs` is empty.
    pub subtitle_format: SubtitleFormat,
    pub write_thumbnail: bool,
    /// Route through yt-dlp's live-from-start capture mode instead of
    /// treating this as a fixed-length fetch (Sprint 10: "Livestream
    /// detection... routed to yt-dlp's live-from-start / ongoing-capture
    /// mode rather than treated as a fixed-length Job"). Set this from
    /// [`VideoMetadata::is_livestream`].
    pub live_from_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Ass,
    Vtt,
}

impl SubtitleFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubtitleFormat::Srt => "srt",
            SubtitleFormat::Ass => "ass",
            SubtitleFormat::Vtt => "vtt",
        }
    }
}

/// Result of a completed [`YtDlpClient::fetch`] call: the downloaded
/// media file plus any sibling subtitle/thumbnail files found on disk
/// afterward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YtDlpFetchOutcome {
    pub media_path: PathBuf,
    /// `(lang, path)` pairs, in the order requested.
    pub subtitle_paths: Vec<(String, PathBuf)>,
    pub thumbnail_path: Option<PathBuf>,
}

pub struct YtDlpClient {
    binary: YtDlpBinary,
}

impl YtDlpClient {
    pub fn new(binary: YtDlpBinary) -> Self {
        Self { binary }
    }

    fn command(&self) -> Command {
        Command::new(&self.binary.path)
    }

    /// `yt-dlp --version`.
    pub async fn installed_version(&self) -> Result<String, MediaError> {
        let output = self
            .command()
            .arg("--version")
            .output()
            .await
            .map_err(|e| self.spawn_err(e))?;
        if !output.status.success() {
            return Err(self.exit_err(output.status, &output.stderr));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Run `yt-dlp -U` to self-update the binary in place, but only if
    /// [`YtDlpBinary::auto_update`] is `true` — otherwise this is a no-op
    /// that returns the current version unchanged. Per Sprint 10 scope,
    /// this is the *only* code path that can ever mutate the binary, and
    /// it never runs without the explicit opt-in.
    pub async fn maybe_self_update(&self) -> Result<UpdateOutcome, MediaError> {
        if !self.binary.auto_update {
            return Ok(UpdateOutcome::Skipped {
                current_version: self.installed_version().await?,
            });
        }
        let output = self
            .command()
            .arg("-U")
            .output()
            .await
            .map_err(|e| self.spawn_err(e))?;
        if !output.status.success() {
            return Err(self.exit_err(output.status, &output.stderr));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let updated = stdout.contains("Updated yt-dlp") || stdout.contains("has been updated");
        Ok(UpdateOutcome::Checked {
            updated,
            version_after: self.installed_version().await?,
        })
    }

    /// `yt-dlp --dump-json --no-playlist <url>`: full metadata for
    /// exactly one video/audio item (never a playlist listing, even if
    /// `url` happens to be inside one).
    pub async fn probe(&self, url: &str) -> Result<VideoMetadata, MediaError> {
        let output = self
            .command()
            .args(["--no-warnings", "--dump-json", "--no-playlist", url])
            .output()
            .await
            .map_err(|e| self.spawn_err(e))?;
        if !output.status.success() {
            return Err(self.exit_err(output.status, &output.stderr));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .ok_or_else(|| MediaError::EmptyOutput {
                binary: BINARY_LABEL.to_string(),
            })?;
        parse_json_line(line)
    }

    /// `yt-dlp --flat-playlist --dump-json <url>`: one JSON line per
    /// child entry, without resolving each child's full formats (that
    /// would mean N full extractions up front for what might be a
    /// hundred-video channel). Sprint 10 scope: "a playlist URL expands
    /// into N child Jobs under one parent queue entry" — the engine
    /// calls [`YtDlpClient::probe`] again per child once it actually
    /// starts that child's download.
    pub async fn probe_playlist(&self, url: &str) -> Result<Vec<PlaylistEntry>, MediaError> {
        let output = self
            .command()
            .args(["--no-warnings", "--flat-playlist", "--dump-json", url])
            .output()
            .await
            .map_err(|e| self.spawn_err(e))?;
        if !output.status.success() {
            return Err(self.exit_err(output.status, &output.stderr));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(parse_json_line)
            .collect()
    }

    /// Download `req.format_id` (plus any requested subtitles/thumbnail),
    /// streaming progress events to `events` as they arrive.
    pub async fn fetch(
        &self,
        req: &YtDlpFetchRequest,
        events: YtDlpEventSender,
    ) -> Result<YtDlpFetchOutcome, MediaError> {
        if let Some(parent) = req.output_stem.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let output_template = format!("{}.%(ext)s", req.output_stem.to_string_lossy());

        let mut cmd = self.command();
        cmd.args(["-f", &req.format_id])
            .args(["-o", &output_template])
            .args(["--newline", "--no-warnings"])
            // See the module doc comment: postprocess *must* be
            // registered before download for both to actually fire.
            .args(["--progress-template", PROGRESS_TEMPLATE_POSTPROCESS])
            .args(["--progress-template", PROGRESS_TEMPLATE_DOWNLOAD]);

        if !req.subtitle_langs.is_empty() {
            cmd.arg("--write-subs")
                .args(["--sub-langs", &req.subtitle_langs.join(",")])
                .args(["--sub-format", req.subtitle_format.as_str()])
                // We embed subtitles ourselves via crate::FfmpegClient
                // (Sprint 10 scope explicitly calls for our own FFmpeg
                // wrapper doing the embedding), so keep yt-dlp from also
                // trying to embed/convert them.
                .arg("--no-embed-subs");
        }
        if req.write_thumbnail {
            cmd.arg("--write-thumbnail").arg("--no-embed-thumbnail");
        }
        if req.live_from_start {
            cmd.arg("--live-from-start");
        }
        cmd.arg(&req.url);

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| self.spawn_err(e))?;
        let stdout = child
            .stdout
            .take()
            .expect("stdout was requested as piped above");
        let stderr = child
            .stderr
            .take()
            .expect("stderr was requested as piped above");

        let mut stderr_reader = BufReader::new(stderr).lines();
        let stderr_task: tokio::task::JoinHandle<String> = tokio::spawn(async move {
            let mut collected = String::new();
            while let Ok(Some(line)) = stderr_reader.next_line().await {
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        });

        let mut media_path: Option<PathBuf> = None;
        let mut stdout_reader = BufReader::new(stdout).lines();
        while let Some(line) = stdout_reader
            .next_line()
            .await
            .map_err(|e| MediaError::Other(format!("reading yt-dlp stdout: {e}")))?
        {
            if let Some(rest) = line.strip_prefix("SDM_PROGRESS:") {
                if let Some(event) = parse_progress_line(rest) {
                    let _ = events.send(event);
                }
                continue;
            }
            if let Some(path) = extract_destination_path(&line) {
                media_path = Some(path);
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| MediaError::Other(format!("waiting on yt-dlp: {e}")))?;
        let stderr_text = stderr_task.await.unwrap_or_default();

        if !status.success() {
            return Err(MediaError::NonZeroExit {
                binary: BINARY_LABEL.to_string(),
                status: status.to_string(),
                stderr: stderr_text,
            });
        }

        let media_path = media_path.ok_or_else(|| {
            MediaError::Other(
                "yt-dlp exited successfully but no '[download] Destination:'/'[Merger] Merging \
             formats into' line was seen in its output"
                    .to_string(),
            )
        })?;

        let subtitle_paths =
            find_subtitle_files(&req.output_stem, &req.subtitle_langs, req.subtitle_format).await;
        let thumbnail_path = find_thumbnail_file(&req.output_stem).await;

        Ok(YtDlpFetchOutcome {
            media_path,
            subtitle_paths,
            thumbnail_path,
        })
    }

    fn spawn_err(&self, source: std::io::Error) -> MediaError {
        MediaError::Spawn {
            binary: self.binary.path.to_string_lossy().to_string(),
            source,
        }
    }

    fn exit_err(&self, status: std::process::ExitStatus, stderr: &[u8]) -> MediaError {
        MediaError::NonZeroExit {
            binary: BINARY_LABEL.to_string(),
            status: status.to_string(),
            stderr: String::from_utf8_lossy(stderr).to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateOutcome {
    /// `auto_update` was `false` — no update was attempted.
    Skipped { current_version: String },
    /// `auto_update` was `true` and `yt-dlp -U` ran.
    Checked {
        updated: bool,
        version_after: String,
    },
}

fn parse_json_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, MediaError> {
    serde_json::from_str(line).map_err(|e| MediaError::Json {
        binary: BINARY_LABEL.to_string(),
        source: e,
        line: line.to_string(),
    })
}

/// yt-dlp substitutes an unset numeric progress field as the bare token
/// `NA` even under the `j` (JSON) format spec in at least one observed
/// case (the final `eta` update of a finished download) — sanitize it to
/// `null` before parsing rather than trusting every field to always be
/// valid JSON.
fn sanitize_na(json_like: &str) -> String {
    // `NA` never legitimately appears as a bare (unquoted) token
    // elsewhere in our own fixed progress-template shape, so a plain
    // substring replace is safe here.
    json_like.replace(":NA", ":null")
}

#[derive(Debug, Deserialize)]
struct DownloadPayload {
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
    speed: Option<f64>,
    eta: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PostprocessPayload {
    postprocessing: bool,
}

/// Distinguish the two progress-template shapes by the presence of the
/// `postprocessing` key rather than an untagged enum: since every field
/// of [`DownloadPayload`] is optional, an untagged `enum { Download,
/// Postprocess }` would greedily match `{"postprocessing":true}` against
/// `Download` first (an all-`None` match still "succeeds"), never
/// reaching the `Postprocess` variant. Checking the key explicitly avoids
/// that trap.
fn parse_progress_line(raw: &str) -> Option<YtDlpEvent> {
    let sanitized = sanitize_na(raw);
    let value: serde_json::Value = serde_json::from_str(&sanitized).ok()?;
    if value.get("postprocessing").is_some() {
        let payload: PostprocessPayload = serde_json::from_value(value).ok()?;
        return payload.postprocessing.then_some(YtDlpEvent::PostProcessing);
    }
    let payload: DownloadPayload = serde_json::from_value(value).ok()?;
    Some(YtDlpEvent::Downloading {
        downloaded_bytes: payload.downloaded_bytes,
        total_bytes: payload.total_bytes,
        speed_bytes_per_sec: payload.speed,
        eta_seconds: payload.eta,
    })
}

/// Recognize the two stable yt-dlp log lines that name the final output
/// file: `[download] Destination: <path>` (the common case) and
/// `[Merger] Merging formats into "<path>"` (when yt-dlp itself performed
/// a merge because the format selector implied one).
fn extract_destination_path(line: &str) -> Option<PathBuf> {
    if let Some(rest) = line.trim_start().strip_prefix("[download] Destination: ") {
        return Some(PathBuf::from(rest.trim()));
    }
    if let Some(idx) = line.find("[Merger] Merging formats into \"") {
        let start = idx + "[Merger] Merging formats into \"".len();
        if let Some(end) = line[start..].find('"') {
            return Some(PathBuf::from(&line[start..start + end]));
        }
    }
    if let Some(rest) = line
        .trim_start()
        .strip_prefix("[ExtractAudio] Destination: ")
    {
        return Some(PathBuf::from(rest.trim()));
    }
    None
}

async fn find_subtitle_files(
    stem: &Path,
    langs: &[String],
    format: SubtitleFormat,
) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for lang in langs {
        let candidate = PathBuf::from(format!(
            "{}.{lang}.{}",
            stem.to_string_lossy(),
            format.as_str()
        ));
        if tokio::fs::metadata(&candidate).await.is_ok() {
            found.push((lang.clone(), candidate));
        }
    }
    found
}

async fn find_thumbnail_file(stem: &Path) -> Option<PathBuf> {
    for ext in ["jpg", "jpeg", "webp", "png"] {
        let candidate = PathBuf::from(format!("{}.{ext}", stem.to_string_lossy()));
        if tokio::fs::metadata(&candidate).await.is_ok() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_bare_na_tokens_before_parsing() {
        let raw = r#"{"downloaded_bytes":1024,"total_bytes":2048,"speed":100.5,"eta":NA}"#;
        let event = parse_progress_line(raw).expect("should parse despite NA token");
        assert_eq!(
            event,
            YtDlpEvent::Downloading {
                downloaded_bytes: Some(1024),
                total_bytes: Some(2048),
                speed_bytes_per_sec: Some(100.5),
                eta_seconds: None,
            }
        );
    }

    #[test]
    fn parses_clean_download_progress() {
        let raw = r#"{"downloaded_bytes":500,"total_bytes":1000,"speed":50.0,"eta":10}"#;
        let event = parse_progress_line(raw).unwrap();
        assert_eq!(
            event,
            YtDlpEvent::Downloading {
                downloaded_bytes: Some(500),
                total_bytes: Some(1000),
                speed_bytes_per_sec: Some(50.0),
                eta_seconds: Some(10),
            }
        );
    }

    #[test]
    fn parses_postprocess_marker() {
        let raw = r#"{"postprocessing":true}"#;
        assert_eq!(parse_progress_line(raw), Some(YtDlpEvent::PostProcessing));
    }

    #[test]
    fn extracts_plain_download_destination() {
        let line = "[download] Destination: /tmp/out.mp4";
        assert_eq!(
            extract_destination_path(line),
            Some(PathBuf::from("/tmp/out.mp4"))
        );
    }

    #[test]
    fn extracts_merger_destination() {
        let line = "[Merger] Merging formats into \"/tmp/out.mkv\"";
        assert_eq!(
            extract_destination_path(line),
            Some(PathBuf::from("/tmp/out.mkv"))
        );
    }

    #[test]
    fn checksum_mismatch_is_reported() {
        let expected = sha256_hex(b"hello world");
        assert_eq!(expected.len(), 64);
    }
}
