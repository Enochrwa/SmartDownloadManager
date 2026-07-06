//! WebDAV integration tests (Sprint 8 DoD): scheme translation actually
//! reaches a real server, PROPFIND listing, PUT upload, and a ranged
//! GET download all round-trip correctly.
//!
//! Same philosophy as ftp_integration.rs/sftp_integration.rs: talks to
//! a real WebDAV server whose base URL comes from SDM_TEST_WEBDAV_URL
//! (e.g. http://127.0.0.1:8078, matching the webdav-integration CI
//! job's server - see .github/workflows/ci.yml). If unset, every test
//! no-ops instead of failing.
//!
//! The CI job runs a Python wsgidav server rather than "nginx with the
//! dav module" - docs/SPRINT_PLAN_PHASE2.md's original suggestion.
//! wsgidav was substituted because stock nginx doesn't ship PROPFIND
//! support without a non-default third-party module (nginx-dav-ext-module)
//! that isn't in Ubuntu's package archive, whereas wsgidav is a
//! pip-installable, fully RFC 4918-compliant WebDAV server - same
//! protocol coverage, less CI fragility.

use sdm_protocols::{build_client, download_range, download_single, probe, webdav};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

fn base_url() -> Option<String> {
    std::env::var("SDM_TEST_WEBDAV_URL").ok()
}

macro_rules! skip_without_server {
    () => {
        if base_url().is_none() {
            eprintln!(
                "SDM_TEST_WEBDAV_URL not set - skipping WebDAV integration test. \
                 See .github/workflows/ci.yml's webdav-integration job to run this for real."
            );
            return;
        }
    };
}

#[tokio::test]
async fn webdav_scheme_reaches_a_real_server() {
    skip_without_server!();
    let base = base_url().unwrap();
    let webdav_url = base.replacen("http://", "webdav://", 1) + "/hello.txt";
    let http_url = webdav::to_http_url(&webdav_url).unwrap();

    let client = build_client();
    let resp = client.get(&http_url).send().await.unwrap();
    assert!(resp.status().is_success());
    let body = resp.text().await.unwrap();
    assert_eq!(body.trim_end(), "hello webdav world");
}

#[tokio::test]
async fn webdav_lists_a_directory_via_propfind() {
    skip_without_server!();
    let base = base_url().unwrap();
    let client = build_client();
    let entries = webdav::list_dir(&client, &base).await.unwrap();

    assert!(
        entries
            .iter()
            .any(|e| e.href.trim_end_matches('/').ends_with("hello.txt")),
        "expected hello.txt among {entries:?}"
    );
}

#[tokio::test]
async fn webdav_uploads_then_downloads_a_file() {
    skip_without_server!();
    let base = base_url().unwrap();
    let client = build_client();
    let content = b"round-tripped through sdm's WebDAV PUT/GET".to_vec();

    let put_url = format!("{base}/sdm_roundtrip.txt");
    webdav::upload(&client, &put_url, content.clone())
        .await
        .unwrap();

    let resp = client.get(&put_url).send().await.unwrap();
    assert!(resp.status().is_success());
    assert_eq!(resp.bytes().await.unwrap().as_ref(), content.as_slice());
}

/// Sprint 8 DoD: "WebDAV reuses the Sprint 1-2 range-request and
/// segment-splitting logic almost unchanged" - verify a segmented,
/// multi-range download against a real WebDAV server actually works,
/// using the same crate::http::{probe, download_range} HTTP downloads
/// use (WebDAV's GET+Range is byte-for-byte HTTP).
#[tokio::test]
async fn webdav_supports_segmented_range_downloads() {
    skip_without_server!();
    let base = base_url().unwrap();
    let client = build_client();
    let content: Vec<u8> = (0..200_000).map(|i| (i % 256) as u8).collect();

    let put_url = format!("{base}/sdm_segmented.bin");
    webdav::upload(&client, &put_url, content.clone())
        .await
        .unwrap();

    let info = probe(&client, &put_url).await.unwrap();
    assert_eq!(info.total_bytes, Some(content.len() as u64));
    assert!(
        info.supports_range,
        "wsgidav should advertise Range support"
    );

    let dest = tempfile::NamedTempFile::new().unwrap();
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dest.path())
        .await
        .unwrap();
    file.set_len(content.len() as u64).await.unwrap();
    let file = Arc::new(tokio::sync::Mutex::new(file));

    let mid = content.len() as u64 / 2;
    let end1 = Arc::new(AtomicU64::new(mid - 1));
    let end2 = Arc::new(AtomicU64::new(content.len() as u64 - 1));
    let (r1, r2) = tokio::join!(
        download_range(
            &client,
            &put_url,
            0,
            end1,
            file.clone(),
            Arc::new(AtomicU64::new(0)),
            None
        ),
        download_range(
            &client,
            &put_url,
            mid,
            end2,
            file.clone(),
            Arc::new(AtomicU64::new(mid)),
            None
        ),
    );
    r1.unwrap();
    r2.unwrap();

    assert_eq!(tokio::fs::read(dest.path()).await.unwrap(), content);

    let dest_single = tempfile::NamedTempFile::new().unwrap();
    download_single(&client, &put_url, dest_single.path(), None)
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(dest_single.path()).await.unwrap(), content);
}
