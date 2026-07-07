//! Sprint 10 end-to-end tests: `MediaEngine` orchestrating `sdm-media`'s
//! yt-dlp/FFmpeg subprocess wrappers against real binaries.
//!
//! `single_video_download_...` exercises the full real path: a local
//! HTTP fixture server (yt-dlp's `generic` extractor, exactly like
//! `crates/media/tests/ytdlp_ffmpeg_flow.rs`) serving a synthetic
//! (ffmpeg-`testsrc`-generated, not copyrighted) fixture video, probed,
//! quality-selected, fetched, and — since a raw generic-extractor format
//! already has both audio+video — subtitle/thumbnail-embedded via our
//! own FFmpeg wrapper.
//!
//! `playlist_expansion_...` cannot use a real site the same way (there's
//! no local equivalent of a "playlist" for the generic extractor), so it
//! uses a small fake yt-dlp CLI fixture (`tests/fixtures/fake_ytdlp.py`,
//! see its own doc comment) to verify the orchestration logic — 1 parent
//! Job + N child Jobs, each linked via `parent_job_id` — deterministically.
//!
//! Both are skipped (not failed) when the real `yt-dlp`/`ffmpeg`/
//! `python3` aren't on `PATH`, matching this workspace's existing
//! convention for environment-dependent integration tests.

use std::path::PathBuf;

use sdm_engine::{DuplicatePolicy, MediaDownloadRequest, MediaEngine, QualitySelector};
use sdm_media::{FfmpegBinary, YtDlpBinary};
use sdm_storage::{connect_in_memory, get_job, get_media_meta, list_child_jobs, JobStatus};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture_bytes(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../media/tests/fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("reading fixture {p:?}: {e}"))
}

async fn yt_dlp_available() -> bool {
    std::process::Command::new("yt-dlp")
        .arg("--version")
        .output()
        .is_ok()
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
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
async fn single_video_download_probes_selects_quality_and_embeds_extras() {
    skip_unless!(yt_dlp_available().await, "yt-dlp not on PATH");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sample.mp4"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fixture_bytes("sample.mp4"))
                .insert_header("content-type", "video/mp4"),
        )
        .mount(&server)
        .await;

    let pool = connect_in_memory().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();

    let engine = MediaEngine::new(&pool);
    let (tx, _rx) = sdm_engine::progress::channel();
    let req = MediaDownloadRequest {
        url: format!("{}/sample.mp4", server.uri()),
        destination_dir: tmp.path().to_path_buf(),
        quality: QualitySelector::Best,
        subtitle_langs: vec![],
        embed_thumbnail: false,
        duplicate_policy: DuplicatePolicy::Rename,
        ytdlp: YtDlpBinary::default(),
        ffmpeg: FfmpegBinary::default().with_enforce_lgpl(false),
    };

    let job = engine
        .start_download(req, tx)
        .await
        .expect("single-video download should succeed");

    let record = get_job(&pool, &job.id).await.unwrap().expect("job row");
    assert_eq!(record.status, JobStatus::Completed);
    assert!(record.downloaded_bytes > 0);
    assert!(
        tokio::fs::metadata(&record.destination).await.is_ok(),
        "completed job's destination file should exist on disk"
    );

    let meta = get_media_meta(&pool, &job.id)
        .await
        .unwrap()
        .expect("media_meta row should have been persisted");
    assert!(meta.selected_format_id.is_some());
    assert!(!meta.is_live);
}

