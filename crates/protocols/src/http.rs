//! HTTP/HTTPS downloading via reqwest + rustls.
//!
//! - Sprint 1: single-stream GET with streaming write-to-disk.
//! - Sprint 2: `probe` (Range-request capability detection) and
//!   `download_range` (segmented, positioned-write downloads that support
//!   live shrinking of a segment's upper bound for segment stealing).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE};
use reqwest::{Client, StatusCode};
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::{classify_status, classify_transport_error, parse_retry_after, ErrorClass};

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("transport error: {0}")]
    Transport(#[source] reqwest::Error, ErrorClass),
    #[error("server returned HTTP {status}")]
    Http { status: u16, class: ErrorClass },
    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl ProtoError {
    pub fn class(&self) -> ErrorClass {
        match self {
            ProtoError::Transport(_, class) => class.clone(),
            ProtoError::Http { class, .. } => class.clone(),
            ProtoError::Io(_) => ErrorClass::Other,
        }
    }
}

/// Build the shared reqwest client. TLS 1.2/1.3 via rustls (no native-tls /
/// OpenSSL dependency — see docs/TECH_DECISIONS.md).
pub fn build_client() -> Client {
    Client::builder()
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60 * 30))
        .build()
        .expect("reqwest client config is valid")
}

#[derive(Debug, Clone, Default)]
pub struct ProbeInfo {
    pub total_bytes: Option<u64>,
    pub supports_range: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Probe a URL: does the server support `Range` requests, what's the total
/// size, and what validators (`ETag`/`Last-Modified`) does it expose for
/// later resume validation (Sprint 3)?
///
/// Tries `HEAD` first; many servers don't implement it well (or at all), so
/// we fall back to a `GET` with `Range: bytes=0-0` and inspect whether we
/// get back a `206 Partial Content`.
pub async fn probe(client: &Client, url: &str) -> Result<ProbeInfo, ProtoError> {
    let head_result = client.head(url).send().await;

    let resp = match head_result {
        Ok(r) if r.status().is_success() => r,
        _ => client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|e| {
                let class = classify_transport_error(&e);
                ProtoError::Transport(e, class)
            })?,
    };

    let status = resp.status();
    let headers = resp.headers().clone();

    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
        let retry_after = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let class = classify_status(status.as_u16(), retry_after);
        return Err(ProtoError::Http {
            status: status.as_u16(),
            class,
        });
    }

    let supports_range = status == StatusCode::PARTIAL_CONTENT
        || headers
            .get(ACCEPT_RANGES)
            .map(|v| v.as_bytes() == b"bytes")
            .unwrap_or(false);

    let total_bytes = if status == StatusCode::PARTIAL_CONTENT {
        headers
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_total)
    } else {
        headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse().ok())
    };

    let etag = headers
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let last_modified = headers
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    Ok(ProbeInfo {
        total_bytes,
        supports_range,
        etag,
        last_modified,
    })
}

/// Parses the total-size component out of a `Content-Range: bytes 0-0/12345` header.
fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.parse().ok()
}

/// Sprint 1: single-stream GET, written to `dest` as it arrives. Returns the
/// total number of bytes written.
pub async fn download_single(
    client: &Client,
    url: &str,
    dest: &Path,
    progress_tx: Option<UnboundedSender<u64>>,
) -> Result<u64, ProtoError> {
    let resp = client.get(url).send().await.map_err(|e| {
        let class = classify_transport_error(&e);
        ProtoError::Transport(e, class)
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let class = classify_status(status.as_u16(), retry_after);
        return Err(ProtoError::Http {
            status: status.as_u16(),
            class,
        });
    }

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let mut file = File::create(dest).await?;
    let mut stream = resp.bytes_stream();
    let mut total: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            let class = classify_transport_error(&e);
            ProtoError::Transport(e, class)
        })?;
        file.write_all(&chunk).await?;
        total += chunk.len() as u64;
        if let Some(tx) = &progress_tx {
            let _ = tx.send(chunk.len() as u64);
        }
    }
    file.flush().await?;
    Ok(total)
}

