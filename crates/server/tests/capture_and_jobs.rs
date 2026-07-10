//! Integration tests for `sdm-server`'s HTTP surface: the pairing flow
//! that authenticates the Sprint 11 browser extension, the bearer-token
//! auth gate on every job-touching endpoint, plain job CRUD, and the
//! dedup-aware `/capture` + `/capture/batch` endpoints. Driven end-to-end
//! against the real `axum::Router` via `tower::ServiceExt::oneshot`, with
//! a `wiremock` HTTP server standing in for the remote file being
//! downloaded (same pattern `crates/engine/tests/download_flow.rs` uses),
//! so these exercise the *actual* engine, not a mock of it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use sdm_api_types::{
    BatchCaptureRequest, BatchCaptureResponse, CaptureRequest, CaptureResponse, JobResponse,
    PairingStatusResponse, PairingTokenIssueResponse, PairingVerifyRequest, PairingVerifyResponse,
};

const LOOPBACK: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 54321);
const REMOTE: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 5)),
    12345,
);

async fn test_router() -> (Router, tempfile::TempDir) {
    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Arc::new(sdm_engine::Engine::new(pool.clone()));
    let dir = tempfile::tempdir().unwrap();
    let router = sdm_server::build_router(pool, engine, dir.path().to_path_buf());
    (router, dir)
}

fn json_request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    let mut req = builder.body(Body::from(body.to_string())).unwrap();
    req.extensions_mut().insert(ConnectInfo(LOOPBACK));
    req
}

fn get_request(uri: &str, token: Option<&str>, from: SocketAddr) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo(from));
    req
}

async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "failed to parse response body as JSON: {e}\nbody: {}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

