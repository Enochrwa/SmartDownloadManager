//! HTTP/HTTPS downloading via reqwest + rustls.
//!
//! Sprint 1: single-stream GET with streaming write-to-disk.
//! Sprint 2 will add Range-request probing and segmented downloads here.

use anyhow::Result;

pub async fn supports_range(_url: &str) -> Result<bool> {
    // TODO(Sprint 2): HEAD request, check Accept-Ranges header.
    unimplemented!("range probing lands in Sprint 2")
}
