//! Download verification (Sprint 4).
//!
//! Computes a whole-file checksum with one of the standard algorithms and,
//! if the caller supplied an expected value up front, compares against it.
//! The actual checksum is always stored on the job record even when no
//! expected value was given, so it becomes available for later duplicate
//! detection (same content, different URL/filename).

use std::path::Path;

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::error::EngineError;

const READ_BUF_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
    Crc32,
}

impl ChecksumAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChecksumAlgorithm::Md5 => "md5",
            ChecksumAlgorithm::Sha1 => "sha1",
            ChecksumAlgorithm::Sha256 => "sha256",
            ChecksumAlgorithm::Sha512 => "sha512",
            ChecksumAlgorithm::Crc32 => "crc32",
        }
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "md5" => ChecksumAlgorithm::Md5,
            "sha1" => ChecksumAlgorithm::Sha1,
            "sha256" | "sha-256" => ChecksumAlgorithm::Sha256,
            "sha512" | "sha-512" => ChecksumAlgorithm::Sha512,
            "crc32" => ChecksumAlgorithm::Crc32,
            other => anyhow::bail!(
                "unknown checksum algorithm: {other} (expected one of md5, sha1, sha256, sha512, crc32)"
            ),
        })
    }
}

/// A user-supplied "algo:hex" expected-checksum spec, e.g. `sha256:abcd...`.
#[derive(Debug, Clone)]
pub struct ExpectedChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub hex: String,
}

impl ExpectedChecksum {
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let (algo, hex) = spec.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("expected checksum spec as \"algorithm:hex\", got: {spec}")
        })?;
        Ok(ExpectedChecksum {
            algorithm: ChecksumAlgorithm::parse(algo)?,
            hex: hex.trim().to_ascii_lowercase(),
        })
    }
}

enum Hasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
    Crc32(crc32fast::Hasher),
}

impl Hasher {
    fn new(algo: ChecksumAlgorithm) -> Self {
        match algo {
            ChecksumAlgorithm::Md5 => Hasher::Md5(Md5::new()),
            ChecksumAlgorithm::Sha1 => Hasher::Sha1(Sha1::new()),
            ChecksumAlgorithm::Sha256 => Hasher::Sha256(Sha256::new()),
            ChecksumAlgorithm::Sha512 => Hasher::Sha512(Sha512::new()),
            ChecksumAlgorithm::Crc32 => Hasher::Crc32(crc32fast::Hasher::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Md5(h) => h.update(data),
            Hasher::Sha1(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
            Hasher::Sha512(h) => h.update(data),
            Hasher::Crc32(h) => h.update(data),
        }
    }

    fn finalize_hex(self) -> String {
        match self {
            Hasher::Md5(h) => hex::encode(h.finalize()),
            Hasher::Sha1(h) => hex::encode(h.finalize()),
            Hasher::Sha256(h) => hex::encode(h.finalize()),
            Hasher::Sha512(h) => hex::encode(h.finalize()),
            Hasher::Crc32(h) => hex::encode(h.finalize().to_be_bytes()),
        }
    }
}

/// Stream `path` through the given algorithm and return the lowercase hex
/// digest. Reads in fixed-size chunks so multi-GB files don't need to be
/// held in memory.
pub async fn compute_file_checksum(
    path: &Path,
    algo: ChecksumAlgorithm,
) -> Result<String, EngineError> {
    let mut file = File::open(path).await?;
    let mut hasher = Hasher::new(algo);
    let mut buf = vec![0u8; READ_BUF_SIZE];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize_hex())
}

/// Compute the checksum of `path` and compare it against `expected`
/// (case-insensitive). Returns the computed hex digest either way so the
/// caller can persist it regardless of match/mismatch.
pub async fn verify_file(
    path: &Path,
    expected: &ExpectedChecksum,
) -> Result<(String, bool), EngineError> {
    let actual = compute_file_checksum(path, expected.algorithm).await?;
    let matches = actual.eq_ignore_ascii_case(&expected.hex);
    Ok((actual, matches))
}

/// CRC32 of an in-memory buffer — used for per-chunk hashing during/after
/// segmented downloads, where re-reading small ranges off disk is cheap.
pub fn crc32_bytes(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sha256_matches_known_vector() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.bin");
        tokio::fs::write(&path, b"abc").await.unwrap();

        let digest = compute_file_checksum(&path, ChecksumAlgorithm::Sha256)
            .await
            .unwrap();
        // Well-known SHA-256("abc").
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn md5_matches_known_vector() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.bin");
        tokio::fs::write(&path, b"abc").await.unwrap();

        let digest = compute_file_checksum(&path, ChecksumAlgorithm::Md5)
            .await
            .unwrap();
        assert_eq!(digest, "900150983cd24fb0d6963f7d28e17f72");
    }

    #[tokio::test]
    async fn verify_file_detects_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.bin");
        tokio::fs::write(&path, b"abc").await.unwrap();

        let expected = ExpectedChecksum::parse(
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let (_actual, matches) = verify_file(&path, &expected).await.unwrap();
        assert!(!matches);
    }

    #[test]
    fn expected_checksum_parses_algo_and_hex() {
        let parsed = ExpectedChecksum::parse("sha1:ABCDEF").unwrap();
        assert_eq!(parsed.algorithm, ChecksumAlgorithm::Sha1);
        assert_eq!(parsed.hex, "abcdef");
    }

    #[test]
    fn expected_checksum_rejects_missing_colon() {
        assert!(ExpectedChecksum::parse("sha1-abcdef").is_err());
    }
}