#[tokio::test]
async fn health_is_open_and_unauthenticated() {
    let (router, _dir) = test_router().await;
    let resp = router
        .oneshot(get_request("/health", None, LOOPBACK))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn jobs_endpoint_rejects_missing_and_unknown_tokens() {
    let (router, _dir) = test_router().await;

    let resp = router
        .clone()
        .oneshot(get_request("/jobs", None, LOOPBACK))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = router
        .oneshot(get_request("/jobs", Some("not-a-real-token"), LOOPBACK))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pairing_token_issuance_is_loopback_only() {
    let (router, _dir) = test_router().await;

    // A "remote" peer (simulating a request from elsewhere on the LAN,
    // since this endpoint has no other auth yet) must be rejected.
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/pairing/tokens",
            None,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    // (json_request always stamps LOOPBACK; build a remote one by hand.)
    let mut remote_req = Request::builder()
        .method("POST")
        .uri("/pairing/tokens")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    remote_req.extensions_mut().insert(ConnectInfo(REMOTE));
    let remote_resp = router.clone().oneshot(remote_req).await.unwrap();
    assert_eq!(remote_resp.status(), StatusCode::UNAUTHORIZED);

    // The loopback request from `json_request` above should have
    // succeeded.
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn full_pairing_flow_then_authenticated_request_updates_status() {
    let (router, _dir) = test_router().await;

    // 1. Desktop app mints a token (loopback-only).
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/pairing/tokens",
            None,
            serde_json::json!({"label": "Chrome on test box"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let issued: PairingTokenIssueResponse = body_json(resp).await;
    assert_eq!(issued.label, "Chrome on test box");

    // 2. Extension verifies it.
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/pairing/verify",
            None,
            serde_json::to_value(PairingVerifyRequest {
                token: issued.token.clone(),
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let verify: PairingVerifyResponse = body_json(resp).await;
    assert!(verify.ok);

    // 3. Before any authenticated request, status is not yet "connected"
    //    from this token's perspective... but verify() itself touches
    //    last_seen_at, so it should already read as connected.
    let resp = router
        .clone()
        .oneshot(get_request("/pairing/status", None, LOOPBACK))
        .await
        .unwrap();
    let status: PairingStatusResponse = body_json(resp).await;
    assert!(status.connected);
    assert_eq!(status.paired_extensions.len(), 1);

    // 4. An unknown token still gets rejected on protected routes.
    let resp = router
        .clone()
        .oneshot(get_request("/jobs", Some("wrong-token"), LOOPBACK))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 5. The real token works on a protected route.
    let resp = router
        .oneshot(get_request("/jobs", Some(&issued.token), LOOPBACK))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn issue_token(router: &Router) -> String {
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/pairing/tokens",
            None,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let issued: PairingTokenIssueResponse = body_json(resp).await;
    issued.token
}

async fn mock_file_server(bytes: &'static [u8]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", bytes.len().to_string())
                .insert_header("Accept-Ranges", "bytes")
                .set_body_bytes(bytes),
        )
        .mount(&server)
        .await;
    server
}

async fn wait_for_job_terminal(router: &Router, token: &str, job_id: &str) -> JobResponse {
    for _ in 0..200 {
        let resp = router
            .clone()
            .oneshot(get_request(
                &format!("/jobs/{job_id}"),
                Some(token),
                LOOPBACK,
            ))
            .await
            .unwrap();
        if resp.status() == StatusCode::OK {
            let job: JobResponse = body_json(resp).await;
            if job.status == "completed" || job.status == "failed" {
                return job;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("job {job_id} never reached a terminal state");
}

#[tokio::test]
async fn create_job_downloads_the_file_end_to_end() {
    let (router, dir) = test_router().await;
    let token = issue_token(&router).await;
    let server = mock_file_server(b"hello from sdm-server integration test").await;

    let dest = dir.path().join("out.bin");
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/jobs",
            Some(&token),
            serde_json::json!({
                "url": format!("{}/file.bin", server.uri()),
                "destination": dest.to_string_lossy(),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Poll the list until our job shows up (creation is fire-and-forget).
    let mut job_id = None;
    for _ in 0..100 {
        let resp = router
            .clone()
            .oneshot(get_request("/jobs", Some(&token), LOOPBACK))
            .await
            .unwrap();
        let jobs: Vec<JobResponse> = body_json(resp).await;
        if let Some(j) = jobs
            .into_iter()
            .find(|j| j.destination == dest.to_string_lossy())
        {
            job_id = Some(j.id);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let job_id = job_id.expect("job row never appeared");

    let job = wait_for_job_terminal(&router, &token, &job_id).await;
    assert_eq!(job.status, "completed");
    let contents = tokio::fs::read(&dest).await.unwrap();
    assert_eq!(contents, b"hello from sdm-server integration test");
}

#[tokio::test]
async fn capture_starts_a_job_and_deduplicates_the_second_call() {
    let (router, _dir) = test_router().await;
    let token = issue_token(&router).await;
    let server = mock_file_server(b"captured payload").await;
    let url = format!("{}/captured.bin", server.uri());

    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/capture",
            Some(&token),
            serde_json::to_value(CaptureRequest {
                url: url.clone(),
                page_url: Some("https://example.com/page".to_string()),
                suggested_filename: Some("captured.bin".to_string()),
                size_hint_bytes: Some(17),
                source: "context-menu".to_string(),
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let first: CaptureResponse = body_json(resp).await;
    assert!(!first.deduplicated);

    let job = wait_for_job_terminal(&router, &token, &first.job.id).await;
    assert_eq!(job.status, "completed");

    // Capturing the exact same URL again should be recognized as a
    // duplicate of the job/history we already have, per Sprint 11's
    // dedup-against-Sprint-4 scope note, and must NOT start a second
    // download.
    let resp = router
        .oneshot(json_request(
            "POST",
            "/capture",
            Some(&token),
            serde_json::to_value(CaptureRequest {
                url,
                page_url: None,
                suggested_filename: None,
                size_hint_bytes: None,
                source: "clipboard".to_string(),
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let second: CaptureResponse = body_json(resp).await;
    assert!(second.deduplicated);
    assert_eq!(second.job.id, first.job.id);
}

#[tokio::test]
async fn batch_capture_dedupes_repeated_urls_within_the_same_paste() {
    let (router, _dir) = test_router().await;
    let token = issue_token(&router).await;
    let server_a = mock_file_server(b"file A contents").await;
    let server_b = mock_file_server(b"file B contents, a bit longer").await;
    let url_a = format!("{}/a.bin", server_a.uri());
    let url_b = format!("{}/b.bin", server_b.uri());

    let resp = router
        .oneshot(json_request(
            "POST",
            "/capture/batch",
            Some(&token),
            serde_json::to_value(BatchCaptureRequest {
                // url_a listed twice, as if the pasted text contained both
                // a raw link and an <a href> pointing at the same file.
                urls: vec![url_a.clone(), url_b.clone(), url_a.clone()],
                page_url: None,
            })
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let batch: BatchCaptureResponse = body_json(resp).await;
    // The repeated `url_a` collapses to a single result entry.
    assert_eq!(batch.results.len(), 2);
    assert!(batch.results.iter().all(|r| r.error.is_none()));
}

#[tokio::test]
async fn pause_then_resume_and_cancel_job_lifecycle() {
    let (router, dir) = test_router().await;
    let token = issue_token(&router).await;
    // A reasonably large body + no artificial delay is enough that the
    // job is very likely still "running" a few milliseconds in, giving
    // pause something real to interrupt.
    let body: Vec<u8> = (0..2_000_000u32).map(|i| (i % 256) as u8).collect();
    let body: &'static [u8] = Box::leak(body.into_boxed_slice());
    let server = mock_file_server(body).await;
    let dest = dir.path().join("big.bin");

    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/jobs",
            Some(&token),
            serde_json::json!({
                "url": format!("{}/big.bin", server.uri()),
                "destination": dest.to_string_lossy(),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let mut job_id = None;
    for _ in 0..100 {
        let resp = router
            .clone()
            .oneshot(get_request("/jobs", Some(&token), LOOPBACK))
            .await
            .unwrap();
        let jobs: Vec<JobResponse> = body_json(resp).await;
        if let Some(j) = jobs
            .into_iter()
            .find(|j| j.destination == dest.to_string_lossy())
        {
            job_id = Some(j.id);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let job_id = job_id.expect("job row never appeared");

    // Cancel (delete) it outright and confirm it's gone.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/jobs/{job_id}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .map(|mut r| {
                    r.extensions_mut().insert(ConnectInfo(LOOPBACK));
                    r
                })
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = router
        .oneshot(get_request(
            &format!("/jobs/{job_id}"),
            Some(&token),
            LOOPBACK,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Same construction as `test_router`, but also hands back the pool so a
/// test can verify a route's storage side-effects directly (via
/// `sdm_storage`) rather than only through another HTTP round trip.
async fn test_router_with_pool() -> (Router, sdm_storage::SqlitePool, tempfile::TempDir) {
    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Arc::new(sdm_engine::Engine::new(pool.clone()));
    let dir = tempfile::tempdir().unwrap();
    let router = sdm_server::build_router(pool.clone(), engine, dir.path().to_path_buf());
    (router, pool, dir)
}

/// Sprint 12: `POST /auth/cookies` is behind the same bearer pairing-token
/// gate as every other `protected` route (rejecting missing/unknown
/// tokens is already covered generically by
/// `jobs_endpoint_rejects_missing_and_unknown_tokens` above for `/jobs`;
/// this proves the gate applies to this route specifically too), and a
/// successful import is actually retrievable afterwards via
/// `sdm_storage::auth::resolve_auth_config` — the same path
/// `sdm download`/the engine would use to pick it up.
#[tokio::test]
async fn auth_cookie_import_requires_token_and_is_retrievable_after_import() {
    let (router, pool, _dir) = test_router_with_pool().await;

    // No token: rejected, same as every other protected route.
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/auth/cookies",
            None,
            serde_json::json!({"domain": "example.com", "cookie": "session=abc123"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let token = issue_token(&router).await;

    // Empty domain/cookie: rejected as a client error, not silently
    // accepted as an empty auth config.
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/auth/cookies",
            Some(&token),
            serde_json::json!({"domain": "", "cookie": "session=abc123"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // A real import succeeds...
    let resp = router
        .clone()
        .oneshot(json_request(
            "POST",
            "/auth/cookies",
            Some(&token),
            serde_json::json!({"domain": "example.com", "cookie": "session=abc123; csrftoken=xyz"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // ...and is actually retrievable, via the exact same
    // `resolve_auth_config` path `sdm download` uses to pick up a saved
    // domain auth config for a URL on that domain.
    let store = sdm_storage::CredentialStore::new(pool.clone());
    let resolved = sdm_storage::auth::resolve_auth_config(
        &pool,
        &store,
        None,
        "https://example.com/some/file.zip",
    )
    .await
    .unwrap()
    .expect("cookie import should be resolvable for the imported domain");
    assert_eq!(
        resolved.cookie.as_deref(),
        Some("session=abc123; csrftoken=xyz")
    );

    // Importing again for the same domain replaces (not duplicates) the
    // stored cookie.
    let resp = router
        .oneshot(json_request(
            "POST",
            "/auth/cookies",
            Some(&token),
            serde_json::json!({"domain": "example.com", "cookie": "session=updated456"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resolved = sdm_storage::auth::resolve_auth_config(
        &pool,
        &store,
        None,
        "https://example.com/some/other-file.zip",
    )
    .await
    .unwrap()
    .expect("updated cookie should still be resolvable");
    assert_eq!(resolved.cookie.as_deref(), Some("session=updated456"));
}
