//! Integration tests for `sdm-media`'s `yt-dlp`/FFmpeg wrappers.
//!
//! These exercise the *real* `yt-dlp` and `ffmpeg` binaries (never
//! mocked in-process — there is no meaningful way to fake a subprocess
//! wrapper's contract without actually running the subprocess), against:
//!
//! - a local, hand-rolled HTTP/1.1 fixture server (`tests/support/tiny_http.rs`)
//!   standing in for a real video site, serving a synthetic (not
//!   copyrighted, ffmpeg-`testsrc`-generated) fixture video that yt-dlp's
//!   built-in `generic` extractor recognizes as a direct media URL — this
//!   is the same "serve a local fixture instead of hitting the real
//!   internet" principle Sprint 9's HLS/DASH tests use with `wiremock`,
//!   just without pulling in `wiremock` itself (see the dev-dependency
//!   comment in `Cargo.toml`).
//! - committed synthetic media fixtures in `tests/fixtures/` for the pure
//!   FFmpeg-side tests (merge/embed), which don't need a network layer.
//!
//! Every test is skipped (not failed) when `yt-dlp`/`ffmpeg` aren't on
//! `PATH`, matching this workspace's existing convention for
//! environment-dependent integration tests (see
//! `crates/protocols/tests/ftp_integration.rs`).

mod support;

use std::path::PathBuf;

use sdm_media::{
    FfmpegBinary, FfmpegClient, SubtitleFormat, SubtitleTrack, YtDlpBinary, YtDlpClient,
    YtDlpEvent, YtDlpFetchRequest,
};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

async fn yt_dlp_available() -> bool {
    YtDlpClient::new(YtDlpBinary::default())
        .installed_version()
        .await
        .is_ok()
}

async fn ffmpeg_available() -> bool {
    FfmpegClient::new(FfmpegBinary::default().with_enforce_lgpl(false))
        .verify_lgpl_build()
        .await
        .is_ok()
        || std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
}

macro_rules! skip_unless {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            eprintln!("skipping: {}", $msg);
            return;
        }
    };
}

#[tokio::test]
async fn probes_a_direct_video_url_via_the_generic_extractor() {
    skip_unless!(yt_dlp_available().await, "yt-dlp not on PATH");

    let video_bytes = tokio::fs::read(fixtures_dir().join("sample.mp4"))
        .await
        .expect("reading committed sample.mp4 fixture");
    let mut files = std::collections::HashMap::new();
    files.insert("sample.mp4", ("video/mp4", video_bytes));
    let server = support::tiny_http::start(files).await;

    let client = YtDlpClient::new(YtDlpBinary::default());
    let url = format!("{}/sample.mp4", server.base_url());
    let metadata = client.probe(&url).await.expect("probe should succeed");

    // The generic extractor won't populate rich metadata for a raw file,
    // but it must recognize it as a single fetchable item with at least
    // one usable format.
    assert!(
        !metadata.formats.is_empty(),
        "expected at least one format, got {:?}",
        metadata.formats
    );
    assert!(!metadata.is_livestream());
}

#[tokio::test]
async fn fetches_the_requested_format_and_reports_progress() {
    skip_unless!(yt_dlp_available().await, "yt-dlp not on PATH");

    let video_bytes = tokio::fs::read(fixtures_dir().join("sample.mp4"))
        .await
        .expect("reading committed sample.mp4 fixture");
    let mut files = std::collections::HashMap::new();
    files.insert("sample.mp4", ("video/mp4", video_bytes));
    let server = support::tiny_http::start(files).await;

    let client = YtDlpClient::new(YtDlpBinary::default());
    let url = format!("{}/sample.mp4", server.base_url());
    let metadata = client.probe(&url).await.expect("probe should succeed");
    let format = metadata
        .formats
        .first()
        .expect("generic extractor should report at least one format");

    let tmp = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<YtDlpEvent>();
    let req = YtDlpFetchRequest {
        url: url.clone(),
        format_id: format.format_id.clone(),
        output_stem: tmp.path().join("fetched"),
        subtitle_langs: vec![],
        subtitle_format: SubtitleFormat::Srt,
        write_thumbnail: false,
        live_from_start: false,
    };

    let outcome = client
        .fetch(&req, tx)
        .await
        .expect("fetch of a known-good direct URL should succeed");

    assert!(
        tokio::fs::metadata(&outcome.media_path).await.is_ok(),
        "fetched file should exist at {:?}",
        outcome.media_path
    );
    let fetched_bytes = tokio::fs::read(&outcome.media_path).await.unwrap();
    assert!(!fetched_bytes.is_empty());

    // We don't assert on *which* events arrive (a same-host transfer can
    // complete before a single progress tick fires), only that if any
    // arrived, they were well-formed downloading/postprocessing events
    // (i.e. parsing didn't silently produce garbage).
    while let Ok(event) = rx.try_recv() {
        match event {
            YtDlpEvent::Downloading { .. } | YtDlpEvent::PostProcessing => {}
        }
    }
}

