//! Error type for `sdm-media`'s `yt-dlp`/FFmpeg subprocess wrappers.

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("failed to launch {binary}: {source}")]
    Spawn {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{binary} exited with status {status}: {stderr}")]
    NonZeroExit {
        binary: String,
        status: String,
        stderr: String,
    },

    #[error("failed to parse {binary} output as JSON: {source} (line: {line:?})")]
    Json {
        binary: String,
        #[source]
        source: serde_json::Error,
        line: String,
    },

    #[error("{binary} produced no usable output")]
    EmptyOutput { binary: String },

    #[error("{binary} binary checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch {
        binary: String,
        expected: String,
        actual: String,
    },

    #[error(
        "refusing to use this ffmpeg build: it was compiled with {flag}, which is not \
         LGPL-compliant per docs/LICENSING.md item 1 (no --enable-gpl/--enable-nonfree \
         allowed in the bundled binary). Pass `enforce_lgpl: false` only if you understand \
         the licensing implications of distributing GPL-linked binaries."
    )]
    NonLgplBuild { flag: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
