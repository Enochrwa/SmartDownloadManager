//! Sprint 4 end-to-end tests: checksum verification, per-chunk corruption
//! detection + targeted repair, mirror failover, and duplicate detection.
//! Same `wiremock`-driven approach as `download_flow.rs`.

use std::time::Duration;

use sdm_engine::{ConnectionsOption, DownloadRequest, DuplicatePolicy, Engine, ExpectedChecksum};
use sha2::{Digest, Sha256};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn body(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
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

struct RangeResponder {
    data: Vec<u8>,
    etag: String,
}

impl Respond for RangeResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let total = self.data.len() as u64;
        if let Some((start, end)) = parse_range(req) {
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
        }
    }
}

fn responder_clone(r: &RangeResponder) -> RangeResponder {
    RangeResponder {
        data: r.data.clone(),
        etag: r.etag.clone(),
    }
}

async fn mount_range_server(data: Vec<u8>, etag: &str) -> MockServer {
    let server = MockServer::start().await;
    let responder = RangeResponder {
        data,
        etag: etag.to_string(),
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

fn assert_bytes_eq(actual: &[u8], expected: &[u8], msg: &str) {
    if actual.len() != expected.len() {
        panic!(
            "{msg}: length mismatch, actual={} expected={}",
            actual.len(),
            expected.len()
        );
    }
    if let Some(i) = actual.iter().zip(expected.iter()).position(|(a, b)| a != b) {
        panic!("{msg}: first mismatch at byte {i}");
    }
}

#[tokio::test]
async fn checksum_match_completes_and_persists_actual_checksum() {
    let data = body(20_000);
    let expected_hex = sha256_hex(&data);
    let server = mount_range_server(data, "etag-1").await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let (tx, _rx) = sdm_engine::channel();

    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        mirrors: vec![],
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(2),
        expected_checksum: Some(
            ExpectedChecksum::parse(&format!("sha256:{expected_hex}")).unwrap(),
        ),
        duplicate_policy: DuplicatePolicy::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    assert!(job.checksum_verified);
    assert_eq!(job.checksum_actual.as_deref(), Some(expected_hex.as_str()));
}

#[tokio::test]
async fn checksum_mismatch_fails_the_job() {
    let data = body(20_000);
    let server = mount_range_server(data, "etag-1").await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let (tx, _rx) = sdm_engine::channel();

    let wrong_hash = "0".repeat(64);
    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        mirrors: vec![],
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(2),
        expected_checksum: Some(ExpectedChecksum::parse(&format!("sha256:{wrong_hash}")).unwrap()),
        duplicate_policy: DuplicatePolicy::default(),
    };
    let err = engine.start_download(req, tx).await.unwrap_err();
    assert!(matches!(
        err,
        sdm_engine::EngineError::ChecksumMismatch { .. }
    ));

    let jobs = sdm_storage::list_jobs(&pool).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, sdm_storage::JobStatus::Failed);
    assert!(!jobs[0].checksum_verified);
}

#[tokio::test]
async fn corrupt_chunk_is_detected_and_only_that_chunk_is_refetched() {
    // 600_000 bytes / 256 KiB chunks -> chunk 1 is exactly [262144, 524287].
    let data = body(600_000);
    let server = mount_range_server(data.clone(), "etag-1").await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let (tx, _rx) = sdm_engine::channel();

    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        mirrors: vec![],
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(3),
        expected_checksum: None,
        duplicate_policy: DuplicatePolicy::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();
    assert_eq!(job.status, sdm_engine::JobStatus::Completed);

    // Corrupt two bytes squarely inside chunk 1's byte range.
    let mut corrupted = data.clone();
    corrupted[300_000] ^= 0xFF;
    corrupted[300_001] ^= 0xFF;
    tokio::fs::write(&dest, &corrupted).await.unwrap();

    let before = server.received_requests().await.unwrap().len();
    let repaired = engine.verify_and_repair(&job.id).await.unwrap();
    assert_eq!(repaired, 1, "exactly one chunk should have been corrupt");

    let after = server.received_requests().await.unwrap();
    let new_requests = &after[before..];
    assert_eq!(
        new_requests.len(),
        1,
        "repair should issue exactly one HTTP request, not a whole-file re-download"
    );
    let range = parse_range(&new_requests[0]).expect("repair request must be a Range GET");
    assert_eq!(
        range,
        (262_144, Some(524_287)),
        "repair must be scoped to exactly the corrupted chunk's byte range"
    );

    let repaired_bytes = tokio::fs::read(&dest).await.unwrap();
    assert_bytes_eq(&repaired_bytes, &data, "file after repair");
}

#[tokio::test]
async fn corrupt_chunk_repair_is_a_no_op_when_nothing_is_corrupt() {
    let data = body(600_000);
    let server = mount_range_server(data.clone(), "etag-1").await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let (tx, _rx) = sdm_engine::channel();

    let req = DownloadRequest {
        url: format!("{}/file.bin", server.uri()),
        mirrors: vec![],
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(3),
        expected_checksum: None,
        duplicate_policy: DuplicatePolicy::default(),
    };
    let job = engine.start_download(req, tx).await.unwrap();

    let repaired = engine.verify_and_repair(&job.id).await.unwrap();
    assert_eq!(repaired, 0);
}

