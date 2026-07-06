//! Shared SSH transport plumbing for SFTP and SCP (Sprint 8).
//!
//! `crates/protocols/src/sftp.rs` and `crates/protocols/src/scp.rs` both
//! ride on one `russh::client` session, so URL parsing, authentication,
//! and `known_hosts` verification live here once instead of being
//! duplicated per protocol.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::keys::{load_secret_key, ssh_key, PublicKey};
use russh::ChannelId;

use crate::error::ErrorClass;

#[derive(Debug, thiserror::Error)]
pub enum SshProtoError {
    #[error("SSH transport error: {0}")]
    Ssh(#[from] russh::Error),
    #[error("SSH key error: {0}")]
    Key(#[from] russh::keys::Error),
    #[error("SFTP error: {0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error("authentication failed for user {user:?}")]
    AuthFailed { user: String },
    #[error(
        "host key mismatch for {host}:{port} — the server's key does not match the one on \
         record in {known_hosts_path}. This could mean the server was reconfigured, or it \
         could mean a man-in-the-middle attack. Refusing to connect."
    )]
    HostKeyMismatch {
        host: String,
        port: u16,
        known_hosts_path: String,
    },
    #[error(
        "unknown host key for {host}:{port} (not in {known_hosts_path}). Re-run with \
         --accept-new-hostkey to trust it on first connection."
    )]
    UnknownHostKey {
        host: String,
        port: u16,
        known_hosts_path: String,
    },
    #[error("invalid SSH URL: {0}")]
    InvalidUrl(String),
    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("SCP protocol error: {0}")]
    ScpProtocol(String),
}

impl SshProtoError {
    /// Coarse error classification, reusing the retry FSM's existing
    /// vocabulary (see `crates/protocols::ftp` for the same pattern).
    pub fn class(&self) -> ErrorClass {
        match self {
            SshProtoError::Ssh(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("timed out") || msg.contains("timeout") {
                    ErrorClass::Timeout
                } else if msg.contains("dns") || msg.contains("resolve") || msg.contains("lookup") {
                    ErrorClass::DnsFailure
                } else {
                    ErrorClass::Other
                }
            }
            SshProtoError::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => ErrorClass::Timeout,
            // Auth failures and host-key problems are never transient —
            // retrying with the same credentials/known_hosts state will
            // fail identically every time.
            SshProtoError::AuthFailed { .. }
            | SshProtoError::HostKeyMismatch { .. }
            | SshProtoError::UnknownHostKey { .. }
            | SshProtoError::InvalidUrl(_) => ErrorClass::Other,
            _ => ErrorClass::Other,
        }
    }

    /// Whether this class of failure is worth retrying at all — narrower
    /// than `class().is_retryable()` because a host-key mismatch must
    /// never be retried into acceptance no matter how `ErrorClass::Other`
    /// is normally treated.
    pub fn is_retryable(&self) -> bool {
        !matches!(
            self,
            SshProtoError::AuthFailed { .. }
                | SshProtoError::HostKeyMismatch { .. }
                | SshProtoError::UnknownHostKey { .. }
                | SshProtoError::InvalidUrl(_)
        )
    }
}

/// A parsed `sftp://` or `scp://` URL:
/// `sftp[|scp]://[user[:pass]@]host[:port]/path`.
///
/// `path` is stored exactly as it appears after the host (leading slash
/// stripped), which matters for how it's later handed to
/// `SSH_FXP_OPEN`/`scp -f`/`scp -t`: a *relative* path (the common case,
/// e.g. `sftp://user@host/documents/file.txt` → `path` =
/// `"documents/file.txt"`) resolves relative to the login's home
/// directory, exactly like typing that path at an interactive `sftp`
/// prompt. An *absolute* path needs a doubled slash in the URL —
/// `sftp://user@host//var/www/file.txt` → `path` = `"/var/www/file.txt"`
/// — matching the convention curl and most SFTP clients use, since a
/// bare single slash can't distinguish "absolute" from "the first path
/// segment happens to be empty."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshUrl {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Password from the URL, if present. Prefer `--ssh-key` in real
    /// usage — this exists mainly for parity with `ftp://user:pass@`
    /// URLs and quick local testing.
    pub password: Option<String>,
    /// Remote path, without the leading slash.
    pub path: String,
}

