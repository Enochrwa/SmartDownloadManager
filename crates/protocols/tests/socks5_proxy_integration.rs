//! Sprint 12 DoD: "a download through a configured SOCKS5 proxy with auth
//! succeeds and is verified (via test proxy server logs) to have actually
//! routed through it."
//!
//! Rather than pull in a third-party SOCKS5 server crate just for tests
//! (one more dependency to vet against `deny.toml`, for something this
//! small), this implements the minimal slice of RFC 1928 (SOCKS5) + RFC
//! 1929 (username/password subnegotiation) needed to prove routing:
//! greeting -> [optional auth] -> CONNECT -> relay. Every CONNECT target
//! it sees is appended to a shared, awaitable log the test asserts
//! against — that log *is* the "test proxy server logs" the DoD calls
//! for.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use sdm_protocols::{build_client_with_proxy, download_single, probe, ProxyConfig};

/// What the test proxy observed. Cloned out and asserted on after the
/// download completes.
#[derive(Default, Clone)]
struct ProxyLog(Arc<Mutex<Vec<String>>>);

impl ProxyLog {
    fn record(&self, target: String) {
        self.0.lock().unwrap().push(target);
    }
    fn targets(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Socks5TestServer {
    addr: SocketAddr,
    log: ProxyLog,
}

/// `None` means "no auth required"; `Some((user, pass))` means the server
/// rejects any connection that doesn't present exactly these credentials.
async fn start_socks5_test_server(
    required_auth: Option<(&'static str, &'static str)>,
) -> Socks5TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log = ProxyLog::default();
    let log_for_task = log.clone();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let log = log_for_task.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_socks5_connection(stream, required_auth, log).await {
                    tracing_light_log(&format!("socks5 test server connection error: {e}"));
                }
            });
        }
    });

    Socks5TestServer { addr, log }
}

// A dependency-free stand-in for `tracing::debug!` — this crate's tests
// don't otherwise need a tracing subscriber wired up, and a failed test
// connection isn't itself a test failure (the assertions on `log` are
// what matter), so this just goes to stderr for local debugging.
fn tracing_light_log(msg: &str) {
    eprintln!("{msg}");
}

async fn handle_socks5_connection(
    mut client: TcpStream,
    required_auth: Option<(&'static str, &'static str)>,
    log: ProxyLog,
) -> std::io::Result<()> {
    // --- Greeting: VER(1) NMETHODS(1) METHODS(NMETHODS) ---
    let mut header = [0u8; 2];
    client.read_exact(&mut header).await?;
    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    client.read_exact(&mut methods).await?;

    let selected_method: u8 = if required_auth.is_some() { 0x02 } else { 0x00 };
    client.write_all(&[0x05, selected_method]).await?;

    if let Some((expected_user, expected_pass)) = required_auth {
        // --- Username/password subnegotiation (RFC 1929) ---
        let mut ver_ulen = [0u8; 2];
        client.read_exact(&mut ver_ulen).await?;
        let ulen = ver_ulen[1] as usize;
        let mut uname = vec![0u8; ulen];
        client.read_exact(&mut uname).await?;

        let mut plen_buf = [0u8; 1];
        client.read_exact(&mut plen_buf).await?;
        let plen = plen_buf[0] as usize;
        let mut passwd = vec![0u8; plen];
        client.read_exact(&mut passwd).await?;

        let ok = uname == expected_user.as_bytes() && passwd == expected_pass.as_bytes();
        client
            .write_all(&[0x01, if ok { 0x00 } else { 0x01 }])
            .await?;
        if !ok {
            return Ok(());
        }
    }

    // --- Request: VER(1) CMD(1) RSV(1) ATYP(1) DST.ADDR DST.PORT(2) ---
    let mut req_header = [0u8; 4];
    client.read_exact(&mut req_header).await?;
    let atyp = req_header[3];

    let target = match atyp {
        0x01 => {
            // IPv4
            let mut addr = [0u8; 4];
            client.read_exact(&mut addr).await?;
            let mut port_buf = [0u8; 2];
            client.read_exact(&mut port_buf).await?;
            let port = u16::from_be_bytes(port_buf);
            format!("{}.{}.{}.{}:{}", addr[0], addr[1], addr[2], addr[3], port)
        }
        0x03 => {
            // Domain name
            let mut len_buf = [0u8; 1];
            client.read_exact(&mut len_buf).await?;
            let mut domain = vec![0u8; len_buf[0] as usize];
            client.read_exact(&mut domain).await?;
            let mut port_buf = [0u8; 2];
            client.read_exact(&mut port_buf).await?;
            let port = u16::from_be_bytes(port_buf);
            format!("{}:{}", String::from_utf8_lossy(&domain), port)
        }
        other => {
            // ATYP 0x04 (IPv6) isn't exercised by this test suite;
            // reject cleanly rather than trying to half-parse it.
            client
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(std::io::Error::other(format!(
                "unsupported SOCKS5 ATYP {other}"
            )));
        }
    };

    log.record(target.clone());

    let upstream = match TcpStream::connect(&target).await {
        Ok(s) => s,
        Err(e) => {
            client
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(e);
        }
    };

    // Reply: success, bind addr/port are irrelevant for this test.
    client
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();
    let c2u = tokio::io::copy(&mut client_read, &mut upstream_write);
    let u2c = tokio::io::copy(&mut upstream_read, &mut client_write);
    let _ = tokio::join!(c2u, u2c);
    Ok(())
}

