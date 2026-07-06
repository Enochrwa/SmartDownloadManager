//! SCP downloading/uploading, layered on the same `russh` session as SFTP
//! (Sprint 8).
//!
//! SCP is deliberately the simpler, non-resumable sibling here: unlike
//! SFTP's `SSH_FXP_OPEN` (which takes an explicit byte offset) or FTP's
//! `REST` command, the legacy `scp://` wire protocol has no seek/offset
//! primitive at all — it's a straight-line "here's a control line, here's
//! the whole file, here's a trailing sentinel byte" exchange with no way
//! to ask the remote `scp` server process to start partway through a
//! file. `docs/SPRINT_PLAN_PHASE2.md` calls this out explicitly ("document
//! this limitation rather than faking resume"), so `download`/`upload`
//! below always transfer the complete file; callers that want resumable
//! transfers to an SSH host should use `crate::sftp` instead.

use std::path::Path;

use russh::ChannelMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

use crate::ssh::{SshProtoError, SshSession};

type ClientChannel = russh::Channel<russh::client::Msg>;

/// Shell-quote a path for embedding in the remote `scp -f`/`scp -t`
/// command line: wrap in single quotes, escaping any embedded single
/// quote as `'\''` (the standard POSIX-shell trick), so a path containing
/// spaces or shell metacharacters can't break out of the command.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// Thin reader over an exec channel's raw data frames, with a one-chunk
/// lookahead buffer so callers can hand back "leftover" bytes that
/// belonged to the *next* logical unit (e.g. the sentinel byte that can
/// legally arrive glued to the end of a file's last data frame).
struct ChannelReader<'a> {
    channel: &'a mut ClientChannel,
    leftover: Vec<u8>,
}

impl<'a> ChannelReader<'a> {
    fn new(channel: &'a mut ClientChannel) -> Self {
        Self {
            channel,
            leftover: Vec::new(),
        }
    }

    /// Pull the next nonempty chunk of channel data, preferring any
    /// buffered leftover before reading a fresh SSH packet.
    async fn next_chunk(&mut self) -> Result<Vec<u8>, SshProtoError> {
        if !self.leftover.is_empty() {
            return Ok(std::mem::take(&mut self.leftover));
        }
        loop {
            match self.channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    if !data.is_empty() {
                        return Ok(data.to_vec());
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    return Err(SshProtoError::ScpProtocol(format!(
                        "remote scp stderr: {}",
                        String::from_utf8_lossy(&data)
                    )));
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    return Err(SshProtoError::ScpProtocol(
                        "remote scp closed the channel unexpectedly".to_string(),
                    ));
                }
                _ => {}
            }
        }
    }

    /// Read one `\n`-terminated control line, e.g. `"C0644 1234 file.bin"`
    /// (the trailing newline is stripped). Any bytes read past the
    /// newline are stashed as leftover for the next read.
    async fn read_line(&mut self) -> Result<String, SshProtoError> {
        let mut line = Vec::new();
        loop {
            let chunk = self.next_chunk().await?;
            if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
                line.extend_from_slice(&chunk[..pos]);
                if pos + 1 < chunk.len() {
                    self.leftover = chunk[pos + 1..].to_vec();
                }
                return Ok(String::from_utf8_lossy(&line).into_owned());
            }
            line.extend_from_slice(&chunk);
        }
    }

    /// Read exactly `n` bytes of file payload, writing each piece to
    /// `sink` as it arrives (no need to buffer the whole file in memory).
    /// Any bytes read past the `n`th belong to the trailing sentinel byte
    /// and are stashed as leftover.
    async fn read_exact_to<W: AsyncWriteExt + Unpin>(
        &mut self,
        n: u64,
        sink: &mut W,
    ) -> Result<(), SshProtoError> {
        let mut remaining = n;
        while remaining > 0 {
            let chunk = self.next_chunk().await?;
            let take = (chunk.len() as u64).min(remaining) as usize;
            sink.write_all(&chunk[..take]).await?;
            remaining -= take as u64;
            if take < chunk.len() {
                self.leftover = chunk[take..].to_vec();
            }
        }
        Ok(())
    }

    /// Consume and discard exactly one byte (the sentinel that follows a
    /// file's payload).
    async fn read_sentinel(&mut self) -> Result<(), SshProtoError> {
        let chunk = self.next_chunk().await?;
        if chunk.len() > 1 {
            self.leftover = chunk[1..].to_vec();
        }
        Ok(())
    }

    async fn read_ack(&mut self) -> Result<(), SshProtoError> {
        let chunk = self.next_chunk().await?;
        match chunk.first() {
            Some(0) => {
                if chunk.len() > 1 {
                    self.leftover = chunk[1..].to_vec();
                }
                Ok(())
            }
            Some(_) => Err(SshProtoError::ScpProtocol(format!(
                "remote scp rejected the transfer: {}",
                String::from_utf8_lossy(&chunk[1.min(chunk.len())..])
            ))),
            None => Err(SshProtoError::ScpProtocol(
                "remote scp sent an empty ack frame".to_string(),
            )),
        }
    }
}

