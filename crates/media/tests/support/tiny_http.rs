//! A deliberately tiny HTTP/1.1 fixture server, used only by this
//! crate's integration tests to serve a local video file that yt-dlp's
//! `generic` extractor can pick up — standing in for a real video site
//! the same way `wiremock` stands in for a real HTTP API elsewhere in
//! this workspace, but without pulling in `wiremock`'s modern hyper/
//! rustls dependency stack (see the comment in `crates/media/Cargo.toml`
//! dev-dependencies for why that matters in this project's sandbox).
//!
//! Deliberately minimal: handles exactly one `GET /<path>` request per
//! connection, ignores headers, and always responds 200 with the full
//! body plus a `Content-Type`/`Content-Length`. That's everything yt-dlp
//! needs from a direct-file URL.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct FixtureServer {
    pub port: u16,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl FixtureServer {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Start a fixture server serving `files` (path, without leading slash ->
/// (content-type, body)). Runs until the returned [`FixtureServer`] (and
/// its embedded shutdown sender) is dropped.
pub async fn start(files: HashMap<&'static str, (&'static str, Vec<u8>)>) -> FixtureServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a local test port");
    let port = listener.local_addr().expect("local addr").port();

    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let files = Arc::new(files);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = listener.accept() => {
                    let Ok((mut socket, _)) = accepted else { continue };
                    let files = Arc::clone(&files);
                    tokio::spawn(async move {
                        let _ = handle_one(&mut socket, &files).await;
                    });
                }
            }
        }
    });

    FixtureServer {
        port,
        _shutdown: tx,
    }
}

async fn handle_one(
    socket: &mut tokio::net::TcpStream,
    files: &HashMap<&'static str, (&'static str, Vec<u8>)>,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; 8192];
    let mut request = Vec::new();
    // Read until we see the end of the headers; we don't care about a
    // request body for GET fixture requests.
    loop {
        let n = socket.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buf[..n]);
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    let request_text = String::from_utf8_lossy(&request);
    let first_line = request_text.lines().next().unwrap_or_default();
    let mut parts = first_line.split_whitespace();
    let _method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or("/").trim_start_matches('/');

    match files.get(path) {
        Some((content_type, body)) => {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccept-Ranges: none\r\n\r\n",
                body.len()
            );
            socket.write_all(header.as_bytes()).await?;
            socket.write_all(body).await?;
        }
        None => {
            let header =
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            socket.write_all(header).await?;
        }
    }
    socket.flush().await
}