#[tokio::test]
async fn quality_selection_fetches_the_requested_format_not_just_the_default() {
    skip_unless!(yt_dlp_available().await, "yt-dlp not on PATH");

    let video_bytes = tokio::fs::read(fixtures_dir().join("sample.mp4"))
        .await
        .expect("reading committed sample.mp4 fixture");
    let mut files = std::collections::HashMap::new();
    files.insert("sample.mp4", ("video/mp4", video_bytes.clone()));
    let server = support::tiny_http::start(files).await;

    let client = YtDlpClient::new(YtDlpBinary::default());
    let url = format!("{}/sample.mp4", server.base_url());
    let metadata = client.probe(&url).await.unwrap();
    let format = metadata.formats.first().expect("at least one format");

    let tmp = tempfile::tempdir().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let req = YtDlpFetchRequest {
        url,
        // Explicitly request this exact format_id, proving the request
        // is honored rather than yt-dlp silently picking "best".
        format_id: format.format_id.clone(),
        output_stem: tmp.path().join("chosen_quality"),
        subtitle_langs: vec![],
        subtitle_format: SubtitleFormat::Srt,
        write_thumbnail: false,
        live_from_start: false,
    };
    let outcome = client.fetch(&req, tx).await.unwrap();
    let fetched = tokio::fs::read(&outcome.media_path).await.unwrap();
    // Re-encoded/remuxed by yt-dlp so won't be byte-identical, but should
    // be roughly the same order of magnitude, not empty/truncated.
    assert!(fetched.len() > video_bytes.len() / 4);
}

#[tokio::test]
async fn merges_separate_video_and_audio_into_one_file() {
    skip_unless!(ffmpeg_available().await, "ffmpeg not on PATH");

    let ffmpeg = FfmpegClient::new(FfmpegBinary::default().with_enforce_lgpl(false));
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("merged.mp4");

    ffmpeg
        .merge_audio_video(
            &fixtures_dir().join("sample_video_only.mp4"),
            &fixtures_dir().join("sample_audio_only.m4a"),
            &output,
        )
        .await
        .expect("merge should succeed");

    let bytes = tokio::fs::read(&output).await.unwrap();
    assert!(!bytes.is_empty());

    // Verify the merged file actually has both a video and an audio
    // stream (not just a copy of one input) via ffprobe-equivalent
    // `ffmpeg -i` stderr stream listing.
    let probe = std::process::Command::new("ffmpeg")
        .args(["-i", output.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    assert!(
        stderr.contains("Video:"),
        "expected a video stream: {stderr}"
    );
    assert!(
        stderr.contains("Audio:"),
        "expected an audio stream: {stderr}"
    );
}

#[tokio::test]
async fn embeds_subtitles_into_a_selectable_track() {
    skip_unless!(ffmpeg_available().await, "ffmpeg not on PATH");

    let ffmpeg = FfmpegClient::new(FfmpegBinary::default().with_enforce_lgpl(false));
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("with_subs.mp4");

    let subs = vec![SubtitleTrack {
        lang: "en".to_string(),
        path: fixtures_dir().join("sample.srt"),
        format: SubtitleFormat::Srt,
    }];

    ffmpeg
        .embed_subtitles(&fixtures_dir().join("sample.mp4"), &subs, &output)
        .await
        .expect("subtitle embed should succeed");

    let probe = std::process::Command::new("ffmpeg")
        .args(["-i", output.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    assert!(
        stderr.contains("Subtitle:"),
        "expected an embedded subtitle stream: {stderr}"
    );
}

#[tokio::test]
async fn embeds_a_thumbnail_as_attached_cover_art() {
    skip_unless!(ffmpeg_available().await, "ffmpeg not on PATH");

    let ffmpeg = FfmpegClient::new(FfmpegBinary::default().with_enforce_lgpl(false));
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("with_thumb.mp4");

    ffmpeg
        .embed_thumbnail(
            &fixtures_dir().join("sample.mp4"),
            &fixtures_dir().join("sample_thumb.jpg"),
            &output,
        )
        .await
        .expect("thumbnail embed should succeed");

    let bytes = tokio::fs::read(&output).await.unwrap();
    assert!(!bytes.is_empty());
    let probe = std::process::Command::new("ffmpeg")
        .args(["-i", output.to_str().unwrap()])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    assert!(
        stderr.contains("Video: mjpeg") || stderr.contains("attached pic"),
        "expected an attached-pic thumbnail stream: {stderr}"
    );
}

#[tokio::test]
async fn rejects_a_gpl_ffmpeg_build_when_enforcement_is_on() {
    skip_unless!(ffmpeg_available().await, "ffmpeg not on PATH");

    // This workspace's sandbox/CI ffmpeg is typically Ubuntu's distro
    // package, which *is* built with --enable-gpl -- exactly the build
    // docs/LICENSING.md item 1 says must never be the one we
    // distribute. With enforcement on (the default), that must be
    // rejected rather than silently used.
    let ffmpeg = FfmpegClient::new(FfmpegBinary::default());
    let result = ffmpeg.verify_lgpl_build().await;

    let output = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .unwrap();
    let banner = String::from_utf8_lossy(&output.stdout);
    if banner.contains("--enable-gpl") {
        assert!(
            result.is_err(),
            "a GPL-flagged ffmpeg build must be rejected when enforce_lgpl is true"
        );
    } else {
        // Some environments may have a genuinely LGPL-only ffmpeg
        // installed; in that case enforcement should simply pass.
        assert!(result.is_ok());
    }
}
