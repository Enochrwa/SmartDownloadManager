//! FTP integration tests (Sprint 7 DoD): download, upload, and
//! resume-after-kill against a real FTP server.
//!
//! Unlike the HTTP tests elsewhere in this workspace (which mock the
//! server in-process with `wiremock`), there's no equivalent lightweight
//! in-process FTP mock in the Rust ecosystem worth adding as a dependency
//! for this. Instead, these tests talk to a real FTP server, whose address
//! is passed in via `SDM_TEST_FTP_ADDR` (e.g. `127.0.0.1:2121`), with
//! credentials via `SDM_TEST_FTP_USER`/`SDM_TEST_FTP_PASS` (default
//! `test`/`test`, matching the `pyftpdlib` server the `ftp-integration` CI
//! job starts — see `.github/workflows/ci.yml`).
//!
//! If `SDM_TEST_FTP_ADDR` isn't set (the common case: a plain `cargo test`
//! on a dev machine with no FTP server running), every test here no-ops
//! with an explanatory message instead of failing, so `cargo test
//! --workspace` stays green without extra local setup.

use sdm_protocols::ftp::{FtpSession, FtpUrl};

fn test_server_addr() -> Option<String> {
    std::env::var("SDM_TEST_FTP_ADDR").ok()
}

fn test_url(path: &str) -> FtpUrl {
    let addr = test_server_addr().expect("checked by caller");
    let user = std::env::var("SDM_TEST_FTP_USER").unwrap_or_else(|_| "test".to_string());
    let pass = std::env::var("SDM_TEST_FTP_PASS").unwrap_or_else(|_| "test".to_string());
    FtpUrl::parse(&format!("ftp://{user}:{pass}@{addr}/{path}"))
        .expect("constructed URL should always parse")
}

macro_rules! skip_without_server {
    () => {
        if test_server_addr().is_none() {
            eprintln!(
                "SDM_TEST_FTP_ADDR not set — skipping FTP integration test. \
                 See .github/workflows/ci.yml's ftp-integration job to run this for real."
            );
            return;
        }
    };
}

#[tokio::test]
async fn downloads_a_file_end_to_end() {
    skip_without_server!();
    let url = test_url("download_test.bin");
    let content: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();

    // Seed the file via upload first, so this test is self-contained.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    tokio::fs::write(tmp.path(), &content).await.unwrap();
    let mut session = FtpSession::connect(&url).await.unwrap();
    session
        .upload(tmp.path(), "download_test.bin")
        .await
        .unwrap();
    session.quit().await.unwrap();

    let mut session = FtpSession::connect(&url).await.unwrap();
    let dest = tempfile::NamedTempFile::new().unwrap();
    let written = session
        .download("download_test.bin", dest.path(), 0, None)
        .await
        .unwrap();
    session.quit().await.unwrap();

    assert_eq!(written, content.len() as u64);
    let on_disk = tokio::fs::read(dest.path()).await.unwrap();
    assert_eq!(on_disk, content);
}

#[tokio::test]
async fn uploads_a_file() {
    skip_without_server!();
    let url = test_url("upload_test.bin");
    let content = b"hello from sdm's FTP upload test".to_vec();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    tokio::fs::write(tmp.path(), &content).await.unwrap();

    let mut session = FtpSession::connect(&url).await.unwrap();
    let written = session.upload(tmp.path(), "upload_test.bin").await.unwrap();
    session.quit().await.unwrap();
    assert_eq!(written, content.len() as u64);

    // Round-trip: download it back and compare.
    let mut session = FtpSession::connect(&url).await.unwrap();
    let dest = tempfile::NamedTempFile::new().unwrap();
    session
        .download("upload_test.bin", dest.path(), 0, None)
        .await
        .unwrap();
    session.quit().await.unwrap();
    assert_eq!(tokio::fs::read(dest.path()).await.unwrap(), content);
}

/// Simulates "resume after kill": download half the file, then start a
/// fresh session and resume from where the on-disk file leaves off via
/// `REST`, matching how `sdm-engine::ftp` recovers a killed process.
#[tokio::test]
async fn resumes_a_partial_download() {
    skip_without_server!();
    let url = test_url("resume_test.bin");
    let content: Vec<u8> = (0..128 * 1024).map(|i| ((i * 7) % 256) as u8).collect();

    let tmp = tempfile::NamedTempFile::new().unwrap();
    tokio::fs::write(tmp.path(), &content).await.unwrap();
    let mut session = FtpSession::connect(&url).await.unwrap();
    session.upload(tmp.path(), "resume_test.bin").await.unwrap();
    session.quit().await.unwrap();

    let dest = tempfile::NamedTempFile::new().unwrap();
    let halfway = content.len() as u64 / 2;
    tokio::fs::write(dest.path(), &content[..halfway as usize])
        .await
        .unwrap();

    let mut session = FtpSession::connect(&url).await.unwrap();
    let written = session
        .download("resume_test.bin", dest.path(), halfway, None)
        .await
        .unwrap();
    session.quit().await.unwrap();

    assert_eq!(written, content.len() as u64 - halfway);
    let on_disk = tokio::fs::read(dest.path()).await.unwrap();
    assert_eq!(on_disk, content);
}

#[tokio::test]
async fn lists_directory_contents() {
    skip_without_server!();
    let url = test_url("");
    let mut session = FtpSession::connect(&url).await.unwrap();
    // Make sure there's at least one known entry to find.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    tokio::fs::write(tmp.path(), b"x").await.unwrap();
    session
        .upload(tmp.path(), "listing_probe.txt")
        .await
        .unwrap();

    let entries = session.list_dir(None).await.unwrap();
    session.quit().await.unwrap();

    assert!(
        entries.iter().any(|e| e.contains("listing_probe.txt")),
        "expected to find listing_probe.txt in directory listing: {entries:?}"
    );
}