#[tokio::test]
async fn mirror_failover_switches_to_a_working_mirror_on_failure() {
    let data = body(30_000);
    let etag = "etag-1";

    // Mirror A: HEAD succeeds (so probing works) but every GET/Range GET
    // fails with a retryable 503.
    let server_a = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", data.len().to_string())
                .insert_header("Accept-Ranges", "bytes")
                .insert_header("ETag", etag),
        )
        .mount(&server_a)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
        .mount(&server_a)
        .await;

    // Mirror B: works normally, but its HEAD response is deliberately
    // delayed so the latency-based mirror ranking deterministically puts
    // A first (attempt 1) regardless of CI scheduling jitter. GET/Range
    // GET responses are still fast and correct.
    let server_b = MockServer::start().await;
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", data.len().to_string())
                .insert_header("Accept-Ranges", "bytes")
                .insert_header("ETag", etag)
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server_b)
        .await;
    Mock::given(method("GET"))
        .respond_with(RangeResponder {
            data: data.clone(),
            etag: etag.to_string(),
        })
        .mount(&server_b)
        .await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let (tx, _rx) = sdm_engine::channel();

    // Fixed(1) connection -> exactly one segment covering the whole file,
    // so the failover is unambiguous to assert on.
    let req = DownloadRequest {
        url: format!("{}/file.bin", server_a.uri()),
        mirrors: vec![format!("{}/file.bin", server_b.uri())],
        destination: dest.clone(),
        connections: ConnectionsOption::Fixed(1),
        expected_checksum: None,
        duplicate_policy: DuplicatePolicy::default(),
    };

    let job = tokio::time::timeout(Duration::from_secs(10), engine.start_download(req, tx))
        .await
        .expect("download should not hang")
        .expect("download should succeed via the second mirror");

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let downloaded = tokio::fs::read(&dest).await.unwrap();
    assert_bytes_eq(&downloaded, &data, "downloaded file");

    let a_gets = server_a
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "GET")
        .count();
    let b_gets = server_b
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == "GET")
        .count();
    assert_eq!(
        a_gets, 1,
        "mirror A should be tried exactly once, then abandoned"
    );
    assert_eq!(b_gets, 1, "mirror B should serve the successful retry");
}

#[tokio::test]
async fn duplicate_skip_policy_prevents_a_second_download() {
    let data = body(5_000);
    let server = mount_range_server(data, "etag-1").await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let url = format!("{}/file.bin", server.uri());

    let (tx1, _rx1) = sdm_engine::channel();
    let first = engine
        .start_download(
            DownloadRequest {
                url: url.clone(),
                mirrors: vec![],
                destination: dest.clone(),
                connections: ConnectionsOption::Fixed(1),
                expected_checksum: None,
                duplicate_policy: DuplicatePolicy::default(),
            },
            tx1,
        )
        .await
        .unwrap();

    let (tx2, _rx2) = sdm_engine::channel();
    let err = engine
        .start_download(
            DownloadRequest {
                url: url.clone(),
                mirrors: vec![],
                destination: dest.clone(),
                connections: ConnectionsOption::Fixed(1),
                expected_checksum: None,
                duplicate_policy: DuplicatePolicy::Skip,
            },
            tx2,
        )
        .await
        .unwrap_err();

    match err {
        sdm_engine::EngineError::DuplicateSkipped { existing_job_id } => {
            assert_eq!(existing_job_id, first.id);
        }
        other => panic!("expected DuplicateSkipped, got {other:?}"),
    }

    // Skip must not have inserted a new job row.
    let jobs = sdm_storage::list_jobs(&pool).await.unwrap();
    assert_eq!(jobs.len(), 1);
}

#[tokio::test]
async fn duplicate_rename_policy_downloads_to_a_new_filename() {
    let data = body(5_000);
    let server = mount_range_server(data, "etag-1").await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("file.bin");
    let url = format!("{}/file.bin", server.uri());

    for _ in 0..2 {
        let (tx, _rx) = sdm_engine::channel();
        engine
            .start_download(
                DownloadRequest {
                    url: url.clone(),
                    mirrors: vec![],
                    destination: dest.clone(),
                    connections: ConnectionsOption::Fixed(1),
                    expected_checksum: None,
                    duplicate_policy: DuplicatePolicy::Rename,
                },
                tx,
            )
            .await
            .unwrap();
    }

    let jobs = sdm_storage::list_jobs(&pool).await.unwrap();
    assert_eq!(jobs.len(), 2);
    let mut destinations: Vec<String> = jobs.into_iter().map(|j| j.destination).collect();
    destinations.sort();
    assert_ne!(destinations[0], destinations[1]);
    assert!(tokio::fs::metadata(&destinations[0]).await.is_ok());
    assert!(tokio::fs::metadata(&destinations[1]).await.is_ok());
}
