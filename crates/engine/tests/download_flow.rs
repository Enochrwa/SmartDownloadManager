//! End-to-end tests for `sdm_engine::Engine`, driven against a local
//! `wiremock` HTTP server so nothing here touches the real network.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sdm_engine::{ConnectionsOption, DownloadRequest, Engine, ProgressEvent};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn body(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn parse_range(req: &Request) -> Option<(u64, Option<u64>)> {
    let raw = req.headers.get("Range")?.to_str().ok()?;
    let spec = raw.strip_prefix("bytes=")?;
    let mut parts = spec.splitn(2, '-');
    let start: u64 = parts.next()?.parse().ok()?;
    let end = parts
        .next()
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok());
    Some((start, end))
}

/// Serves `data` either whole (plain GET/HEAD) or sliced (ranged GET),
/// always advertising range support + an ETag so resume logic has
/// something to validate against. Optionally delays responses whose start
/// byte falls below `slow_below`, to exercise segment stealing.
struct RangeResponder {
    data: Vec<u8>,
    etag: String,
    slow_below: Option<u64>,
}

impl Respond for RangeResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let total = self.data.len() as u64;
        let base = if let Some((start, end)) = parse_range(req) {
            let end = end.unwrap_or(total - 1).min(total - 1);
            let slice = self.data[start as usize..=end as usize].to_vec();
            ResponseTemplate::new(206)
                .insert_header("Content-Range", format!("bytes {start}-{end}/{total}"))
                .insert_header("Accept-Ranges", "bytes")
                .insert_header("ETag", self.etag.as_str())
                .set_body_bytes(slice)
        } else {
            ResponseTemplate::new(200)
                .insert_header("Content-Length", total.to_string())
                .insert_header("Accept-Ranges", "bytes")
                .insert_header("ETag", self.etag.as_str())
                .set_body_bytes(self.data.clone())
        };

        if let (Some(threshold), Some((start, _))) = (self.slow_below, parse_range(req)) {
            if start < threshold {
                return base.set_delay(Duration::from_millis(250));
            }
        }
        base
    }
}

/// Wraps another responder but fails the *first* request it ever sees with
/// a transient 503, then behaves normally for every subsequent request —
/// simulating one connection dying and needing a retry.
struct FlakyOnce<R> {
    inner: R,
    tripped: AtomicBool,
}

impl<R: Respond> Respond for FlakyOnce<R> {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        if self
            .tripped
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return ResponseTemplate::new(503).insert_header("Retry-After", "0");
        }
        self.inner.respond(req)
    }
}

async fn mount_range_server(data: Vec<u8>, etag: &str, slow_below: Option<u64>) -> MockServer {
    let server = MockServer::start().await;
    let responder = RangeResponder {
        data,
        etag: etag.to_string(),
        slow_below,
    };
    Mock::given(method("HEAD"))
        .respond_with(responder_clone(&responder))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(responder)
        .mount(&server)
        .await;
    server
}

// wiremock's `respond_with` takes ownership, so give HEAD its own instance.
fn responder_clone(r: &RangeResponder) -> RangeResponder {
    RangeResponder {
        data: r.data.clone(),
        etag: r.etag.clone(),
        slow_below: r.slow_below,
    }
}

fn assert_bytes_eq(actual: &[u8], expected: &[u8], msg: &str) {
    if actual.len() != expected.len() {
        panic!(
            "{msg}: length mismatch, actual={} expected={}",
            actual.len(),
            expected.len()
        );
    }
    if let Some(i) = actual.iter().zip(expected.iter()).position(|(a, b)| a != b) {
        let start = i.saturating_sub(8);
        let end = (i + 8).min(actual.len());
        panic!(
            "{msg}: first mismatch at byte {i}: actual={:?} expected={:?}",
            &actual[start..end],
            &expected[start..end]
        );
    }
}

