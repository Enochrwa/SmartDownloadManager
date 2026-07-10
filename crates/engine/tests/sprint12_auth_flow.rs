//! Sprint 12 DoD: "a cookie-authenticated download against a test site
//! requiring a login session succeeds where an unauthenticated request
//! would 401." Exercises the real `Engine::new_with_config` ->
//! `Engine::start_download` path against a `wiremock` server that gates
//! its file behind a required `Cookie`/`Authorization` header — the same
//! mechanism `sdm download --cookie`/`--bearer` uses for a real download,
//! not just a lower-level client-construction check (see
//! `crates/protocols/src/net_config.rs`'s own wiremock test for that
//! layer).

use sdm_engine::{ConnectionsOption, DownloadRequest, Engine};
use sdm_protocols::{AuthHeader, ClientConfig};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn body(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 256) as u8).collect()
}

#[tokio::test]
async fn cookie_authenticated_download_succeeds_where_unauthenticated_would_401() {
    let server = MockServer::start().await;
    let data = body(20_000);

    Mock::given(method("GET"))
        .and(header("Cookie", "sessionid=logged-in-user"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", data.len().to_string())
                .set_body_bytes(data.clone()),
        )
        .mount(&server)
        .await;
    // Anything without the right cookie (including no Cookie header at
    // all) gets a 401 — the same as a real login-gated file host.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let url = format!("{}/private/file.bin", server.uri());

    // Unauthenticated: fails, exactly as a real 401 would.
    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("unauthenticated.bin");
    let (tx, rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: url.clone(),
        destination: dest.clone(),
        connections: ConnectionsOption::Auto,
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let result = engine.start_download(req, tx).await;
    drop(rx);
    assert!(
        result.is_err(),
        "download without the session cookie must fail (401), not silently succeed"
    );

    // Authenticated via ClientConfig's cookie support: succeeds.
    let cfg = ClientConfig::new()
        .with_cookie("sessionid=logged-in-user", &server.uri())
        .unwrap();
    let engine = Engine::new_with_config(pool.clone(), &cfg).unwrap();
    let dest = tmp.path().join("authenticated.bin");
    let (tx, rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url: url.clone(),
        destination: dest.clone(),
        connections: ConnectionsOption::Auto,
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine
        .start_download(req, tx)
        .await
        .expect("cookie-authenticated download should succeed");
    drop(rx);
    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_eq!(on_disk, data);
}

#[tokio::test]
async fn bearer_token_authenticated_download_succeeds_where_unauthenticated_would_401() {
    let server = MockServer::start().await;
    let data = body(15_000);

    Mock::given(method("GET"))
        .and(header("Authorization", "Bearer sprint12-secret-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", data.len().to_string())
                .set_body_bytes(data.clone()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let url = format!("{}/api/private-file", server.uri());
    let pool = sdm_storage::connect_in_memory().await.unwrap();

    let cfg = ClientConfig::new().with_header(AuthHeader::bearer("sprint12-secret-token"));
    let engine = Engine::new_with_config(pool.clone(), &cfg).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("bearer-authenticated.bin");
    let (tx, rx) = sdm_engine::channel();
    let req = DownloadRequest {
        url,
        destination: dest.clone(),
        connections: ConnectionsOption::Auto,
        mirrors: vec![],
        expected_checksum: None,
        duplicate_policy: Default::default(),
    };
    let job = engine
        .start_download(req, tx)
        .await
        .expect("bearer-authenticated download should succeed");
    drop(rx);
    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let on_disk = tokio::fs::read(&dest).await.unwrap();
    assert_eq!(on_disk, data);
}
