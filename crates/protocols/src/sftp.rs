//! SFTP downloading/uploading via `russh` + `russh-sftp` (Sprint 8).
//!
//! Unlike FTP (Sprint 7), SFTP has no separate control/data connection —
//! every operation (`SSH_FXP_OPEN`, `SSH_FXP_READ` at an offset, directory
//! listing) rides one SSH channel with the `sftp` subsystem requested on
//! it. One `SshSession` can open many such channels concurrently, so
//! SFTP *does* get a real segmented, multi-connection download the way
//! HTTP does — each segment just opens its own SFTP subsystem channel on
//! the same authenticated session instead of its own TCP connection.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex as AsyncMutex;

pub use crate::ssh::{HostKeyPolicy, SshAuth, SshProtoError, SshSession, SshUrl};

const READ_CHUNK: usize = 256 * 1024;

/// One directory entry as returned by [`list_dir`].
#[derive(Debug, Clone)]
pub struct SftpDirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Get the size of a remote file via `SSH_FXP_STAT`, used both to plan
/// segments and to validate a resume (a size that shrank since the last
/// attempt means the remote file changed — same class of problem
/// `crates/engine::resume` already handles for HTTP via ETag).
pub async fn stat_size(session: &SshSession, remote_path: &str) -> Result<u64, SshProtoError> {
    let sftp = session.open_sftp().await?;
    let attrs = sftp.metadata(remote_path).await?;
    Ok(attrs.size.unwrap_or(0))
}

/// Directory listing via `SSH_FXP_READDIR`.
pub async fn list_dir(
    session: &SshSession,
    remote_path: &str,
) -> Result<Vec<SftpDirEntry>, SshProtoError> {
    let sftp = session.open_sftp().await?;
    let entries = sftp.read_dir(remote_path).await?;
    Ok(entries
        .map(|e| SftpDirEntry {
            name: e.file_name(),
            is_dir: e.file_type().is_dir(),
            size: e.metadata().size.unwrap_or(0),
        })
        .collect())
}

/// Single-stream download with resume, the SFTP analogue of
/// `crate::ftp::FtpSession::download` — used when the caller only wants
/// one channel (small files, or a server/network that doesn't benefit
/// from parallelism).
pub async fn download_single(
    session: &SshSession,
    remote_path: &str,
    dest: &Path,
    resume_from: u64,
    progress_tx: Option<UnboundedSender<u64>>,
) -> Result<u64, SshProtoError> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let sftp = session.open_sftp().await?;
    let mut remote_file = sftp.open_with_flags(remote_path, OpenFlags::READ).await?;
    remote_file
        .seek(std::io::SeekFrom::Start(resume_from))
        .await?;

    let mut local_file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dest)
        .await?;
    local_file.set_len(resume_from).await?;
    local_file
        .seek(std::io::SeekFrom::Start(resume_from))
        .await?;

    let mut buf = vec![0u8; READ_CHUNK];
    let mut total = 0u64;
    loop {
        let n = remote_file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        local_file.write_all(&buf[..n]).await?;
        total += n as u64;
        if let Some(tx) = &progress_tx {
            let _ = tx.send(n as u64);
        }
    }
    local_file.flush().await?;
    Ok(total)
}

/// Upload `local_path` to `remote_path`, creating (and truncating) the
/// remote file.
pub async fn upload(
    session: &SshSession,
    local_path: &Path,
    remote_path: &str,
) -> Result<u64, SshProtoError> {
    let sftp = session.open_sftp().await?;
    let mut remote_file = sftp
        .open_with_flags(
            remote_path,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await?;

    let mut local_file = tokio::fs::File::open(local_path).await?;
    let mut buf = vec![0u8; READ_CHUNK];
    let mut total = 0u64;
    loop {
        let n = local_file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        remote_file.write_all(&buf[..n]).await?;
        total += n as u64;
    }
    remote_file.flush().await?;
    remote_file.shutdown().await?;
    Ok(total)
}

/// Download one byte range `[start, end]` (inclusive) of `remote_path`
/// into `file` at the matching offset, on its own SFTP channel. Mirrors
/// `crate::http::download_range`'s contract exactly — same
/// live-shrinkable `end` for segment stealing, same `position` counter —
/// so `crates/engine`'s segment allocator (Sprint 2) works unmodified
/// against SFTP segments too.
pub async fn download_range(
    session: &SshSession,
    remote_path: &str,
    start: u64,
    end: Arc<AtomicU64>,
    file: Arc<AsyncMutex<tokio::fs::File>>,
    position: Arc<AtomicU64>,
    progress_tx: Option<UnboundedSender<u64>>,
) -> Result<u64, SshProtoError> {
    let sftp = session.open_sftp().await?;
    let mut remote_file = sftp.open_with_flags(remote_path, OpenFlags::READ).await?;
    remote_file.seek(std::io::SeekFrom::Start(start)).await?;

    let mut pos = start;
    position.store(pos, Ordering::SeqCst);
    let mut buf = vec![0u8; READ_CHUNK];

    loop {
        let target_end = end.load(Ordering::SeqCst);
        if pos > target_end {
            break;
        }
        let remaining_allowed = target_end - pos + 1;
        let want = (buf.len() as u64).min(remaining_allowed) as usize;
        if want == 0 {
            break;
        }
        let n = remote_file.read(&mut buf[..want]).await?;
        if n == 0 {
            break; // EOF
        }
        {
            let mut f = file.lock().await;
            f.seek(std::io::SeekFrom::Start(pos)).await?;
            f.write_all(&buf[..n]).await?;
            f.flush().await?;
        }
        pos += n as u64;
        position.store(pos, Ordering::SeqCst);
        if let Some(tx) = &progress_tx {
            let _ = tx.send(n as u64);
        }
        // Deliberately no "short read means stop" shortcut here: a
        // `read()` returning fewer bytes than requested is normal over a
        // network SFTP session (it does not imply EOF), so the loop just
        // recomputes `target_end` — which only truly shrinks when a
        // sibling segment steals the tail of this range — on the next
        // iteration instead of guessing based on read size.
    }

    Ok(pos - start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::verify_host_key;

    #[test]
    fn dir_entry_carries_type_and_size() {
        let entry = SftpDirEntry {
            name: "file.txt".into(),
            is_dir: false,
            size: 42,
        };
        assert!(!entry.is_dir);
        assert_eq!(entry.size, 42);
    }

    // Real transfer behavior (download/upload/resume/multi-channel speed)
    // is covered end-to-end in
    // `crates/protocols/tests/sftp_integration.rs` against a real OpenSSH
    // server, matching the established pattern in `ftp_integration.rs` —
    // there's no lightweight in-process SFTP *server* mock worth adding
    // as a dependency just for tests (russh-sftp's own server support
    // exists but modeling OpenSSH's actual quirks is exactly the point
    // of testing against the real thing).
    #[test]
    fn known_hosts_helper_is_reexported_for_callers() {
        // Smoke-test the re-export path callers (crates/engine::ssh) use.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let key = russh::keys::PrivateKey::random(
            &mut russh::keys::ssh_key::rand_core::UnwrapErr(rand::rngs::SysRng),
            russh::keys::Algorithm::Ed25519,
        )
        .unwrap();
        verify_host_key(&path, "h", 22, key.public_key(), HostKeyPolicy::AcceptNew).unwrap();
    }
}