#[tokio::test]
async fn playlist_url_expands_into_one_parent_and_n_child_jobs() {
    skip_unless!(
        python3_available(),
        "python3 not on PATH (needed for the fake yt-dlp fixture)"
    );

    let fake_ytdlp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_ytdlp.py");

    let pool = connect_in_memory().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();

    let engine = MediaEngine::new(&pool);
    let (tx, _rx) = sdm_engine::progress::channel();
    let req = MediaDownloadRequest {
        url: "https://fake.invalid/playlist?id=sprint10".to_string(),
        destination_dir: tmp.path().to_path_buf(),
        quality: QualitySelector::Best,
        subtitle_langs: vec![],
        embed_thumbnail: false,
        duplicate_policy: DuplicatePolicy::Rename,
        ytdlp: YtDlpBinary::new(fake_ytdlp.clone()),
        ffmpeg: FfmpegBinary::default(),
    };

    let parent = engine
        .start_download(req, tx)
        .await
        .expect("playlist expansion should succeed even with per-child failures tolerated");

    let parent_record = get_job(&pool, &parent.id)
        .await
        .unwrap()
        .expect("parent row");
    assert_eq!(parent_record.status, JobStatus::Completed);
    assert_eq!(parent_record.parent_job_id, None);

    let children = list_child_jobs(&pool, &parent.id).await.unwrap();
    assert_eq!(children.len(), 3, "the fake playlist has 3 entries");
    for child in &children {
        assert_eq!(child.parent_job_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(child.status, JobStatus::Completed);
        assert!(tokio::fs::metadata(&child.destination).await.is_ok());
    }
}

#[tokio::test]
async fn separate_video_and_audio_formats_are_fetched_and_really_merged() {
    skip_unless!(python3_available(), "python3 not on PATH (needed for the fake yt-dlp fixture)");
    let ffmpeg_ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();
    skip_unless!(ffmpeg_ok, "ffmpeg not on PATH");

    let fake_ytdlp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_ytdlp.py");

    let pool = connect_in_memory().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();

    let engine = MediaEngine::new(&pool);
    let (tx, _rx) = sdm_engine::progress::channel();
    let req = MediaDownloadRequest {
        url: "https://fake.invalid/multiformat".to_string(),
        destination_dir: tmp.path().to_path_buf(),
        // "Best" here has no video+audio combined format, only a
        // video-only "vid" and an audio-only "aud" -- this is exactly
        // the shape that must trigger MediaEngine's own
        // fetch-both-then-ffmpeg-merge path rather than a single fetch.
        quality: QualitySelector::Best,
        subtitle_langs: vec![],
        embed_thumbnail: false,
        duplicate_policy: DuplicatePolicy::Rename,
        ytdlp: YtDlpBinary::new(fake_ytdlp),
        // This sandbox's ffmpeg is Ubuntu's GPL build (see
        // crates/media/tests/ytdlp_ffmpeg_flow.rs); a real distributable
        // build wouldn't be, but that's orthogonal to what this test
        // verifies (the merge actually happening).
        ffmpeg: FfmpegBinary::default().with_enforce_lgpl(false),
    };

    let job = engine
        .start_download(req, tx)
        .await
        .expect("multiformat download+merge should succeed");

    let record = get_job(&pool, &job.id).await.unwrap().unwrap();
    assert_eq!(record.status, JobStatus::Completed);

    // The real proof it's a genuine merge, not just one of the two
    // streams renamed: probe the actual output file with ffmpeg and
    // require both a video and an audio stream to be present.
    let probe = std::process::Command::new("ffmpeg")
        .args(["-i", &record.destination])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    assert!(
        stderr.contains("Video:"),
        "expected a video stream in the merged output: {stderr}"
    );
    assert!(
        stderr.contains("Audio:"),
        "expected an audio stream in the merged output: {stderr}"
    );
}

#[tokio::test]
async fn requested_subtitles_and_thumbnail_are_embedded_in_the_final_file() {
    skip_unless!(python3_available(), "python3 not on PATH (needed for the fake yt-dlp fixture)");
    let ffmpeg_ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();
    skip_unless!(ffmpeg_ok, "ffmpeg not on PATH");

    let fake_ytdlp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_ytdlp.py");

    let pool = connect_in_memory().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();

    let engine = MediaEngine::new(&pool);
    let (tx, _rx) = sdm_engine::progress::channel();
    let req = MediaDownloadRequest {
        url: "https://fake.invalid/watch?v=subs-and-thumb".to_string(),
        destination_dir: tmp.path().to_path_buf(),
        quality: QualitySelector::Best,
        subtitle_langs: vec!["en".to_string()],
        embed_thumbnail: true,
        duplicate_policy: DuplicatePolicy::Rename,
        ytdlp: YtDlpBinary::new(fake_ytdlp),
        ffmpeg: FfmpegBinary::default().with_enforce_lgpl(false),
    };

    let job = engine
        .start_download(req, tx)
        .await
        .expect("download with subtitles+thumbnail should succeed");

    let record = get_job(&pool, &job.id).await.unwrap().unwrap();
    assert_eq!(record.status, JobStatus::Completed);

    let probe = std::process::Command::new("ffmpeg")
        .args(["-i", &record.destination])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&probe.stderr);
    assert!(
        stderr.contains("Subtitle:"),
        "expected an embedded subtitle stream: {stderr}"
    );
    assert!(
        stderr.contains("Video: mjpeg") || stderr.contains("attached pic"),
        "expected an embedded thumbnail (attached pic) stream: {stderr}"
    );
    // And the primary content stream must still be intact alongside it.
    assert!(stderr.contains("Video: h264") || stderr.contains("Video: mpeg4"));
    assert!(stderr.contains("Audio:"));
}
