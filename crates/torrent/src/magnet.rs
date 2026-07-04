//! Magnet URI parsing.
//!
//! Scoped to BitTorrent v1 (`xt=urn:btih:<hash>`) magnet links, matching
//! `librqbit` 7.x's own v1-only support (v2/hybrid `btmh` magnets are
//! tracked as a Phase 3 follow-up, not a Sprint 7 requirement — see
//! `docs/SPRINT_PLAN_PHASE2.md`). We parse the URI ourselves, ahead of
//! handing it to `librqbit`, so `crates/engine` can populate the job's
//! display name and info-hash in storage immediately, without waiting on
//! a tracker/DHT round-trip.

use data_encoding::{BASE32, HEXLOWER_PERMISSIVE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagnetInfo {
    /// Lowercase 40-character hex-encoded SHA-1 info-hash.
    pub info_hash: String,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MagnetParseError {
    #[error("not a magnet: URI")]
    NotAMagnetUri,
    #[error("magnet URI is missing an `xt=urn:btih:` info-hash parameter")]
    MissingInfoHash,
    #[error("info-hash `{0}` is not valid 40-char hex or 32-char base32")]
    InvalidInfoHash(String),
}

/// Parse a `magnet:?xt=urn:btih:...&dn=...&tr=...` URI.
pub fn parse_magnet(uri: &str) -> Result<MagnetInfo, MagnetParseError> {
    let query = uri
        .trim()
        .strip_prefix("magnet:?")
        .ok_or(MagnetParseError::NotAMagnetUri)?;

    let mut info_hash = None;
    let mut display_name = None;
    let mut trackers = Vec::new();

    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, raw_val) = pair.split_once('=').unwrap_or((pair, ""));
        let val = decode(raw_val);
        match key {
            "xt" if info_hash.is_none() => {
                if let Some(hash) = val.strip_prefix("urn:btih:") {
                    info_hash = Some(normalize_hash(hash)?);
                }
                // `urn:btmh:` (v2) magnets are intentionally left
                // unrecognized this sprint; falls through to
                // `MissingInfoHash` below if no `btih` xt is also present.
            }
            "dn" => display_name = Some(val),
            "tr" => trackers.push(val),
            _ => {}
        }
    }

    let info_hash = info_hash.ok_or(MagnetParseError::MissingInfoHash)?;
    Ok(MagnetInfo {
        info_hash,
        display_name,
        trackers,
    })
}

fn normalize_hash(hash: &str) -> Result<String, MagnetParseError> {
    if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(hash.to_ascii_lowercase())
    } else if hash.len() == 32 {
        let upper = hash.to_ascii_uppercase();
        let decoded = BASE32
            .decode(upper.as_bytes())
            .map_err(|_| MagnetParseError::InvalidInfoHash(hash.to_string()))?;
        if decoded.len() != 20 {
            return Err(MagnetParseError::InvalidInfoHash(hash.to_string()));
        }
        Ok(HEXLOWER_PERMISSIVE.encode(&decoded))
    } else {
        Err(MagnetParseError::InvalidInfoHash(hash.to_string()))
    }
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// True if `path_or_uri` looks like something the torrent engine, not the
/// HTTP/FTP engines, should handle: a magnet link or a `.torrent` file path.
pub fn looks_like_torrent_source(path_or_uri: &str) -> bool {
    path_or_uri.starts_with("magnet:?") || path_or_uri.ends_with(".torrent")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_info_hash_with_name_and_trackers() {
        let uri = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=ubuntu.iso&tr=udp%3A%2F%2Ftracker.example.com%3A80&tr=http%3A%2F%2Ftracker2.example.com%2Fannounce";
        let info = parse_magnet(uri).unwrap();
        assert_eq!(info.info_hash, "c12fe1c06bba254a9dc9f519b335aa7c1367a88a");
        assert_eq!(info.display_name.as_deref(), Some("ubuntu.iso"));
        assert_eq!(
            info.trackers,
            vec![
                "udp://tracker.example.com:80",
                "http://tracker2.example.com/announce"
            ]
        );
    }

    #[test]
    fn parses_base32_info_hash() {
        // Base32 encoding of the same 20-byte hash as the hex test above.
        let hex = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
        let bytes = HEXLOWER_PERMISSIVE.decode(hex.as_bytes()).unwrap();
        let b32 = BASE32.encode(&bytes);
        let uri = format!("magnet:?xt=urn:btih:{b32}");
        let info = parse_magnet(&uri).unwrap();
        assert_eq!(info.info_hash, hex);
    }

    #[test]
    fn rejects_non_magnet_uri() {
        assert_eq!(
            parse_magnet("https://example.com/file.torrent"),
            Err(MagnetParseError::NotAMagnetUri)
        );
    }

    #[test]
    fn rejects_magnet_without_info_hash() {
        assert_eq!(
            parse_magnet("magnet:?dn=no-hash-here"),
            Err(MagnetParseError::MissingInfoHash)
        );
    }

    #[test]
    fn rejects_malformed_info_hash() {
        let err = parse_magnet("magnet:?xt=urn:btih:not-a-hash").unwrap_err();
        assert!(matches!(err, MagnetParseError::InvalidInfoHash(_)));
    }

    #[test]
    fn detects_torrent_sources() {
        assert!(looks_like_torrent_source("magnet:?xt=urn:btih:abc"));
        assert!(looks_like_torrent_source("/downloads/ubuntu-24.04.torrent"));
        assert!(!looks_like_torrent_source("https://example.com/file.zip"));
        assert!(!looks_like_torrent_source("ftp://example.com/file.zip"));
    }
}