async fn send_byte(channel: &mut ClientChannel, byte: u8) -> Result<(), SshProtoError> {
    channel.data(&[byte][..]).await?;
    Ok(())
}

/// Download the whole file at `remote_path` via `scp -f`, writing it to
/// `dest`. Always a full re-download — see the module docs on why SCP
/// can't resume.
pub async fn download(
    session: &SshSession,
    remote_path: &str,
    dest: &Path,
    progress_tx: Option<UnboundedSender<u64>>,
) -> Result<u64, SshProtoError> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let mut channel = session.handle.channel_open_session().await?;
    let command = format!("scp -f {}", shell_quote(remote_path));
    channel.exec(true, command).await?;

    // Kick off the exchange: sink (us) sends a zero byte to request the
    // first control line.
    send_byte(&mut channel, 0).await?;
    let size = {
        let mut reader = ChannelReader::new(&mut channel);
        let control = reader.read_line().await?;
        // Expected form: "C<mode> <size> <filename>", e.g. "C0644 10485760 file.bin".
        control
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| {
                SshProtoError::ScpProtocol(format!("unrecognized scp control line: {control:?}"))
            })?
    };

    send_byte(&mut channel, 0).await?; // "go ahead and send the file"

    let mut file = tokio::fs::File::create(dest).await?;
    {
        let mut reader = ChannelReader::new(&mut channel);
        if let Some(tx) = &progress_tx {
            let mut counting = CountingWriter::new(&mut file, tx.clone());
            reader.read_exact_to(size, &mut counting).await?;
        } else {
            reader.read_exact_to(size, &mut file).await?;
        }
        reader.read_sentinel().await?;
    }
    file.flush().await?;

    send_byte(&mut channel, 0).await?; // ack the final sentinel
    let _ = channel.eof().await;
    let _ = channel.close().await;

    Ok(size)
}

/// Forwards each `write_all` call's length to a progress channel — lets
/// `download` report progress without buffering the whole file to compute
/// a total up front.
struct CountingWriter<'a, W> {
    inner: &'a mut W,
    tx: UnboundedSender<u64>,
}

impl<'a, W> CountingWriter<'a, W> {
    fn new(inner: &'a mut W, tx: UnboundedSender<u64>) -> Self {
        Self { inner, tx }
    }
}

impl<'a, W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for CountingWriter<'a, W> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let poll = std::pin::Pin::new(&mut *this.inner).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &poll {
            let _ = this.tx.send(*n as u64);
        }
        poll
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut *this.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut *this.inner).poll_shutdown(cx)
    }
}

/// Upload `local_path` to `remote_path` via `scp -t`.
pub async fn upload(
    session: &SshSession,
    local_path: &Path,
    remote_path: &str,
) -> Result<u64, SshProtoError> {
    let metadata = tokio::fs::metadata(local_path).await?;
    let size = metadata.len();
    let filename = Path::new(remote_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let remote_dir = Path::new(remote_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ".".to_string());

    let mut channel = session.handle.channel_open_session().await?;
    let command = format!("scp -t {}", shell_quote(&remote_dir));
    channel.exec(true, command).await?;

    {
        let mut reader = ChannelReader::new(&mut channel);
        reader.read_ack().await?;
    }
    let header = format!("C0644 {size} {filename}\n");
    channel.data(header.as_bytes()).await?;
    {
        let mut reader = ChannelReader::new(&mut channel);
        reader.read_ack().await?;
    }

    let mut file = tokio::fs::File::open(local_path).await?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut sent = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        channel.data(&buf[..n]).await?;
        sent += n as u64;
    }
    channel.data(&[0u8][..]).await?; // trailing sentinel
    {
        let mut reader = ChannelReader::new(&mut channel);
        reader.read_ack().await?;
    }
    let _ = channel.eof().await;
    let _ = channel.close().await;

    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("plain.txt"), "'plain.txt'");
        assert_eq!(shell_quote("it's.txt"), r"'it'\''s.txt'");
    }

    // Full download/upload behavior is covered in
    // `crates/protocols/tests/sftp_integration.rs` (SCP tests share the
    // same real-OpenSSH-server gate as the SFTP tests, since both ride
    // the one SSH connection).
}
