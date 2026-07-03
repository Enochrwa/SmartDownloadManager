//! Per-chunk corruption detection and targeted repair (Sprint 4).
//!
//! Each segment is divided into fixed-size chunks. Once a segment finishes
//! downloading, we read it back off disk chunk by chunk and record a CRC32
//! for each — a cheap "this is what we believe is on disk" fingerprint.
//!
//! Later (immediately after download, or on user-triggered `sdm verify`),
//! we can re-read the file and recompute those CRCs. Any chunk whose CRC no
//! longer matches is corrupted (silent disk corruption, a torn write, a
//! transfer that slipped past HTTP's own error handling, etc.) and gets
//! re-fetched — critically, *only* that chunk's byte range, not the whole
//! segment or file.

use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use reqwest::Client;
use sdm_storage::{ChunkRecord, SqlitePool};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::EngineError;
use crate::verify::crc32_bytes;

/// Default chunk size for corruption-detection hashing: 256 KiB. Small
/// enough that a repair only re-fetches a small range, large enough not to
/// flood the `chunks` table for big files.
pub const DEFAULT_CHUNK_SIZE: u64 = 256 * 1024;

/// Split an inclusive byte range `[start, end]` into contiguous
/// `chunk_size`-ish pieces (the last piece absorbs the remainder).
pub fn plan_chunks(start: u64, end: u64, chunk_size: u64) -> Vec<(u64, u64)> {
    if end < start {
        return vec![];
    }
    let chunk_size = chunk_size.max(1);
    let mut chunks = Vec::new();
    let mut pos = start;
    while pos <= end {
        let chunk_end = (pos + chunk_size - 1).min(end);
        chunks.push((pos, chunk_end));
        pos = chunk_end + 1;
    }
    chunks
}

/// Read `[start, end]` (inclusive) back from `path` and CRC32 it.
async fn hash_range(path: &Path, start: u64, end: u64) -> Result<u32, EngineError> {
    let mut file = File::open(path).await?;
    file.seek(SeekFrom::Start(start)).await?;
    let len = (end - start + 1) as usize;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).await?;
    Ok(crc32_bytes(&buf))
}

/// Hash the whole completed file in fixed-size chunks and persist the
/// results, replacing any previous chunk rows for this job. Called once a
/// segmented download finishes (Sprint 4 hooks this into `run_segmented`'s
/// completion path) so corruption can later be localized to a specific
/// byte range instead of forcing a whole-file re-download.
pub async fn hash_and_record_file(
    pool: &SqlitePool,
    job_id: &str,
    path: &Path,
    total_bytes: u64,
    chunk_size: u64,
) -> Result<(), EngineError> {
    if total_bytes == 0 {
        sdm_storage::replace_chunks(pool, job_id, &[]).await?;
        return Ok(());
    }
    let boundaries = plan_chunks(0, total_bytes - 1, chunk_size);
    let mut rows = Vec::with_capacity(boundaries.len());
    for (i, (start, end)) in boundaries.iter().enumerate() {
        let crc = hash_range(path, *start, *end).await?;
        rows.push((i as i64, *start as i64, *end as i64, crc));
    }
    sdm_storage::replace_chunks(pool, job_id, &rows).await?;
    Ok(())
}

/// One chunk whose on-disk CRC32 no longer matches what was recorded.
#[derive(Debug, Clone)]
pub struct CorruptChunk {
    pub record_id: String,
    pub start_byte: u64,
    pub end_byte: u64,
}

/// Re-read the whole file chunk-by-chunk and compare against the recorded
/// CRC32s. Returns the list of chunks that no longer match.
pub async fn find_corrupt_chunks(
    pool: &SqlitePool,
    job_id: &str,
    path: &Path,
) -> Result<Vec<CorruptChunk>, EngineError> {
    let chunks: Vec<ChunkRecord> = sdm_storage::get_chunks(pool, job_id).await?;
    let mut corrupt = Vec::new();
    for c in chunks {
        let actual = hash_range(path, c.start_byte as u64, c.end_byte as u64).await?;
        if actual != c.crc32 as u32 {
            corrupt.push(CorruptChunk {
                record_id: c.id,
                start_byte: c.start_byte as u64,
                end_byte: c.end_byte as u64,
            });
        }
    }
    Ok(corrupt)
}

/// Re-fetch exactly one corrupted chunk's byte range from `url` and
/// overwrite it in place, then update the recorded CRC32. This is the
/// "targeted re-download of only the bad chunk" behavior from the Sprint 4
/// DoD: the HTTP request's `Range` header is scoped to `[start_byte,
/// end_byte]` and nothing outside it is touched.
pub async fn repair_chunk(
    client: &Client,
    url: &str,
    file: Arc<AsyncMutex<File>>,
    pool: &SqlitePool,
    chunk: &CorruptChunk,
) -> Result<(), EngineError> {
    let end = Arc::new(AtomicU64::new(chunk.end_byte));
    let position = Arc::new(AtomicU64::new(chunk.start_byte));
    sdm_protocols::download_range(
        client,
        url,
        chunk.start_byte,
        end,
        file.clone(),
        position,
        None,
    )
    .await?;

    let mut f = file.lock().await;
    let len = (chunk.end_byte - chunk.start_byte + 1) as usize;
    let mut buf = vec![0u8; len];
    f.seek(SeekFrom::Start(chunk.start_byte)).await?;
    f.read_exact(&mut buf).await?;
    drop(f);
    let new_crc = crc32_bytes(&buf);
    sdm_storage::update_chunk_crc32(pool, &chunk.record_id, new_crc).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_chunks_covers_range_without_gaps() {
        let chunks = plan_chunks(0, 999, 256);
        assert_eq!(chunks.first().unwrap().0, 0);
        assert_eq!(chunks.last().unwrap().1, 999);
        for w in chunks.windows(2) {
            assert_eq!(w[0].1 + 1, w[1].0);
        }
    }

    #[test]
    fn plan_chunks_single_chunk_when_smaller_than_size() {
        let chunks = plan_chunks(10, 20, 256);
        assert_eq!(chunks, vec![(10, 20)]);
    }

    #[tokio::test]
    async fn hash_and_find_corrupt_round_trip() {
        let pool = sdm_storage::connect_in_memory().await.unwrap();
        sdm_storage::insert_job(&pool, "job-1", "https://example.com/f", "/tmp/f")
            .await
            .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.bin");
        let data: Vec<u8> = (0..2000u32).map(|i| (i % 256) as u8).collect();
        tokio::fs::write(&path, &data).await.unwrap();

        hash_and_record_file(&pool, "job-1", &path, data.len() as u64, 256)
            .await
            .unwrap();

        // No corruption yet.
        let corrupt = find_corrupt_chunks(&pool, "job-1", &path).await.unwrap();
        assert!(corrupt.is_empty());

        // Corrupt a few bytes inside the third chunk (bytes 512..767).
        let mut corrupted_data = data.clone();
        corrupted_data[600] ^= 0xFF;
        corrupted_data[601] ^= 0xFF;
        tokio::fs::write(&path, &corrupted_data).await.unwrap();

        let corrupt = find_corrupt_chunks(&pool, "job-1", &path).await.unwrap();
        assert_eq!(corrupt.len(), 1);
        assert_eq!(corrupt[0].start_byte, 512);
        assert_eq!(corrupt[0].end_byte, 767);
    }
}
