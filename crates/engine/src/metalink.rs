//! Metalink (Sprint 9) engine glue.
//!
//! Per `docs/SPRINT_PLAN_PHASE2.md`, Metalink needs **no new download
//! logic**: a `<file>` entry is just a structured "one primary URL, N
//! mirror URLs, one or more pre-supplied hashes" — exactly the shape
//! [`crate::download::DownloadRequest`] already consumes via Sprint 4's
//! mirror-failover and checksum-verification machinery. This module's
//! only job is fetching + parsing the `.metalink`/`.meta4` document and
//! translating its first (or a chosen) `<file>` entry into that request;
//! `Engine::start_download` does the rest, unmodified.

use std::path::{Path, PathBuf};

use sdm_protocols::metalink::MetalinkFile;

use crate::download::DownloadRequest;
use crate::duplicate::DuplicatePolicy;
use crate::error::EngineError;
use crate::segment::ConnectionsOption;
use crate::verify::ExpectedChecksum;

/// Where the `.metalink`/`.meta4` document itself comes from.
#[derive(Debug, Clone)]
pub enum MetalinkSource {
    Url(String),
    LocalPath(PathBuf),
}

impl MetalinkSource {
    /// `http(s)://...` is treated as a remote document to fetch; anything
    /// else is treated as a local filesystem path — matching how the CLI
    /// already distinguishes remote vs. local elsewhere (e.g. torrent
    /// magnet URIs vs. `.torrent` files).
    pub fn parse(s: &str) -> Self {
        if s.starts_with("http://") || s.starts_with("https://") {
            MetalinkSource::Url(s.to_string())
        } else {
            MetalinkSource::LocalPath(PathBuf::from(s))
        }
    }
}

/// Fetch (if remote) and parse a Metalink document into its list of
/// `<file>` entries.
pub async fn fetch_and_parse(
    client: &reqwest::Client,
    source: &MetalinkSource,
) -> Result<Vec<MetalinkFile>, EngineError> {
    let text = match source {
        MetalinkSource::Url(url) => {
            let resp = client.get(url).send().await.map_err(|e| {
                EngineError::Other(format!("failed to fetch Metalink document: {e}"))
            })?;
            if !resp.status().is_success() {
                return Err(EngineError::Other(format!(
                    "failed to fetch Metalink document: HTTP {}",
                    resp.status()
                )));
            }
            resp.text()
                .await
                .map_err(|e| EngineError::Other(format!("failed to read Metalink document: {e}")))?
        }
        MetalinkSource::LocalPath(path) => tokio::fs::read_to_string(path)
            .await
            .map_err(|e| EngineError::Other(format!("failed to read {}: {e}", path.display())))?,
    };

    sdm_protocols::metalink::parse(&text)
        .map_err(|e| EngineError::Other(format!("failed to parse Metalink document: {e}")))
}

/// Translate one Metalink `<file>` entry into an ordinary
/// [`DownloadRequest`]: first mirror URL (already priority-sorted) is the
/// primary, the rest widen the mirror pool; the strongest available hash
/// becomes the expected checksum.
pub fn build_download_request(
    file: &MetalinkFile,
    destination_dir: &Path,
    connections: ConnectionsOption,
    duplicate_policy: DuplicatePolicy,
) -> Result<DownloadRequest, EngineError> {
    let mut urls = file.urls.iter().map(|u| u.url.clone());
    let primary = urls
        .next()
        .ok_or_else(|| EngineError::Other("Metalink file has no mirror URLs".to_string()))?;
    let mirrors: Vec<String> = urls.collect();

    let expected_checksum = file
        .strongest_hash()
        .map(|h| {
            let algo = sdm_protocols::metalink::normalize_algo(&h.algorithm);
            ExpectedChecksum::parse(&format!("{algo}:{}", h.hex))
        })
        .transpose()
        .map_err(EngineError::from)?;

    Ok(DownloadRequest {
        url: primary,
        mirrors,
        destination: destination_dir.join(&file.name),
        connections,
        expected_checksum,
        duplicate_policy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdm_protocols::metalink::{MetalinkHash, MetalinkUrl};

    fn sample_file() -> MetalinkFile {
        MetalinkFile {
            name: "example.iso".to_string(),
            size: Some(1024),
            hashes: vec![MetalinkHash {
                algorithm: "sha-256".to_string(),
                hex: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85".to_string(),
            }],
            urls: vec![
                MetalinkUrl {
                    url: "https://mirror-a.example.com/example.iso".to_string(),
                    priority: Some(1),
                },
                MetalinkUrl {
                    url: "https://mirror-b.example.com/example.iso".to_string(),
                    priority: Some(2),
                },
            ],
        }
    }

    #[test]
    fn builds_request_with_primary_and_mirrors() {
        let file = sample_file();
        let req = build_download_request(
            &file,
            Path::new("/tmp/downloads"),
            ConnectionsOption::Auto,
            DuplicatePolicy::Rename,
        )
        .unwrap();
        assert_eq!(req.url, "https://mirror-a.example.com/example.iso");
        assert_eq!(
            req.mirrors,
            vec!["https://mirror-b.example.com/example.iso"]
        );
        assert_eq!(req.destination, PathBuf::from("/tmp/downloads/example.iso"));
        assert!(req.expected_checksum.is_some());
    }

    #[test]
    fn parses_local_vs_remote_source() {
        assert!(matches!(
            MetalinkSource::parse("https://example.com/file.metalink"),
            MetalinkSource::Url(_)
        ));
        assert!(matches!(
            MetalinkSource::parse("/tmp/file.metalink"),
            MetalinkSource::LocalPath(_)
        ));
    }
}