/// Sprint 2: ranged GET, writing each chunk at its absolute file offset via
/// seek+write so multiple segments can safely share one preallocated file.
///
/// `end` is an `Arc<AtomicU64>` (inclusive byte offset) so the segment
/// allocator can shrink it concurrently — segment stealing. This function
/// polls `end` on every chunk and truncates/stops exactly at the (possibly
/// new, smaller) boundary, dropping the underlying response stream, which
/// cancels that connection cleanly without corrupting any other segment's
/// bytes (each segment only ever writes within its own byte range).
///
/// `position` is updated to the next byte to be written as progress is made,
/// so the allocator can read it concurrently to compute "how much of this
/// segment is left" for stealing decisions.
pub async fn download_range(
    client: &Client,
    url: &str,
    start: u64,
    end: Arc<AtomicU64>,
    file: Arc<AsyncMutex<File>>,
    position: Arc<AtomicU64>,
    progress_tx: Option<UnboundedSender<u64>>,
) -> Result<u64, ProtoError> {
    let initial_end = end.load(Ordering::SeqCst);
    let range_header = format!("bytes={start}-{initial_end}");

    let resp = client
        .get(url)
        .header(RANGE, range_header)
        .send()
        .await
        .map_err(|e| {
            let class = classify_transport_error(&e);
            ProtoError::Transport(e, class)
        })?;

    let status = resp.status();
    if status != StatusCode::PARTIAL_CONTENT && !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let class = classify_status(status.as_u16(), retry_after);
        return Err(ProtoError::Http {
            status: status.as_u16(),
            class,
        });
    }

    let mut stream = resp.bytes_stream();
    let mut pos = start;
    position.store(pos, Ordering::SeqCst);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            let class = classify_transport_error(&e);
            ProtoError::Transport(e, class)
        })?;

        let target_end = end.load(Ordering::SeqCst);
        if pos > target_end {
            break; // boundary was shrunk out from under us before this chunk
        }
        let remaining_allowed = target_end - pos + 1;
        let write_len = (chunk.len() as u64).min(remaining_allowed);

        if write_len > 0 {
            let mut f = file.lock().await;
            f.seek(SeekFrom::Start(pos)).await?;
            f.write_all(&chunk[..write_len as usize]).await?;
            f.flush().await?;
        }

        pos += write_len;
        position.store(pos, Ordering::SeqCst);
        if let Some(tx) = &progress_tx {
            let _ = tx.send(write_len);
        }

        if write_len < chunk.len() as u64 {
            // Hit a (possibly shrunk) boundary mid-chunk — stop, the rest of
            // this byte range now belongs to whoever stole it.
            break;
        }
    }

    Ok(pos - start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[tokio::test]
    async fn probe_detects_range_support_via_head() {
        let server = MockServer::start().await;
        let data = body(1000);
        Mock::given(method("HEAD"))
            .and(path("/file.bin"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Accept-Ranges", "bytes")
                    .insert_header("Content-Length", data.len().to_string())
                    .insert_header("ETag", "\"abc123\""),
            )
            .mount(&server)
            .await;

        let client = build_client();
        let info = probe(&client, &format!("{}/file.bin", server.uri()))
            .await
            .unwrap();
        assert!(info.supports_range);
        assert_eq!(info.total_bytes, Some(1000));
        assert_eq!(info.etag.as_deref(), Some("\"abc123\""));
    }

    #[tokio::test]
    async fn probe_falls_back_to_ranged_get_when_head_unsupported() {
        let server = MockServer::start().await;
        // No HEAD mock registered at all -> reqwest gets a 404 for HEAD, triggering fallback.
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", "bytes 0-0/5000")
                    .set_body_bytes(vec![0u8]),
            )
            .mount(&server)
            .await;

        let client = build_client();
        let info = probe(&client, &format!("{}/file.bin", server.uri()))
            .await
            .unwrap();
        assert!(info.supports_range);
        assert_eq!(info.total_bytes, Some(5000));
    }

    #[tokio::test]
    async fn download_single_writes_full_body() {
        let server = MockServer::start().await;
        let data = body(20_000);
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone()))
            .mount(&server)
            .await;

        let client = build_client();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        let total = download_single(&client, &format!("{}/file.bin", server.uri()), &dest, None)
            .await
            .unwrap();

        assert_eq!(total, data.len() as u64);
        let written = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(written, data);
    }

    #[tokio::test]
    async fn download_single_surfaces_http_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.bin"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = build_client();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        let err = download_single(
            &client,
            &format!("{}/missing.bin", server.uri()),
            &dest,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err.class(), ErrorClass::HttpError(404));
    }

    #[tokio::test]
    async fn download_range_writes_at_correct_offset() {
        let server = MockServer::start().await;
        let full = body(1000);
        let slice = full[200..400].to_vec();
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", "bytes 200-399/1000")
                    .set_body_bytes(slice.clone()),
            )
            .mount(&server)
            .await;

        let client = build_client();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        let f = File::create(&dest).await.unwrap();
        f.set_len(1000).await.unwrap();
        let file = Arc::new(AsyncMutex::new(f));
        let end = Arc::new(AtomicU64::new(399));
        let position = Arc::new(AtomicU64::new(200));

        let written = download_range(
            &client,
            &format!("{}/file.bin", server.uri()),
            200,
            end,
            file,
            position,
            None,
        )
        .await
        .unwrap();
        assert_eq!(written, 200);

        let on_disk = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(&on_disk[200..400], slice.as_slice());
        // Bytes outside the written range stay zeroed (from preallocation).
        assert!(on_disk[0..200].iter().all(|&b| b == 0));
    }

    #[tokio::test]
    async fn download_range_stops_at_shrunk_boundary_for_segment_stealing() {
        let server = MockServer::start().await;
        let full = body(2000);
        let slice = full[0..1000].to_vec();
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", "bytes 0-999/2000")
                    .set_body_bytes(slice),
            )
            .mount(&server)
            .await;

        let client = build_client();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        let f = File::create(&dest).await.unwrap();
        f.set_len(2000).await.unwrap();
        let file = Arc::new(AsyncMutex::new(f));
        let end = Arc::new(AtomicU64::new(999));
        let position = Arc::new(AtomicU64::new(0));

        // Simulate the allocator stealing the back half right away.
        end.store(199, Ordering::SeqCst);

        let written = download_range(
            &client,
            &format!("{}/file.bin", server.uri()),
            0,
            end,
            file,
            position,
            None,
        )
        .await
        .unwrap();
        assert!(
            written <= 200,
            "worker must not write past the shrunk boundary, wrote {written}"
        );
    }
}