#[tokio::test]
async fn download_actually_routes_through_authenticated_socks5_proxy() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let origin = MockServer::start().await;
    let body = b"hello through the proxy".to_vec();
    Mock::given(method("GET"))
        .and(path("/file.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&origin)
        .await;

    let proxy = start_socks5_test_server(Some(("sdmuser", "sdmpass"))).await;

    let proxy_cfg =
        ProxyConfig::new(format!("socks5h://{}", proxy.addr)).with_auth("sdmuser", "sdmpass");
    let client = build_client_with_proxy(Some(&proxy_cfg)).unwrap();

    let url = format!("{}/file.bin", origin.uri());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");

    let total = download_single(&client, &url, &dest, None).await.unwrap();
    assert_eq!(total, body.len() as u64);
    assert_eq!(tokio::fs::read(&dest).await.unwrap(), body);

    // The DoD's actual requirement: prove the request really went through
    // the proxy, not directly to the origin. The proxy's own connection
    // log recorded the origin's host:port as a CONNECT target, which is
    // only possible if reqwest tunneled through it.
    let logged_targets = proxy.log.targets();
    assert!(
        !logged_targets.is_empty(),
        "proxy handled no connections at all -- request bypassed it"
    );
    let origin_authority = origin.uri().replace("http://", "");
    assert!(
        logged_targets.iter().any(|t| t == &origin_authority),
        "proxy log {logged_targets:?} doesn't contain the origin {origin_authority}"
    );
}

#[tokio::test]
async fn download_through_socks5_proxy_fails_with_wrong_credentials() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"unreachable".to_vec()))
        .mount(&origin)
        .await;

    let proxy = start_socks5_test_server(Some(("sdmuser", "sdmpass"))).await;
    let wrong_cfg =
        ProxyConfig::new(format!("socks5h://{}", proxy.addr)).with_auth("sdmuser", "WRONG");
    let client = build_client_with_proxy(Some(&wrong_cfg)).unwrap();

    let url = format!("{}/file.bin", origin.uri());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");

    let result = download_single(&client, &url, &dest, None).await;
    assert!(
        result.is_err(),
        "download should fail when the proxy rejects the provided credentials"
    );
}

#[tokio::test]
async fn probe_also_routes_through_the_configured_proxy() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let origin = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/file.bin"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Accept-Ranges", "bytes")
                .insert_header("Content-Length", "42"),
        )
        .mount(&origin)
        .await;

    let proxy = start_socks5_test_server(None).await;
    let proxy_cfg = ProxyConfig::new(format!("socks5h://{}", proxy.addr));
    let client = build_client_with_proxy(Some(&proxy_cfg)).unwrap();

    let info = probe(&client, &format!("{}/file.bin", origin.uri()))
        .await
        .unwrap();
    assert_eq!(info.total_bytes, Some(42));
    assert!(!proxy.log.targets().is_empty());
}
