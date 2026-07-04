//! FTP/FTPS downloading via `suppaftp` (tokio backend).
//!
//! `docs/FEATURES.md` §2 and `docs/PRD.md` §6 scope FTP/FTPS into Phase 1
//! ("download any direct HTTP/HTTPS/FTP URL"), but it was never built in
//! Sprints 1-6. `docs/SPRINT_PLAN_PHASE2.md`'s "Phase 1 carryover" closes
//! that gap in Sprint 7, ahead of SFTP/SCP/WebDAV in Sprint 8.
//!
//! Unlike HTTP, FTP has no notion of concurrent byte-range segments across
//! independent connections without non-standard server support, so this
//! module offers a single-stream downloader/uploader with `REST`-based
//! resume — the FTP analogue of Sprint 1's `download_single`, not Sprint
//! 2's segmented `download_range`.

use std::path::Path;

use suppaftp::tokio::AsyncRustlsConnector;
use suppaftp::tokio_rustls::rustls::{ClientConfig, RootCertStore};
use suppaftp::types::FileType;
use suppaftp::FtpError;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

use crate::error::ErrorClass;

type PlainFtpStream = suppaftp::tokio::AsyncFtpStream;
type SecureFtpStream = suppaftp::tokio::AsyncRustlsFtpStream;

#[derive(Debug, thiserror::Error)]
pub enum FtpProtoError {
    #[error("FTP error: {0}")]
    Ftp(#[source] FtpError),
    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid FTP URL: {0}")]
    InvalidUrl(String),
}

impl FtpProtoError {
    /// Coarse error classification, reusing the same [`ErrorClass`] the
    /// retry FSM already understands for HTTP so `crates/engine` doesn't
    /// need a second retry policy per protocol.
    pub fn class(&self) -> ErrorClass {
        match self {
            FtpProtoError::Io(_) => ErrorClass::Other,
            FtpProtoError::InvalidUrl(_) => ErrorClass::Other,
            FtpProtoError::Ftp(e) => classify_ftp_error(e),
        }
    }
}

impl From<FtpError> for FtpProtoError {
    fn from(e: FtpError) -> Self {
        FtpProtoError::Ftp(e)
    }
}

fn classify_ftp_error(err: &FtpError) -> ErrorClass {
    match err {
        FtpError::ConnectionError(io_err) => {
            let msg = io_err.to_string().to_lowercase();
            if io_err.kind() == std::io::ErrorKind::TimedOut || msg.contains("timed out") {
                ErrorClass::Timeout
            } else if msg.contains("dns") || msg.contains("resolve") || msg.contains("lookup") {
                ErrorClass::DnsFailure
            } else {
                ErrorClass::Other
            }
        }
        FtpError::SecureError(_) => ErrorClass::TlsFailure,
        FtpError::UnexpectedResponse(resp) => {
            // 4xx = transient (server busy / can't open data connection right
            // now), 5xx = permanent (bad credentials, no such file, etc).
            let code = resp.status.code();
            if (400..500).contains(&code) {
                ErrorClass::ServerBusy { retry_after: None }
            } else {
                ErrorClass::HttpError(code as u16)
            }
        }
        _ => ErrorClass::Other,
    }
}

/// A parsed `ftp://` or `ftps://` URL: `ftp[s]://[user[:pass]@]host[:port]/path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpUrl {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    /// Remote path, without the leading slash stripped from the URL.
    pub path: String,
}

