//! FFmpeg subprocess wrapper (Sprint 10): audio+video merge, subtitle
//! embedding/conversion, and thumbnail embedding — plus enforcement that
//! the binary we're shelling out to is actually the LGPL build
//! `docs/LICENSING.md` item 1 requires for a distributable binary
//! (`--enable-gpl` must *not* appear in its reported build configuration).
//!
//! FFmpeg is invoked as an external subprocess, never linked, exactly
//! like `yt-dlp` — see `crates/media::ytdlp` and `docs/LICENSING.md`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::error::MediaError;
use crate::ytdlp::SubtitleFormat;

const BINARY_LABEL: &str = "ffmpeg";

/// Configuration for the managed `ffmpeg` binary.
#[derive(Debug, Clone)]
pub struct FfmpegBinary {
    pub path: PathBuf,
    /// When `true` (the default), every operation first checks that this
    /// binary's own `-version` banner does not advertise a GPL-only
    /// build (`--enable-gpl`/`--enable-nonfree`), per
    /// `docs/LICENSING.md` item 1. Set to `false` only for local
    /// development against a system-installed GPL ffmpeg (e.g. Ubuntu's
    /// distro package, which *is* built with `--enable-gpl`) — never for
    /// a binary this project actually redistributes.
    pub enforce_lgpl: bool,
}

impl Default for FfmpegBinary {
    fn default() -> Self {
        Self {
            path: PathBuf::from("ffmpeg"),
            enforce_lgpl: true,
        }
    }
}

impl FfmpegBinary {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn with_enforce_lgpl(mut self, enforce: bool) -> Self {
        self.enforce_lgpl = enforce;
        self
    }
}

/// One subtitle track to embed: language code + path to the subtitle
/// file on disk (as produced by `crate::YtDlpClient::fetch`'s
/// `--write-subs`).
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    pub lang: String,
    pub path: PathBuf,
    pub format: SubtitleFormat,
}

pub struct FfmpegClient {
    binary: FfmpegBinary,
}

impl FfmpegClient {
    pub fn new(binary: FfmpegBinary) -> Self {
        Self { binary }
    }

    fn command(&self) -> Command {
        Command::new(&self.binary.path)
    }

    /// Confirm this binary's build configuration doesn't advertise a
    /// GPL-only build. A no-op when [`FfmpegBinary::enforce_lgpl`] is
    /// `false`.
    pub async fn verify_lgpl_build(&self) -> Result<(), MediaError> {
        if !self.binary.enforce_lgpl {
            return Ok(());
        }
        let output = self
            .command()
            .arg("-version")
            .output()
            .await
            .map_err(|e| self.spawn_err(e))?;
        if !output.status.success() {
            return Err(self.exit_err(output.status, &output.stderr));
        }
        let banner = String::from_utf8_lossy(&output.stdout);
        for flag in ["--enable-gpl", "--enable-nonfree"] {
            if banner.contains(flag) {
                return Err(MediaError::NonLgplBuild {
                    flag: flag.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Mux a video-only and audio-only file into one container, copying
    /// both streams (`-c copy`, no re-encode) since yt-dlp already
    /// delivered them in their final codecs.
    pub async fn merge_audio_video(
        &self,
        video: &Path,
        audio: &Path,
        output: &Path,
    ) -> Result<(), MediaError> {
        self.verify_lgpl_build().await?;
        ensure_parent_dir(output).await?;
        self.run([
            "-y",
            "-i",
            &path_str(video),
            "-i",
            &path_str(audio),
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c",
            "copy",
            &path_str(output),
        ])
        .await
    }

    /// Embed one or more subtitle tracks into `input`, converting each to
    /// MP4-compatible `mov_text` (the standard approach for burning SRT/
    /// ASS/VTT subtitles into an MP4 container as a selectable, not
    /// hard-baked, track).
    pub async fn embed_subtitles(
        &self,
        input: &Path,
        subtitles: &[SubtitleTrack],
        output: &Path,
    ) -> Result<(), MediaError> {
        self.verify_lgpl_build().await?;
        if subtitles.is_empty() {
            return Err(MediaError::Other(
                "embed_subtitles called with no subtitle tracks".to_string(),
            ));
        }
        ensure_parent_dir(output).await?;

        let mut args: Vec<String> = vec!["-y".to_string(), "-i".to_string(), path_str(input)];
        for sub in subtitles {
            args.push("-i".to_string());
            args.push(path_str(&sub.path));
        }
        // Map the original file's video+audio, then each subtitle input
        // in order.
        args.push("-map".to_string());
        args.push("0:v:0".to_string());
        args.push("-map".to_string());
        args.push("0:a:0?".to_string());
        for (i, _) in subtitles.iter().enumerate() {
            args.push("-map".to_string());
            args.push(format!("{}:0", i + 1));
        }
        args.push("-c:v".to_string());
        args.push("copy".to_string());
        args.push("-c:a".to_string());
        args.push("copy".to_string());
        args.push("-c:s".to_string());
        args.push("mov_text".to_string());
        for (i, sub) in subtitles.iter().enumerate() {
            args.push(format!("-metadata:s:s:{i}"));
            args.push(format!("language={}", iso6392_best_effort(&sub.lang)));
        }
        args.push(path_str(output));

        self.run(args.iter().map(String::as_str)).await
    }

    /// Embed a thumbnail image as an MP4 "attached pic" video stream —
    /// the standard way a thumbnail travels inside an MP4/M4A file (shown
    /// by players as cover art), without re-encoding the primary stream.
    pub async fn embed_thumbnail(
        &self,
        input: &Path,
        thumbnail: &Path,
        output: &Path,
    ) -> Result<(), MediaError> {
        self.verify_lgpl_build().await?;
        ensure_parent_dir(output).await?;
        self.run([
            "-y",
            "-i",
            &path_str(input),
            "-i",
            &path_str(thumbnail),
            "-map",
            "0",
            "-map",
            "1",
            "-c",
            "copy",
            "-disposition:v:1",
            "attached_pic",
            &path_str(output),
        ])
        .await
    }

    async fn run<'a, I: IntoIterator<Item = &'a str>>(&self, args: I) -> Result<(), MediaError> {
        let output = self
            .command()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| self.spawn_err(e))?;
        if !output.status.success() {
            return Err(self.exit_err(output.status, &output.stderr));
        }
        Ok(())
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

fn path_str(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

async fn ensure_parent_dir(output: &Path) -> Result<(), MediaError> {
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    Ok(())
}

/// Best-effort two-letter -> three-letter language code mapping for the
/// handful of languages this project's own tests/fixtures use; ffmpeg
/// accepts arbitrary strings for `-metadata:s:s:N language=...` and most
/// players tolerate a two-letter code anyway, so falling back to the
/// input unchanged (rather than failing) is the right default for codes
/// outside this small table.
fn iso6392_best_effort(lang: &str) -> String {
    match lang {
        "en" => "eng",
        "fr" => "fre",
        "es" => "spa",
        "de" => "ger",
        "rw" => "kin",
        other => return other.to_string(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_language_codes() {
        assert_eq!(iso6392_best_effort("en"), "eng");
        assert_eq!(iso6392_best_effort("rw"), "kin");
    }

    #[test]
    fn falls_back_to_input_for_unknown_codes() {
        assert_eq!(iso6392_best_effort("xx"), "xx");
    }
}