async fn drain(mut rx: sdm_engine::ProgressReceiver) -> Vec<ProgressEvent> {
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

#[tokio::test]
async fn sprint1_single_stream_download_completes_and_persists() {
    let server = MockServer::start().await;
    let data = body(50_000);
    // No Accept-Ranges anywhere -> forces the Sprint 1 single-stream path.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", data.len().to_string())
                .set_body_bytes(data.clone()),
        )
        .mount(&server)
        .await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");

    let (tx, rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        destination: dest.clone(),
        connections: ConnectionsOption::Auto,
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_bytes_eq(&on_disk, &data, "single-stream download");

    let events = drain(rx).await;
    assert!(events
        .iter()
        .any(|e| matches!(e, ProgressEvent::Completed { .. })));
}

#[tokio::test]
async fn sprint2_segmented_download_uses_multiple_range_requests_and_is_byte_correct() {
    let data = body(2_000_000);
    let server = mount_range_server(data.clone(), "\"v1\"", None).await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");

    let (tx, _rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(4),
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_eq!(on_disk.len(), data.len());
    assert_bytes_eq(&on_disk, &data, "segmented download");

    let requests = server.received_requests().await.unwrap();
    let ranged_gets = requests
        .iter()
        .filter(|r| r.method.as_str() == "GET" && r.headers.get("Range").is_some())
        .count();
    assert!(
        ranged_gets >= 4,
        "expected at least 4 ranged GETs, saw {ranged_gets}"
    );

    let segments = sdm_storage::get_segments(&pool, &job.id).await.unwrap();
    assert!(
        segments.len() >= 4,
        "expected at least 4 segments, got {}",
        segments.len()
    );
    assert!(segments
        .iter()
        .all(|s| s.status == sdm_storage::SegmentStatus::Completed));
}

#[tokio::test]
async fn sprint2_segment_stealing_kicks_in_for_a_slow_segment() {
    let data = body(4_000_000);
    let slow_below = data.len() as u64 / 4; // the whole first segment is slow
    let server = mount_range_server(data.clone(), "\"v1\"", Some(slow_below)).await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");

    let (tx, _rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(4),
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_bytes_eq(
        &on_disk,
        &data,
        "file must be byte-correct even after stealing",
    );

    let segments = sdm_storage::get_segments(&pool, &job.id).await.unwrap();
    assert!(
        segments.len() > 4,
        "expected extra segments from stealing, got {}",
        segments.len()
    );
}

#[tokio::test]
async fn sprint3_retries_a_transient_failure_without_corrupting_the_file() {
    let data = body(1_500_000);
    let server = MockServer::start().await;
    let etag = "\"v1\"".to_string();
    let head_responder = RangeResponder {
        data: data.clone(),
        etag: etag.clone(),
        slow_below: None,
    };
    Mock::given(method("HEAD"))
        .respond_with(head_responder)
        .mount(&server)
        .await;

    let flaky = FlakyOnce {
        inner: RangeResponder {
            data: data.clone(),
            etag,
            slow_below: None,
        },
        tripped: AtomicBool::new(false),
    };
    Mock::given(method("GET"))
        .respond_with(flaky)
        .mount(&server)
        .await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");

    let (tx, rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(3),
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_bytes_eq(
        &on_disk,
        &data,
        "one flaky connection must not corrupt the rest of the file",
    );

    let events = drain(rx).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProgressEvent::Retrying { .. })),
        "expected a Retrying progress event"
    );
}

#[tokio::test]
async fn sprint3_resume_after_restart_skips_completed_segments_and_finishes_correctly() {
    let data = body(1_000_000);
    let server = mount_range_server(data.clone(), "\"stable-etag\"", None).await;

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("sdm.db").to_string_lossy().to_string();
    let dl_dir = tempfile::tempdir().unwrap();
    let dest = dl_dir.path().join("file.bin");

    // --- "first run": start the download, but only let it finish planning
    // and completing segment 0 by hand, then simulate a crash by just
    // dropping everything without finishing the others. ---
    let job_id = uuid_like_id();
    {
        let pool = sdm_storage::connect(&db_path).await.unwrap();
        sdm_storage::insert_job(
            &pool,
            &job_id,
            &format!("{}/file.bin", server.uri()),
            dest.to_str().unwrap(),
        )
        .await
        .unwrap();
        sdm_storage::update_job_probe(
            &pool,
            &job_id,
            Some(data.len() as i64),
            true,
            Some("\"stable-etag\""),
            None,
            4,
        )
        .await
        .unwrap();

        let plan = sdm_engine::segment::plan_segments(data.len() as u64, 4);
        let plan_i64: Vec<(i64, i64, i64)> = plan
            .iter()
            .enumerate()
            .map(|(i, (s, e))| (i as i64, *s as i64, *e as i64))
            .collect();
        let records = sdm_storage::replace_segments(&pool, &job_id, &plan_i64)
            .await
            .unwrap();

        // Pre-write segment 0's bytes to disk and mark it Completed; leave
        // the rest Pending, exactly as if the process died mid-flight.
        let file = tokio::fs::File::create(&dest).await.unwrap();
        file.set_len(data.len() as u64).await.unwrap();
        drop(file);
        let seg0 = &records[0];
        let bytes = &data[seg0.start_byte as usize..=seg0.end_byte as usize];
        tokio::fs::write(&dest, {
            // write just segment 0's slice at its offset, leaving the rest
            // of the (already zero-filled, correctly-sized) file alone.
            let mut buf = vec![0u8; data.len()];
            buf[seg0.start_byte as usize..=seg0.end_byte as usize].copy_from_slice(bytes);
            buf
        })
        .await
        .unwrap();

        sdm_storage::update_segment(
            &pool,
            &seg0.id,
            seg0.end_byte - seg0.start_byte + 1,
            sdm_storage::SegmentStatus::Completed,
            0,
            None,
        )
        .await
        .unwrap();
        sdm_storage::set_job_status(&pool, &job_id, sdm_storage::JobStatus::Paused)
            .await
            .unwrap();
        pool.close().await;
    }

    // --- "app restart, weeks later": open a brand-new pool + engine
    // pointed at the same DB file and destination, and resume. ---
    {
        let pool = sdm_storage::connect(&db_path).await.unwrap();
        let engine = Engine::new(pool.clone());
        let (tx, _rx) = sdm_engine::channel();
        let job = engine.resume_download(&job_id, tx).await.unwrap();

        assert_eq!(job.status, sdm_engine::JobStatus::Completed);
        let on_disk = tokio::fs::read(&dest).await.unwrap();
        assert_bytes_eq(
            &on_disk,
            &data,
            "resumed download must be byte-identical to the source",
        );
    }
}

#[tokio::test]
async fn sprint3_resume_restarts_from_scratch_when_etag_changed() {
    let data_v2 = body(300_000);
    let server = mount_range_server(data_v2.clone(), "\"v2-different\"", None).await;

    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("sdm.db").to_string_lossy().to_string();
    let dl_dir = tempfile::tempdir().unwrap();
    let dest = dl_dir.path().join("file.bin");

    let job_id = uuid_like_id();
    {
        let pool = sdm_storage::connect(&db_path).await.unwrap();
        sdm_storage::insert_job(
            &pool,
            &job_id,
            &format!("{}/file.bin", server.uri()),
            dest.to_str().unwrap(),
        )
        .await
        .unwrap();
        // Pretend we'd previously probed a DIFFERENT (now-stale) version of the resource.
        sdm_storage::update_job_probe(
            &pool,
            &job_id,
            Some(999_999),
            true,
            Some("\"v1-stale\""),
            None,
            2,
        )
        .await
        .unwrap();
        let plan_i64 = vec![(0i64, 0i64, 499_999i64), (1i64, 500_000i64, 999_998i64)];
        let records = sdm_storage::replace_segments(&pool, &job_id, &plan_i64)
            .await
            .unwrap();
        sdm_storage::update_segment(
            &pool,
            &records[0].id,
            500_000,
            sdm_storage::SegmentStatus::Completed,
            0,
            None,
        )
        .await
        .unwrap();
        sdm_storage::set_job_status(&pool, &job_id, sdm_storage::JobStatus::Paused)
            .await
            .unwrap();
        pool.close().await;
    }

    let pool = sdm_storage::connect(&db_path).await.unwrap();
    let engine = Engine::new(pool.clone());
    let (tx, _rx) = sdm_engine::channel();
    let job = engine.resume_download(&job_id, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_bytes_eq(
        &on_disk,
        &data_v2,
        "must restart and fetch the NEW content when the ETag changed",
    );
}

#[tokio::test]
async fn sprint3_new_download_renames_instead_of_clobbering_existing_file() {
    let data = body(10_000);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", data.len().to_string())
                .set_body_bytes(data.clone()),
        )
        .mount(&server)
        .await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool);
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("movie.bin");

    let original_contents = b"pre-existing unrelated file, do not touch".to_vec();
    tokio::fs::write(&dest, &original_contents).await.unwrap();

    let (tx, _rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        destination: dest.clone(),
        connections: ConnectionsOption::Auto,
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    // Original file must be untouched.
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), original_contents);
    // New content lives at the renamed path.
    let renamed = tmp.path().join("movie (1).bin");
    assert_eq!(tokio::fs::read(&renamed).await.unwrap(), data);
    assert_eq!(job.destination, renamed.to_string_lossy());
}

fn uuid_like_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