impl FtpUrl {
    pub fn parse(url: &str) -> Result<Self, FtpProtoError> {
        let (secure, rest) = if let Some(r) = url.strip_prefix("ftps://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("ftp://") {
            (false, r)
        } else {
            return Err(FtpProtoError::InvalidUrl(format!(
                "not an ftp:// or ftps:// URL: {url}"
            )));
        };

        let (authority, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx + 1..]),
            None => (rest, ""),
        };

        let (userinfo, host_port) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, authority),
        };

        let (user, password) = match userinfo {
            Some(u) => match u.split_once(':') {
                Some((user, pass)) => (decode(user), decode(pass)),
                None => (decode(u), String::new()),
            },
            None => ("anonymous".to_string(), "anonymous@".to_string()),
        };

        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|_| FtpProtoError::InvalidUrl(format!("bad port in {url}")))?,
            ),
            None => (host_port.to_string(), if secure { 990 } else { 21 }),
        };

        if host.is_empty() {
            return Err(FtpProtoError::InvalidUrl(format!("missing host in {url}")));
        }

        Ok(FtpUrl {
            secure,
            host,
            port,
            user,
            password,
            path: decode(path),
        })
    }
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// Build a rustls `ClientConfig` trusting the Mozilla root store bundled via
/// `webpki-roots`, matching the TLS posture already used for HTTPS
/// (`docs/TECH_DECISIONS.md`: rustls everywhere, no native-tls/OpenSSL).
fn tls_client_config() -> ClientConfig {
    // rustls 0.23 requires a process-wide default `CryptoProvider` to be
    // installed before `ClientConfig::builder()` can be called with no
    // explicit provider. `install_default` is idempotent-safe to call more
    // than once (from repeated FTPS connections, or alongside reqwest's own
    // rustls stack) — a failure just means one was already installed.
    let _ = suppaftp::tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let root_store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

/// One connected, authenticated FTP session — plain or FTPS (explicit
/// `AUTH TLS`), depending on the scheme in [`FtpUrl`].
pub enum FtpSession {
    Plain(PlainFtpStream),
    Secure(Box<SecureFtpStream>),
}

impl FtpSession {
    /// Connect and log in. FTPS uses explicit `AUTH TLS` (the recommended
    /// mode per `docs/SPRINT_PLAN_PHASE2.md`) over the standard control
    /// port, not implicit TLS on port 990.
    pub async fn connect(url: &FtpUrl) -> Result<Self, FtpProtoError> {
        let addr = format!("{}:{}", url.host, url.port);

        if !url.secure {
            let mut stream = PlainFtpStream::connect(addr.as_str())
                .await
                .map_err(FtpProtoError::from)?;
            stream.login(&url.user, &url.password).await?;
            stream.transfer_type(FileType::Binary).await?;
            return Ok(FtpSession::Plain(stream));
        }

        let stream = SecureFtpStream::connect(addr.as_str())
            .await
            .map_err(FtpProtoError::from)?;
        let connector = AsyncRustlsConnector::from(suppaftp::tokio_rustls::TlsConnector::from(
            std::sync::Arc::new(tls_client_config()),
        ));
        let mut stream = stream.into_secure(connector, &url.host).await?;
        stream.login(&url.user, &url.password).await?;
        stream.transfer_type(FileType::Binary).await?;
        Ok(FtpSession::Secure(Box::new(stream)))
    }

    /// Download `remote_path` to `dest`, resuming from `resume_from` bytes
    /// (0 for a fresh download) via the `REST` command. Returns the total
    /// number of bytes written in this call (i.e. not counting bytes
    /// already on disk from a previous attempt).
    pub async fn download(
        &mut self,
        remote_path: &str,
        dest: &Path,
        resume_from: u64,
        progress_tx: Option<UnboundedSender<u64>>,
    ) -> Result<u64, FtpProtoError> {
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dest)
            .await?;
        file.set_len(resume_from).await?;
        file.seek(std::io::SeekFrom::Start(resume_from)).await?;

        if resume_from > 0 {
            self.resume_transfer(resume_from as usize).await?;
        }

        let mut reader = self.retr_as_stream(remote_path).await?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).await?;
            total += n as u64;
            if let Some(tx) = &progress_tx {
                let _ = tx.send(n as u64);
            }
        }
        file.flush().await?;
        self.finalize_retr(reader).await?;
        Ok(total)
    }

    /// Upload `local_path` to `remote_path`.
    pub async fn upload(
        &mut self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<u64, FtpProtoError> {
        let mut file = File::open(local_path).await?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await?;
        let mut cursor = std::io::Cursor::new(buf);
        let written = match self {
            FtpSession::Plain(s) => s.put_file(remote_path, &mut cursor).await?,
            FtpSession::Secure(s) => s.put_file(remote_path, &mut cursor).await?,
        };
        Ok(written)
    }

    /// Directory listing via `MLSD` (structured, RFC 3659) with a `LIST`
    /// fallback for servers that don't support it.
    pub async fn list_dir(&mut self, path: Option<&str>) -> Result<Vec<String>, FtpProtoError> {
        let mlsd = match self {
            FtpSession::Plain(s) => s.mlsd(path).await,
            FtpSession::Secure(s) => s.mlsd(path).await,
        };
        match mlsd {
            Ok(entries) => Ok(entries),
            Err(_) => {
                let list = match self {
                    FtpSession::Plain(s) => s.list(path).await?,
                    FtpSession::Secure(s) => s.list(path).await?,
                };
                Ok(list)
            }
        }
    }

    async fn resume_transfer(&mut self, offset: usize) -> Result<(), FtpProtoError> {
        match self {
            FtpSession::Plain(s) => s.resume_transfer(offset).await?,
            FtpSession::Secure(s) => s.resume_transfer(offset).await?,
        }
        Ok(())
    }

    /// Open a `RETR` data stream, boxed so the caller doesn't need to care
    /// whether this session is plain or TLS — both `DataStream<T>`
    /// instantiations are `Send + Unpin`, same as any `TcpStream`/`Box`ed
    /// TLS stream.
    async fn retr_as_stream(
        &mut self,
        remote_path: &str,
    ) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, FtpProtoError> {
        Ok(match self {
            FtpSession::Plain(s) => Box::new(s.retr_as_stream(remote_path).await?),
            FtpSession::Secure(s) => Box::new(s.retr_as_stream(remote_path).await?),
        })
    }

    async fn finalize_retr(
        &mut self,
        stream: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    ) -> Result<(), FtpProtoError> {
        match self {
            FtpSession::Plain(s) => s.finalize_retr_stream(stream).await?,
            FtpSession::Secure(s) => s.finalize_retr_stream(stream).await?,
        }
        Ok(())
    }

    pub async fn quit(self) -> Result<(), FtpProtoError> {
        match self {
            FtpSession::Plain(mut s) => s.quit().await?,
            FtpSession::Secure(mut s) => s.quit().await?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use suppaftp::Status;

    use super::*;

    #[test]
    fn parses_plain_ftp_url_with_credentials() {
        // Deliberately non-credential-shaped placeholders (no real word,
        // no realistic password entropy) so this test fixture isn't
        // mistaken for a leaked secret by scanners like GitGuardian.
        let (user, pass) = ("testuser01", "placeholder-not-a-secret");
        let url =
            FtpUrl::parse(&format!("ftp://{user}:{pass}@ftp.example.com/pub/file.zip")).unwrap();
        assert!(!url.secure);
        assert_eq!(url.host, "ftp.example.com");
        assert_eq!(url.port, 21);
        assert_eq!(url.user, "testuser01");
        assert_eq!(url.password, "placeholder-not-a-secret");
        assert_eq!(url.path, "pub/file.zip");
    }

    #[test]
    fn parses_ftps_url_with_explicit_port() {
        let url = FtpUrl::parse("ftps://ftp.example.com:2121/file.iso").unwrap();
        assert!(url.secure);
        assert_eq!(url.port, 2121);
        assert_eq!(url.user, "anonymous");
        assert_eq!(url.path, "file.iso");
    }

    #[test]
    fn defaults_to_anonymous_when_no_credentials_given() {
        let url = FtpUrl::parse("ftp://ftp.example.com/").unwrap();
        assert_eq!(url.user, "anonymous");
        assert_eq!(url.password, "anonymous@");
        assert_eq!(url.path, "");
    }

    #[test]
    fn rejects_non_ftp_scheme() {
        let err = FtpUrl::parse("https://example.com/file").unwrap_err();
        assert!(matches!(err, FtpProtoError::InvalidUrl(_)));
    }

    #[test]
    fn decodes_percent_encoded_userinfo_and_path() {
        let (user, pass) = ("account%40corp", "not%40secret");
        let url = FtpUrl::parse(&format!("ftp://{user}:{pass}@host/dir%20name/file.txt")).unwrap();
        assert_eq!(url.user, "account@corp");
        assert_eq!(url.password, "not@secret");
        assert_eq!(url.path, "dir name/file.txt");
    }

    #[test]
    fn classifies_5xx_as_permanent_http_error() {
        // 550 (file unavailable) should not be silently retried forever.
        let resp =
            suppaftp::types::Response::new(Status::FileUnavailable, b"no such file".to_vec());
        let err = FtpError::UnexpectedResponse(resp);
        assert!(!classify_ftp_error(&err).is_retryable());
    }

    #[test]
    fn classifies_4xx_as_transient_server_busy() {
        let resp = suppaftp::types::Response::new(
            Status::RequestedActionNotTaken,
            b"busy, try again".to_vec(),
        );
        let err = FtpError::UnexpectedResponse(resp);
        assert!(classify_ftp_error(&err).is_retryable());
    }
}