impl SshUrl {
    pub fn parse(url: &str, expected_scheme: &str) -> Result<Self, SshProtoError> {
        let prefix = format!("{expected_scheme}://");
        let rest = url
            .strip_prefix(&prefix)
            .ok_or_else(|| SshProtoError::InvalidUrl(format!("not a {prefix} URL: {url}")))?;

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
                Some((user, pass)) => (decode(user), Some(decode(pass))),
                None => (decode(u), None),
            },
            None => {
                let current_user = std::env::var("USER")
                    .or_else(|_| std::env::var("USERNAME"))
                    .unwrap_or_else(|_| "root".to_string());
                (current_user, None)
            }
        };

        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|_| SshProtoError::InvalidUrl(format!("bad port in {url}")))?,
            ),
            None => (host_port.to_string(), 22),
        };

        if host.is_empty() {
            return Err(SshProtoError::InvalidUrl(format!("missing host in {url}")));
        }

        Ok(SshUrl {
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

/// How to authenticate an SSH session.
#[derive(Clone)]
pub enum SshAuth {
    Password(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
}

/// `known_hosts` verification policy — mirrors OpenSSH's
/// `StrictHostKeyChecking` in spirit, but with only the two modes
/// `docs/SPRINT_PLAN_PHASE2.md` Sprint 8 asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// Reject any host key not already present in `known_hosts` (default).
    Strict,
    /// Trust and persist a host key seen for the first time. A key that
    /// *conflicts* with an existing entry is still always rejected,
    /// regardless of this setting — that's the actual MITM-relevant case.
    AcceptNew,
}

/// Default `known_hosts` path: `~/.ssh/known_hosts`, matching OpenSSH.
pub fn default_known_hosts_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ssh").join("known_hosts")
}

/// The exact host identifier OpenSSH writes into `known_hosts`:
/// bare hostname for port 22, `[host]:port` otherwise.
fn host_entry_label(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Look up `host:port` in the `known_hosts` file at `path`. Returns
/// `Some(matching_key_line)` if an entry for this host exists (regardless
/// of whether the key itself matches), `None` if the host has never been
/// seen. Hashed (`|1|...`) entries are skipped — matching OpenSSH's HMAC
/// hashing scheme would need the per-entry salt embedded in the hash
/// itself, which this minimal client-side implementation doesn't attempt;
/// plain hostname entries (the default for anything sdm itself appends)
/// are fully supported.
fn find_host_entry(path: &Path, host: &str, port: u16) -> Option<(String, PublicKey)> {
    let contents = std::fs::read_to_string(path).ok()?;
    let label = host_entry_label(host, port);
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("|1|") {
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let hosts_field = parts.next()?;
        if !hosts_field.split(',').any(|h| h == label || h == host) {
            continue;
        }
        let rest = format!("{} {}", parts.next()?, parts.next().unwrap_or(""));
        if let Ok(key) = ssh_key::PublicKey::from_openssh(rest.trim()) {
            return Some((line.to_string(), key));
        }
    }
    None
}

/// Append a newly-trusted host key to `known_hosts`, creating the file
/// (and its parent `~/.ssh` directory) if needed.
fn append_host_key(path: &Path, host: &str, port: u16, key: &PublicKey) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let label = host_entry_label(host, port);
    let line = format!(
        "{label} {}\n",
        key.to_openssh().unwrap_or_default().trim_end()
    );
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

/// Verify a server's host key against `known_hosts`, applying
/// [`HostKeyPolicy`]. This is the single decision point both the
/// production client handler and unit tests exercise.
pub fn verify_host_key(
    known_hosts_path: &Path,
    host: &str,
    port: u16,
    server_key: &PublicKey,
    policy: HostKeyPolicy,
) -> Result<(), SshProtoError> {
    match find_host_entry(known_hosts_path, host, port) {
        Some((_line, recorded_key)) => {
            if recorded_key == *server_key {
                Ok(())
            } else {
                // A conflicting entry is ALWAYS rejected — accept-new only
                // covers hosts with no entry at all, never a mismatch.
                Err(SshProtoError::HostKeyMismatch {
                    host: host.to_string(),
                    port,
                    known_hosts_path: known_hosts_path.to_string_lossy().to_string(),
                })
            }
        }
        None => match policy {
            HostKeyPolicy::AcceptNew => {
                append_host_key(known_hosts_path, host, port, server_key)?;
                Ok(())
            }
            HostKeyPolicy::Strict => Err(SshProtoError::UnknownHostKey {
                host: host.to_string(),
                port,
                known_hosts_path: known_hosts_path.to_string_lossy().to_string(),
            }),
        },
    }
}

/// `russh::client::Handler` that delegates host-key decisions to
/// [`verify_host_key`]. Kept deliberately dumb otherwise — sdm has no use
/// for interactive channel data on the client side outside the SFTP
/// subsystem and SCP's exec channel, both of which read their channel
/// output directly rather than through this handler.
pub struct SshClientHandler {
    pub host: String,
    pub port: u16,
    pub known_hosts_path: PathBuf,
    pub policy: HostKeyPolicy,
}

impl russh::client::Handler for SshClientHandler {
    type Error = SshProtoError;

    async fn check_server_key(&mut self, server_key: &PublicKey) -> Result<bool, Self::Error> {
        match verify_host_key(
            &self.known_hosts_path,
            &self.host,
            self.port,
            server_key,
            self.policy,
        ) {
            Ok(()) => Ok(true),
            Err(e) => Err(e),
        }
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        // SCP's exec-channel reader (`crate::scp`) drains channel data via
        // `Channel::wait()` directly rather than through the handler, so
        // there's nothing to do here — see docs/TECH_DECISIONS.md's note
        // that russh's `Handler` is a connection-lifecycle callback, not
        // the primary data path for a client we're driving ourselves.
        Ok(())
    }
}

/// A live, authenticated SSH session shared by SFTP and SCP. One
/// `russh::client::Handle` can open many channels concurrently, which is
/// exactly what SFTP's multi-channel segmented download (below) needs.
pub struct SshSession {
    pub handle: russh::client::Handle<SshClientHandler>,
}

impl SshSession {
    pub async fn connect(
        url: &SshUrl,
        auth: &SshAuth,
        known_hosts_path: PathBuf,
        policy: HostKeyPolicy,
    ) -> Result<Self, SshProtoError> {
        let config = Arc::new(russh::client::Config::default());
        let handler = SshClientHandler {
            host: url.host.clone(),
            port: url.port,
            known_hosts_path,
            policy,
        };
        let mut handle =
            russh::client::connect(config, (url.host.as_str(), url.port), handler).await?;

        let success = match auth {
            SshAuth::Password(password) => handle
                .authenticate_password(&url.user, password)
                .await?
                .success(),
            SshAuth::PrivateKey { path, passphrase } => {
                let key_pair = load_secret_key(path, passphrase.as_deref())?;
                let hash_alg = handle.best_supported_rsa_hash().await?.flatten();
                let key_with_hash =
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash_alg);
                handle
                    .authenticate_publickey(&url.user, key_with_hash)
                    .await?
                    .success()
            }
        };

        if !success {
            return Err(SshProtoError::AuthFailed {
                user: url.user.clone(),
            });
        }

        Ok(SshSession { handle })
    }

    /// Open a fresh channel on this session and request the `sftp`
    /// subsystem on it, returning the resulting [`russh_sftp::client::SftpSession`].
    /// Multiple calls on the same `SshSession` open independent channels,
    /// which is what powers multi-channel segmented downloads.
    pub async fn open_sftp(&self) -> Result<russh_sftp::client::SftpSession, SshProtoError> {
        let channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
        Ok(sftp)
    }
}

/// Empty extension map, used where `russh_sftp` callbacks want one.
#[allow(dead_code)]
pub(crate) fn no_extensions() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sftp_url_with_password_and_port() {
        let url =
            SshUrl::parse("sftp://alice:hunter2@example.com:2222/srv/file.bin", "sftp").unwrap();
        assert_eq!(url.host, "example.com");
        assert_eq!(url.port, 2222);
        assert_eq!(url.user, "alice");
        assert_eq!(url.password.as_deref(), Some("hunter2"));
        assert_eq!(url.path, "srv/file.bin");
    }

    #[test]
    fn parses_scp_url_defaulting_port_22() {
        let url = SshUrl::parse("scp://bob@example.com/home/bob/file.tar.gz", "scp").unwrap();
        assert_eq!(url.port, 22);
        assert_eq!(url.user, "bob");
        assert_eq!(url.password, None);
    }

    #[test]
    fn rejects_wrong_scheme() {
        let err = SshUrl::parse("ftp://example.com/file", "sftp").unwrap_err();
        assert!(matches!(err, SshProtoError::InvalidUrl(_)));
    }

    #[test]
    fn defaults_to_current_user_without_userinfo() {
        let url = SshUrl::parse("sftp://example.com/path", "sftp").unwrap();
        assert!(!url.user.is_empty());
    }

    fn test_keypair() -> russh::keys::PrivateKey {
        russh::keys::PrivateKey::random(
            &mut russh::keys::ssh_key::rand_core::UnwrapErr(rand::rngs::SysRng),
            russh::keys::Algorithm::Ed25519,
        )
        .unwrap()
    }

    #[test]
    fn accepts_and_persists_unknown_host_key_with_accept_new_policy() {
        let dir = tempfile::tempdir().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        let key_pair = test_keypair();
        let public = key_pair.public_key();

        // First connection: unknown host, Strict must reject...
        let strict_err = verify_host_key(
            &known_hosts,
            "example.com",
            22,
            public,
            HostKeyPolicy::Strict,
        )
        .unwrap_err();
        assert!(matches!(strict_err, SshProtoError::UnknownHostKey { .. }));

        // ...but AcceptNew trusts it and writes it to disk.
        verify_host_key(
            &known_hosts,
            "example.com",
            22,
            public,
            HostKeyPolicy::AcceptNew,
        )
        .unwrap();
        assert!(known_hosts.exists());

        // Second connection: now that it's recorded, even Strict accepts it.
        verify_host_key(
            &known_hosts,
            "example.com",
            22,
            public,
            HostKeyPolicy::Strict,
        )
        .unwrap();
    }

    #[test]
    fn rejects_mismatched_host_key_even_with_accept_new_policy() {
        let dir = tempfile::tempdir().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        let original = test_keypair();
        let attacker = test_keypair();

        verify_host_key(
            &known_hosts,
            "example.com",
            22,
            original.public_key(),
            HostKeyPolicy::AcceptNew,
        )
        .unwrap();

        // A *different* key for the same host must be rejected under
        // BOTH policies — this is the actual MITM-relevant guarantee.
        for policy in [HostKeyPolicy::Strict, HostKeyPolicy::AcceptNew] {
            let err = verify_host_key(
                &known_hosts,
                "example.com",
                22,
                attacker.public_key(),
                policy,
            )
            .unwrap_err();
            assert!(matches!(err, SshProtoError::HostKeyMismatch { .. }));
        }
    }

    #[test]
    fn distinguishes_hosts_by_nonstandard_port() {
        let dir = tempfile::tempdir().unwrap();
        let known_hosts = dir.path().join("known_hosts");
        let key_pair = test_keypair();

        verify_host_key(
            &known_hosts,
            "example.com",
            2222,
            key_pair.public_key(),
            HostKeyPolicy::AcceptNew,
        )
        .unwrap();

        // Port 22 on the same hostname is a distinct, still-unknown entry.
        let err = verify_host_key(
            &known_hosts,
            "example.com",
            22,
            key_pair.public_key(),
            HostKeyPolicy::Strict,
        )
        .unwrap_err();
        assert!(matches!(err, SshProtoError::UnknownHostKey { .. }));
    }
}
