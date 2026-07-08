//! sdm-storage: SQLite-backed persistence for jobs, segments, and settings.
//!
//! Sprint 1: schema + migrations, basic job CRUD.
//! Sprint 2: segment rows (one per connection).
//! Sprint 3: segment-state journaling — every status transition for a
//! segment or job is written through to SQLite immediately, so a crash at
//! any point leaves a recoverable, consistent on-disk state.
//! Sprint 4: checksum fields on jobs, per-chunk hash rows for targeted
//! corruption repair, mirror rows, and duplicate-detection lookups.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;
pub use sqlx::SqlitePool;

/// Open (creating if needed) the SQLite database at `db_path` and run all
/// pending migrations.
pub async fn connect(db_path: &str) -> anyhow::Result<SqlitePool> {
    let url = format!("sqlite://{db_path}?mode=rwc");
    let pool = SqlitePool::connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Open an in-memory database — handy for tests.
pub async fn connect_in_memory() -> anyhow::Result<SqlitePool> {
    let pool = SqlitePool::connect("sqlite::memory:").await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Probing,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Probing => "probing",
            JobStatus::Downloading => "downloading",
            JobStatus::Paused => "paused",
            JobStatus::Verifying => "verifying",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for JobStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "queued" => JobStatus::Queued,
            "probing" => JobStatus::Probing,
            "downloading" => JobStatus::Downloading,
            "paused" => JobStatus::Paused,
            "verifying" => JobStatus::Verifying,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            other => anyhow::bail!("unknown job status: {other}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentStatus {
    Pending,
    Downloading,
    Completed,
    Failed,
}

impl SegmentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SegmentStatus::Pending => "pending",
            SegmentStatus::Downloading => "downloading",
            SegmentStatus::Completed => "completed",
            SegmentStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for SegmentStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pending" => SegmentStatus::Pending,
            "downloading" => SegmentStatus::Downloading,
            "completed" => SegmentStatus::Completed,
            "failed" => SegmentStatus::Failed,
            other => anyhow::bail!("unknown segment status: {other}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Http,
    Ftp,
    Torrent,
    /// Sprint 8.
    Sftp,
    /// Sprint 8. Non-resumable (see `crates/protocols::scp` module docs).
    Scp,
    /// Sprint 8. Stored as a translated `http(s)://` URL under the hood —
    /// see `crates/engine::webdav` — this variant exists purely so
    /// `sdm status`/`sdm list` display the protocol the user actually
    /// asked for.
    WebDav,
    /// Sprint 9. HLS (`.m3u8`) — see `crates/engine::hls`.
    Hls,
    /// Sprint 9. MPEG-DASH (`.mpd`) — see `crates/engine::dash`. Metalink
    /// deliberately has *no* variant here: it resolves to an ordinary
    /// `Http` job with mirrors + a checksum (see 0004_sprint9.sql).
    Dash,
    /// Sprint 10. yt-dlp-backed extraction from one of yt-dlp's
    /// thousands of supported sites — see `crates/engine::media`.
    Media,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::Http => "http",
            JobKind::Ftp => "ftp",
            JobKind::Torrent => "torrent",
            JobKind::Sftp => "sftp",
            JobKind::Scp => "scp",
            JobKind::WebDav => "webdav",
            JobKind::Hls => "hls",
            JobKind::Dash => "dash",
            JobKind::Media => "media",
        }
    }
}

impl std::str::FromStr for JobKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "http" => JobKind::Http,
            "ftp" => JobKind::Ftp,
            "torrent" => JobKind::Torrent,
            "sftp" => JobKind::Sftp,
            "scp" => JobKind::Scp,
            "webdav" => JobKind::WebDav,
            "hls" => JobKind::Hls,
            "dash" => JobKind::Dash,
            "media" => JobKind::Media,
            other => anyhow::bail!("unknown job kind: {other}"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub status: JobStatus,
    pub job_kind: JobKind,
    pub total_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    pub connections: i64,
    pub supports_range: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
    pub checksum_algorithm: Option<String>,
    pub checksum_expected: Option<String>,
    pub checksum_actual: Option<String>,
    pub checksum_verified: bool,
    /// Sprint 10: set for a playlist/channel/album child job, pointing
    /// at the one parent Job the queue/UI groups it under. `None` for
    /// every job kind except a media child.
    pub parent_job_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentRecord {
    pub id: String,
    pub job_id: String,
    pub seq: i64,
    pub start_byte: i64,
    pub end_byte: i64,
    pub downloaded_bytes: i64,
    pub status: SegmentStatus,
    pub retry_count: i64,
    pub last_error_class: Option<String>,
}

/// Insert a brand-new job row in `Queued` status.
pub async fn insert_job(
    pool: &SqlitePool,
    id: &str,
    url: &str,
    destination: &str,
) -> anyhow::Result<()> {
    insert_job_with_kind(pool, id, url, destination, JobKind::Http).await
}

/// Same as [`insert_job`], but for a non-HTTP job kind (FTP/FTPS or
/// BitTorrent, Sprint 7). Kept as a separate function rather than adding a
/// parameter to `insert_job` so every existing Sprint 1-6 call site keeps
/// compiling unchanged.
pub async fn insert_job_with_kind(
    pool: &SqlitePool,
    id: &str,
    url: &str,
    destination: &str,
    kind: JobKind,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO jobs (id, url, destination, status, job_kind, downloaded_bytes, connections, supports_range, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 1, 0, ?6, ?6)",
    )
    .bind(id)
    .bind(url)
    .bind(destination)
    .bind(JobStatus::Queued.as_str())
    .bind(kind.as_str())
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Same as [`insert_job_with_kind`], but for a playlist/channel/album
/// child job (Sprint 10) that should be linked back to its parent Job.
/// A separate function for the same reason [`insert_job_with_kind`]
/// itself is separate from [`insert_job`]: every existing call site keeps
/// compiling unchanged.
pub async fn insert_child_job_with_kind(
    pool: &SqlitePool,
    id: &str,
    url: &str,
    destination: &str,
    kind: JobKind,
    parent_job_id: &str,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO jobs (id, url, destination, status, job_kind, downloaded_bytes, connections, supports_range, parent_job_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 1, 0, ?6, ?7, ?7)",
    )
    .bind(id)
    .bind(url)
    .bind(destination)
    .bind(JobStatus::Queued.as_str())
    .bind(kind.as_str())
    .bind(parent_job_id)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// All child jobs of a playlist/channel/album parent, in the order they
/// were created (i.e. the order yt-dlp's flat-playlist listing gave
/// them).
pub async fn list_child_jobs(
    pool: &SqlitePool,
    parent_job_id: &str,
) -> anyhow::Result<Vec<JobRecord>> {
    let rows = sqlx::query(
        "SELECT id, url, destination, status, job_kind, total_bytes, downloaded_bytes, connections,
                supports_range, etag, last_modified, error_class, error_message,
                checksum_algorithm, checksum_expected, checksum_actual, checksum_verified, parent_job_id
         FROM jobs WHERE parent_job_id = ?1 ORDER BY created_at ASC",
    )
    .bind(parent_job_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_job).collect()
}
/// validators, and chosen connection count.
#[allow(clippy::too_many_arguments)]
pub async fn update_job_probe(
    pool: &SqlitePool,
    id: &str,
    total_bytes: Option<i64>,
    supports_range: bool,
    etag: Option<&str>,
    last_modified: Option<&str>,
    connections: i64,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE jobs SET total_bytes = ?1, supports_range = ?2, etag = ?3, last_modified = ?4,
         connections = ?5, status = ?6, updated_at = ?7 WHERE id = ?8",
    )
    .bind(total_bytes)
    .bind(supports_range as i64)
    .bind(etag)
    .bind(last_modified)
    .bind(connections)
    .bind(JobStatus::Downloading.as_str())
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_job_status(pool: &SqlitePool, id: &str, status: JobStatus) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE jobs SET status = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(status.as_str())
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_job_error(
    pool: &SqlitePool,
    id: &str,
    class: &str,
    message: &str,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE jobs SET status = ?1, error_class = ?2, error_message = ?3, updated_at = ?4 WHERE id = ?5",
    )
    .bind(JobStatus::Failed.as_str())
    .bind(class)
    .bind(message)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_job_downloaded_bytes(
    pool: &SqlitePool,
    id: &str,
    downloaded_bytes: i64,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE jobs SET downloaded_bytes = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(downloaded_bytes)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_job(pool: &SqlitePool, id: &str) -> anyhow::Result<Option<JobRecord>> {
    let row = sqlx::query(
        "SELECT id, url, destination, status, job_kind, total_bytes, downloaded_bytes, connections,
                supports_range, etag, last_modified, error_class, error_message,
                checksum_algorithm, checksum_expected, checksum_actual, checksum_verified, parent_job_id
         FROM jobs WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(row_to_job(row)?))
}

pub async fn list_jobs(pool: &SqlitePool) -> anyhow::Result<Vec<JobRecord>> {
    let rows = sqlx::query(
        "SELECT id, url, destination, status, job_kind, total_bytes, downloaded_bytes, connections,
                supports_range, etag, last_modified, error_class, error_message,
                checksum_algorithm, checksum_expected, checksum_actual, checksum_verified, parent_job_id
         FROM jobs ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_job).collect()
}

fn row_to_job(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<JobRecord> {
    let status_str: String = row.try_get("status")?;
    let job_kind_str: String = row.try_get("job_kind")?;
    Ok(JobRecord {
        id: row.try_get("id")?,
        url: row.try_get("url")?,
        destination: row.try_get("destination")?,
        status: status_str.parse()?,
        job_kind: job_kind_str.parse()?,
        total_bytes: row.try_get("total_bytes")?,
        downloaded_bytes: row.try_get("downloaded_bytes")?,
        connections: row.try_get("connections")?,
        supports_range: row.try_get::<i64, _>("supports_range")? != 0,
        etag: row.try_get("etag")?,
        last_modified: row.try_get("last_modified")?,
        error_class: row.try_get("error_class")?,
        error_message: row.try_get("error_message")?,
        checksum_algorithm: row.try_get("checksum_algorithm")?,
        checksum_expected: row.try_get("checksum_expected")?,
        checksum_actual: row.try_get("checksum_actual")?,
        checksum_verified: row.try_get::<i64, _>("checksum_verified")? != 0,
        parent_job_id: row.try_get("parent_job_id")?,
    })
}

/// Persist the expected checksum (if the caller supplied one) at job
/// creation/start time, before the download runs.
pub async fn set_job_expected_checksum(
    pool: &SqlitePool,
    id: &str,
    algorithm: &str,
    expected_hex: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET checksum_algorithm = ?1, checksum_expected = ?2 WHERE id = ?3")
        .bind(algorithm)
        .bind(expected_hex)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Persist the actual (computed) checksum and whether it matched the
/// expected value, once verification has run post-download.
pub async fn set_job_checksum_result(
    pool: &SqlitePool,
    id: &str,
    algorithm: &str,
    actual_hex: &str,
    verified: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE jobs SET checksum_algorithm = ?1, checksum_actual = ?2, checksum_verified = ?3 WHERE id = ?4",
    )
    .bind(algorithm)
    .bind(actual_hex)
    .bind(verified as i64)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Find existing jobs that look like duplicates of a new request: same
/// source URL, same destination filename, or (if a checksum is already
/// known for the new request) same completed checksum. Used to drive
/// duplicate-detection policy (overwrite/rename/skip) before a new
/// download starts.
pub async fn find_duplicate_jobs(
    pool: &SqlitePool,
    url: &str,
    destination_filename: &str,
    checksum: Option<&str>,
) -> anyhow::Result<Vec<JobRecord>> {
    let like_pattern = format!("%/{destination_filename}");
    let rows = sqlx::query(
        "SELECT id, url, destination, status, job_kind, total_bytes, downloaded_bytes, connections,
                supports_range, etag, last_modified, error_class, error_message,
                checksum_algorithm, checksum_expected, checksum_actual, checksum_verified, parent_job_id
         FROM jobs
         WHERE url = ?1
            OR destination = ?2
            OR destination LIKE ?3
            OR (?4 IS NOT NULL AND checksum_actual = ?4)
         ORDER BY created_at DESC",
    )
    .bind(url)
    .bind(destination_filename)
    .bind(&like_pattern)
    .bind(checksum)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_job).collect()
}

/// Replace all segments for a job (used when (re)starting a download).
pub async fn replace_segments(
    pool: &SqlitePool,
    job_id: &str,
    segments: &[(i64, i64, i64)],
) -> anyhow::Result<Vec<SegmentRecord>> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM segments WHERE job_id = ?1")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;

    let now = Utc::now().to_rfc3339();
    let mut out = Vec::with_capacity(segments.len());
    for (seq, start, end) in segments.iter().copied() {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO segments (id, job_id, seq, start_byte, end_byte, downloaded_bytes, status, retry_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, 0, ?7)",
        )
        .bind(&id)
        .bind(job_id)
        .bind(seq)
        .bind(start)
        .bind(end)
        .bind(SegmentStatus::Pending.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        out.push(SegmentRecord {
            id,
            job_id: job_id.to_string(),
            seq,
            start_byte: start,
            end_byte: end,
            downloaded_bytes: 0,
            status: SegmentStatus::Pending,
            retry_count: 0,
            last_error_class: None,
        });
    }
    tx.commit().await?;
    Ok(out)
}

pub async fn get_segments(pool: &SqlitePool, job_id: &str) -> anyhow::Result<Vec<SegmentRecord>> {
    let rows = sqlx::query(
        "SELECT id, job_id, seq, start_byte, end_byte, downloaded_bytes, status, retry_count, last_error_class
         FROM segments WHERE job_id = ?1 ORDER BY seq ASC",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let status_str: String = row.try_get("status")?;
            Ok(SegmentRecord {
                id: row.try_get("id")?,
                job_id: row.try_get("job_id")?,
                seq: row.try_get("seq")?,
                start_byte: row.try_get("start_byte")?,
                end_byte: row.try_get("end_byte")?,
                downloaded_bytes: row.try_get("downloaded_bytes")?,
                status: status_str.parse()?,
                retry_count: row.try_get("retry_count")?,
                last_error_class: row.try_get("last_error_class")?,
            })
        })
        .collect()
}

/// Journal a segment's progress + status. Called on every state transition
/// (Sprint 3) so a crash mid-download leaves accurate on-disk state.
pub async fn update_segment(
    pool: &SqlitePool,
    id: &str,
    downloaded_bytes: i64,
    status: SegmentStatus,
    retry_count: i64,
    last_error_class: Option<&str>,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE segments SET downloaded_bytes = ?1, status = ?2, retry_count = ?3,
         last_error_class = ?4, updated_at = ?5 WHERE id = ?6",
    )
    .bind(downloaded_bytes)
    .bind(status.as_str())
    .bind(retry_count)
    .bind(last_error_class)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a single new segment row, used by the segment-stealing allocator
/// when it splits an in-progress segment into two.
pub async fn insert_segment(pool: &SqlitePool, seg: &SegmentRecord) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO segments (id, job_id, seq, start_byte, end_byte, downloaded_bytes, status, retry_count, last_error_class, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(&seg.id)
    .bind(&seg.job_id)
    .bind(seg.seq)
    .bind(seg.start_byte)
    .bind(seg.end_byte)
    .bind(seg.downloaded_bytes)
    .bind(seg.status.as_str())
    .bind(seg.retry_count)
    .bind(&seg.last_error_class)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Shrink a segment's end boundary in place (the "give away the back half"
/// side of segment stealing).
pub async fn shrink_segment_end(
    pool: &SqlitePool,
    id: &str,
    new_end_byte: i64,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE segments SET end_byte = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(new_end_byte)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// One fixed-size chunk's hash within a segment, recorded as it lands on
/// disk. Used by the corruption-repair pass to identify and re-fetch only
/// the byte range that's actually bad, instead of the whole file/segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRecord {
    pub id: String,
    pub job_id: String,
    pub seq: i64,
    pub start_byte: i64,
    pub end_byte: i64,
    pub crc32: i64,
}

/// Insert chunk-hash rows in bulk (one job typically has many chunks).
pub async fn replace_chunks(
    pool: &SqlitePool,
    job_id: &str,
    chunks: &[(i64, i64, i64, u32)],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM chunks WHERE job_id = ?1")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;

    for (seq, start, end, crc32) in chunks.iter().copied() {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO chunks (id, job_id, seq, start_byte, end_byte, crc32)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&id)
        .bind(job_id)
        .bind(seq)
        .bind(start)
        .bind(end)
        .bind(crc32 as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Update a single chunk's recorded hash after a targeted repair
/// re-download.
pub async fn update_chunk_crc32(pool: &SqlitePool, id: &str, crc32: u32) -> anyhow::Result<()> {
    sqlx::query("UPDATE chunks SET crc32 = ?1 WHERE id = ?2")
        .bind(crc32 as i64)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_chunks(pool: &SqlitePool, job_id: &str) -> anyhow::Result<Vec<ChunkRecord>> {
    let rows = sqlx::query(
        "SELECT id, job_id, seq, start_byte, end_byte, crc32 FROM chunks
         WHERE job_id = ?1 ORDER BY seq ASC",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(ChunkRecord {
                id: row.try_get("id")?,
                job_id: row.try_get("job_id")?,
                seq: row.try_get("seq")?,
                start_byte: row.try_get("start_byte")?,
                end_byte: row.try_get("end_byte")?,
                crc32: row.try_get("crc32")?,
            })
        })
        .collect()
}

/// One mirror URL for a job. `seq = 0` is the primary URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorRecord {
    pub id: String,
    pub job_id: String,
    pub url: String,
    pub seq: i64,
    pub latency_ms: Option<i64>,
    pub failure_count: i64,
}

/// Persist the mirror list for a job, in ranked order (index 0 = primary /
/// fastest at probe time).
pub async fn replace_mirrors(
    pool: &SqlitePool,
    job_id: &str,
    urls: &[String],
    latencies_ms: &[Option<i64>],
) -> anyhow::Result<Vec<MirrorRecord>> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM mirrors WHERE job_id = ?1")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;

    let mut out = Vec::with_capacity(urls.len());
    for (seq, url) in urls.iter().enumerate() {
        let id = uuid::Uuid::new_v4().to_string();
        let latency = latencies_ms.get(seq).copied().flatten();
        sqlx::query(
            "INSERT INTO mirrors (id, job_id, url, seq, latency_ms, failure_count)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        )
        .bind(&id)
        .bind(job_id)
        .bind(url)
        .bind(seq as i64)
        .bind(latency)
        .execute(&mut *tx)
        .await?;
        out.push(MirrorRecord {
            id,
            job_id: job_id.to_string(),
            url: url.clone(),
            seq: seq as i64,
            latency_ms: latency,
            failure_count: 0,
        });
    }
    tx.commit().await?;
    Ok(out)
}

pub async fn get_mirrors(pool: &SqlitePool, job_id: &str) -> anyhow::Result<Vec<MirrorRecord>> {
    let rows = sqlx::query(
        "SELECT id, job_id, url, seq, latency_ms, failure_count FROM mirrors
         WHERE job_id = ?1 ORDER BY seq ASC",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(MirrorRecord {
                id: row.try_get("id")?,
                job_id: row.try_get("job_id")?,
                url: row.try_get("url")?,
                seq: row.try_get("seq")?,
                latency_ms: row.try_get("latency_ms")?,
                failure_count: row.try_get("failure_count")?,
            })
        })
        .collect()
}

/// Delete a job and all its dependent rows (segments/chunks/mirrors cascade
/// via `ON DELETE CASCADE`). Used by the desktop app to let a person clear
/// completed/failed entries out of the queue view (Sprint 6).
pub async fn delete_job(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// List jobs whose `status` is one of the given values, most-recently
/// created first. Used on app startup to find jobs that were mid-flight
/// (Downloading/Probing/Verifying) when the process last exited without a
/// clean shutdown, so they can be automatically resumed (Sprint 6 session
/// restore).
pub async fn list_jobs_by_status(
    pool: &SqlitePool,
    statuses: &[JobStatus],
) -> anyhow::Result<Vec<JobRecord>> {
    let all = list_jobs(pool).await?;
    Ok(all
        .into_iter()
        .filter(|j| statuses.contains(&j.status))
        .collect())
}

/// Get one setting value by key (app-wide config: theme, download
/// directory, backup directory, bandwidth limits, etc. — see
/// `crates/engine::recovery` and the desktop app's settings commands).
pub async fn get_setting(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(match row {
        Some(row) => Some(row.try_get("value")?),
        None => None,
    })
}

/// Set (insert or overwrite) one setting value by key.
pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a setting by key. A no-op if the key doesn't exist.
pub async fn delete_setting(pool: &SqlitePool, key: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Fetch every setting as a flat map — used to hydrate the desktop app's
/// settings panel in one call.
pub async fn list_settings(pool: &SqlitePool) -> anyhow::Result<Vec<(String, String)>> {
    let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key ASC")
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("key")?, row.try_get("value")?)))
        .collect()
}

/// Record a failed attempt against a mirror (used to deprioritize
/// consistently-failing mirrors on subsequent segment retries).
pub async fn record_mirror_failure(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE mirrors SET failure_count = failure_count + 1, last_used_at = ?1 WHERE id = ?2",
    )
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_mirror_use(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE mirrors SET last_used_at = ?1 WHERE id = ?2")
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Torrent-specific metadata for a `job_kind = 'torrent'` job (Sprint 7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentMetaRecord {
    pub job_id: String,
    pub info_hash: String,
    pub magnet_uri: Option<String>,
    pub torrent_file_path: Option<String>,
    pub display_name: Option<String>,
    pub piece_count: Option<i64>,
    pub file_count: Option<i64>,
    pub peer_count: i64,
    pub sequential: bool,
    /// JSON-encoded array of selected file indices, or `None` for "all
    /// files" (mirrors `librqbit::AddTorrentOptions::only_files`).
    pub only_files: Option<String>,
}

/// Insert the torrent metadata row for a job, once at creation time —
/// mirrors [`set_job_expected_checksum`]'s "populate up front" pattern.
#[allow(clippy::too_many_arguments)]
pub async fn insert_torrent_meta(
    pool: &SqlitePool,
    job_id: &str,
    info_hash: &str,
    magnet_uri: Option<&str>,
    torrent_file_path: Option<&str>,
    display_name: Option<&str>,
    sequential: bool,
    only_files: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO torrent_meta (job_id, info_hash, magnet_uri, torrent_file_path, display_name, sequential, only_files)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(job_id)
    .bind(info_hash)
    .bind(magnet_uri)
    .bind(torrent_file_path)
    .bind(display_name)
    .bind(sequential as i64)
    .bind(only_files)
    .execute(pool)
    .await?;
    Ok(())
}

/// Refresh the swarm-derived fields (piece/file counts once metadata has
/// resolved from the swarm, and the current peer count) as the download
/// progresses.
pub async fn update_torrent_swarm_info(
    pool: &SqlitePool,
    job_id: &str,
    piece_count: Option<i64>,
    file_count: Option<i64>,
    peer_count: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE torrent_meta SET piece_count = ?1, file_count = ?2, peer_count = ?3 WHERE job_id = ?4",
    )
    .bind(piece_count)
    .bind(file_count)
    .bind(peer_count)
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_torrent_meta(
    pool: &SqlitePool,
    job_id: &str,
) -> anyhow::Result<Option<TorrentMetaRecord>> {
    let row = sqlx::query(
        "SELECT job_id, info_hash, magnet_uri, torrent_file_path, display_name,
                piece_count, file_count, peer_count, sequential, only_files
         FROM torrent_meta WHERE job_id = ?1",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(TorrentMetaRecord {
        job_id: row.try_get("job_id")?,
        info_hash: row.try_get("info_hash")?,
        magnet_uri: row.try_get("magnet_uri")?,
        torrent_file_path: row.try_get("torrent_file_path")?,
        display_name: row.try_get("display_name")?,
        piece_count: row.try_get("piece_count")?,
        file_count: row.try_get("file_count")?,
        peer_count: row.try_get("peer_count")?,
        sequential: row.try_get::<i64, _>("sequential")? != 0,
        only_files: row.try_get("only_files")?,
    }))
}

/// Look up a job by BitTorrent info-hash — the torrent analogue of
/// duplicate detection by URL (Sprint 4's `find_duplicate_by_hash`), so
/// re-adding the same magnet/`.torrent` doesn't start a second download.
pub async fn find_job_by_info_hash(
    pool: &SqlitePool,
    info_hash: &str,
) -> anyhow::Result<Option<JobRecord>> {
    let row = sqlx::query(
        "SELECT j.id, j.url, j.destination, j.status, j.job_kind, j.total_bytes, j.downloaded_bytes,
                j.connections, j.supports_range, j.etag, j.last_modified, j.error_class, j.error_message,
                j.checksum_algorithm, j.checksum_expected, j.checksum_actual, j.checksum_verified, j.parent_job_id
         FROM jobs j
         JOIN torrent_meta t ON t.job_id = j.id
         WHERE t.info_hash = ?1
         ORDER BY j.created_at DESC
         LIMIT 1",
    )
    .bind(info_hash)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(row_to_job(row)?))
}

/// Manifest-based job metadata (Sprint 9: HLS/DASH). Mirrors
/// [`TorrentMetaRecord`]'s "one row per job, populated up front /
/// refreshed as selection happens" pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestMetaRecord {
    pub job_id: String,
    pub manifest_kind: String,
    pub manifest_url: String,
    pub media_playlist_url: Option<String>,
    pub selected_variant: Option<String>,
    pub video_representation_id: Option<String>,
    pub audio_representation_id: Option<String>,
    pub is_live: bool,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_manifest_meta(
    pool: &SqlitePool,
    job_id: &str,
    manifest_kind: &str,
    manifest_url: &str,
    media_playlist_url: Option<&str>,
    selected_variant: Option<&str>,
    video_representation_id: Option<&str>,
    audio_representation_id: Option<&str>,
    is_live: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO manifest_meta (job_id, manifest_kind, manifest_url, media_playlist_url,
                                     selected_variant, video_representation_id, audio_representation_id, is_live)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(job_id)
    .bind(manifest_kind)
    .bind(manifest_url)
    .bind(media_playlist_url)
    .bind(selected_variant)
    .bind(video_representation_id)
    .bind(audio_representation_id)
    .bind(is_live as i64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_manifest_meta(
    pool: &SqlitePool,
    job_id: &str,
) -> anyhow::Result<Option<ManifestMetaRecord>> {
    let row = sqlx::query(
        "SELECT job_id, manifest_kind, manifest_url, media_playlist_url, selected_variant,
                video_representation_id, audio_representation_id, is_live
         FROM manifest_meta WHERE job_id = ?1",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(ManifestMetaRecord {
        job_id: row.try_get("job_id")?,
        manifest_kind: row.try_get("manifest_kind")?,
        manifest_url: row.try_get("manifest_url")?,
        media_playlist_url: row.try_get("media_playlist_url")?,
        selected_variant: row.try_get("selected_variant")?,
        video_representation_id: row.try_get("video_representation_id")?,
        audio_representation_id: row.try_get("audio_representation_id")?,
        is_live: row.try_get::<i64, _>("is_live")? != 0,
    }))
}

/// One resolved segment (init or media) for one track of a manifest-based
/// job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSegmentRecord {
    pub id: String,
    pub job_id: String,
    pub track: String,
    pub kind: String,
    pub seq: i64,
    pub url: String,
    pub downloaded: bool,
}

/// Replace every segment row for `(job_id, track)` with a freshly
/// resolved list — used at job-start time once the manifest has been
/// parsed and segment URLs resolved. Idempotent re-runs (e.g. resuming
/// after a manifest re-fetch found no changes) just rewrite the same
/// rows; `downloaded` flags for URLs that didn't change position are not
/// preserved across a full replace, matching [`replace_segments`]'s
/// (Sprint 2) same "authoritative re-plan" semantics — callers that want
/// to preserve completed-segment state across a resume should check
/// [`get_manifest_segments`] before calling this again.
pub async fn replace_manifest_segments(
    pool: &SqlitePool,
    job_id: &str,
    track: &str,
    segments: &[(String, i64, String)], // (kind, seq, url)
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM manifest_segments WHERE job_id = ?1 AND track = ?2")
        .bind(job_id)
        .bind(track)
        .execute(pool)
        .await?;

    for (kind, seq, url) in segments {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO manifest_segments (id, job_id, track, kind, seq, url, downloaded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        )
        .bind(&id)
        .bind(job_id)
        .bind(track)
        .bind(kind)
        .bind(seq)
        .bind(url)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn get_manifest_segments(
    pool: &SqlitePool,
    job_id: &str,
    track: &str,
) -> anyhow::Result<Vec<ManifestSegmentRecord>> {
    // 'media' > 'init' alphabetically, so ORDER BY kind DESC puts init first.
    let rows = sqlx::query(
        "SELECT id, job_id, track, kind, seq, url, downloaded
         FROM manifest_segments WHERE job_id = ?1 AND track = ?2
         ORDER BY kind DESC, seq ASC",
    )
    .bind(job_id)
    .bind(track)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(ManifestSegmentRecord {
                id: row.try_get("id")?,
                job_id: row.try_get("job_id")?,
                track: row.try_get("track")?,
                kind: row.try_get("kind")?,
                seq: row.try_get("seq")?,
                url: row.try_get("url")?,
                downloaded: row.try_get::<i64, _>("downloaded")? != 0,
            })
        })
        .collect()
}

pub async fn mark_manifest_segment_downloaded(
    pool: &SqlitePool,
    segment_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE manifest_segments SET downloaded = 1 WHERE id = ?1")
        .bind(segment_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Sprint 10: yt-dlp-backed job metadata — mirrors [`ManifestMetaRecord`]'s
/// "one row per job, populated once probing finishes" pattern. Chapters
/// and formats are stored as JSON (see the migration's comment) since
/// they're variable-shape, extractor-dependent lists read back as a unit
/// alongside the rest of the row rather than queried individually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetaRecord {
    pub job_id: String,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration_seconds: Option<f64>,
    /// Raw JSON, as produced by `serde_json::to_string` of
    /// `Vec<sdm_media::ChapterInfo>` — this crate deliberately doesn't
    /// depend on `sdm-media` to avoid a storage <-> media coupling, so
    /// callers (`crates/engine::media`) serialize/deserialize it.
    pub chapters_json: Option<String>,
    /// Raw JSON of `Vec<sdm_media::FormatInfo>`, same rationale.
    pub formats_json: Option<String>,
    pub is_live: bool,
    pub selected_format_id: Option<String>,
    /// Raw JSON of `Vec<String>` (subtitle language codes).
    pub subtitle_langs_json: Option<String>,
    pub embed_thumbnail: bool,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_media_meta(
    pool: &SqlitePool,
    job_id: &str,
    title: Option<&str>,
    thumbnail_url: Option<&str>,
    duration_seconds: Option<f64>,
    chapters_json: Option<&str>,
    formats_json: Option<&str>,
    is_live: bool,
    selected_format_id: Option<&str>,
    subtitle_langs_json: Option<&str>,
    embed_thumbnail: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO media_meta (job_id, title, thumbnail_url, duration_seconds, chapters_json,
                                  formats_json, is_live, selected_format_id, subtitle_langs_json, embed_thumbnail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(job_id) DO UPDATE SET
            title = excluded.title,
            thumbnail_url = excluded.thumbnail_url,
            duration_seconds = excluded.duration_seconds,
            chapters_json = excluded.chapters_json,
            formats_json = excluded.formats_json,
            is_live = excluded.is_live,
            selected_format_id = excluded.selected_format_id,
            subtitle_langs_json = excluded.subtitle_langs_json,
            embed_thumbnail = excluded.embed_thumbnail",
    )
    .bind(job_id)
    .bind(title)
    .bind(thumbnail_url)
    .bind(duration_seconds)
    .bind(chapters_json)
    .bind(formats_json)
    .bind(is_live as i64)
    .bind(selected_format_id)
    .bind(subtitle_langs_json)
    .bind(embed_thumbnail as i64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_media_meta(
    pool: &SqlitePool,
    job_id: &str,
) -> anyhow::Result<Option<MediaMetaRecord>> {
    let row = sqlx::query(
        "SELECT job_id, title, thumbnail_url, duration_seconds, chapters_json, formats_json,
                is_live, selected_format_id, subtitle_langs_json, embed_thumbnail
         FROM media_meta WHERE job_id = ?1",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(MediaMetaRecord {
        job_id: row.try_get("job_id")?,
        title: row.try_get("title")?,
        thumbnail_url: row.try_get("thumbnail_url")?,
        duration_seconds: row.try_get("duration_seconds")?,
        chapters_json: row.try_get("chapters_json")?,
        formats_json: row.try_get("formats_json")?,
        is_live: row.try_get::<i64, _>("is_live")? != 0,
        selected_format_id: row.try_get("selected_format_id")?,
        subtitle_langs_json: row.try_get("subtitle_langs_json")?,
        embed_thumbnail: row.try_get::<i64, _>("embed_thumbnail")? != 0,
    }))
}

/// One issued browser-extension pairing token (Sprint 11). `sdmd` mints
/// these on demand for the first-run pairing flow; the extension stores
/// whichever one the user confirms and sends it back as a bearer token on
/// every subsequent REST/WebSocket call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingTokenRecord {
    pub token: String,
    pub label: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
}

fn row_to_pairing_token(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<PairingTokenRecord> {
    Ok(PairingTokenRecord {
        token: row.try_get("token")?,
        label: row.try_get("label")?,
        created_at: row.try_get("created_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

/// Mint a brand-new pairing token. Callers generate the random token value
/// itself (see `sdm-server`'s use of a CSPRNG) — this just persists it.
pub async fn insert_pairing_token(
    pool: &SqlitePool,
    token: &str,
    label: &str,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO pairing_tokens (token, label) VALUES (?1, ?2)")
        .bind(token)
        .bind(label)
        .execute(pool)
        .await?;
    Ok(())
}

/// Look up a pairing token — `None` if it was never issued. Callers are
/// responsible for also checking `revoked_at.is_none()` before trusting it
/// (kept separate from the query so a revoked-but-present token can still
/// be told apart from one that never existed, e.g. for error messages).
pub async fn get_pairing_token(
    pool: &SqlitePool,
    token: &str,
) -> anyhow::Result<Option<PairingTokenRecord>> {
    let row = sqlx::query(
        "SELECT token, label, created_at, last_seen_at, revoked_at
         FROM pairing_tokens WHERE token = ?1",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    row.map(row_to_pairing_token).transpose()
}

/// Stamp a token's `last_seen_at` to now — called on every authenticated
/// extension request, so the desktop app's "Extension connected" status
/// indicator can tell a live extension from a stale, never-revisited one.
pub async fn touch_pairing_token(pool: &SqlitePool, token: &str) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE pairing_tokens SET last_seen_at = ?1 WHERE token = ?2")
        .bind(&now)
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

/// All non-revoked tokens, most recently created first — used to render
/// the desktop app's paired-extensions list.
pub async fn list_pairing_tokens(pool: &SqlitePool) -> anyhow::Result<Vec<PairingTokenRecord>> {
    let rows = sqlx::query(
        "SELECT token, label, created_at, last_seen_at, revoked_at
         FROM pairing_tokens WHERE revoked_at IS NULL ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(row_to_pairing_token).collect()
}

/// Revoke a token (soft-delete) so a lost/uninstalled extension can no
/// longer authenticate, without losing the audit trail of it having
/// existed.
pub async fn revoke_pairing_token(pool: &SqlitePool, token: &str) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE pairing_tokens SET revoked_at = ?1 WHERE token = ?2")
        .bind(&now)
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether any non-revoked token has been seen within `within_seconds` of
/// now — this is exactly the "Extension connected" boolean the desktop
/// app's status indicator polls (docs/SPRINT_PLAN_PHASE2.md Sprint 11).
pub async fn has_recent_pairing_activity(
    pool: &SqlitePool,
    within_seconds: i64,
) -> anyhow::Result<bool> {
    let tokens = list_pairing_tokens(pool).await?;
    let now = Utc::now();
    Ok(tokens.iter().any(|t| match &t.last_seen_at {
        Some(seen) => chrono::DateTime::parse_from_rfc3339(seen)
            .map(|dt| (now - dt.with_timezone(&Utc)).num_seconds() <= within_seconds)
            .unwrap_or(false),
        None => false,
    }))
}

/// One row of a search result — deliberately a separate, smaller type
/// from `JobRecord` rather than reusing it: `JobRecord`/`row_to_job` are
/// threaded through the engine, server routes, CLI, and desktop app
/// already, and widening that shared type for a search-only concern
/// (plus its FTS join) would touch every one of those call sites for no
/// benefit — search callers only need enough to render a result row and
/// look the job back up by id if the user acts on it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResultRecord {
    pub job_id: String,
    pub url: String,
    pub filename: String,
    pub category: Option<String>,
    pub status: JobStatus,
    pub job_kind: JobKind,
    pub created_at: String,
}

fn row_to_search_result(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<SearchResultRecord> {
    let status_str: String = row.try_get("status")?;
    let job_kind_str: String = row.try_get("job_kind")?;
    Ok(SearchResultRecord {
        job_id: row.try_get("id")?,
        url: row.try_get("url")?,
        filename: row.try_get("destination")?,
        category: row.try_get("category")?,
        status: status_str.parse()?,
        job_kind: job_kind_str.parse()?,
        created_at: row.try_get("created_at")?,
    })
}

/// Sprint 12 search filters — shared by `sdm search` (CLI), `GET /search`
/// (REST), and the desktop app's search bar, so all three surfaces stay
/// behaviorally identical by construction rather than by convention.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    /// Free-text query. In FTS mode this is passed to SQLite's FTS5
    /// `MATCH` (tokenized, so "report final" matches either token in any
    /// order); in regex mode it's compiled with the `regex` crate and
    /// applied in-process against filename/url/category/status.
    pub text: Option<String>,
    /// Sprint 12 DoD: `sdm search --regex`. FTS5 has no regex mode of its
    /// own (its tokenizer only supports token/prefix matching), so this
    /// path applies status/category/date filters as a SQL pushdown first
    /// (keeps it fast even on a large history) and then regex-matches
    /// only the surviving candidate rows in Rust.
    pub regex: bool,
    pub category: Option<String>,
    pub status: Option<JobStatus>,
    /// Inclusive, RFC3339 (matches how `created_at` is stored — see
    /// `insert_job_with_kind`).
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// Default 50 — a search box result list, not a full export; callers
    /// that want everything can pass a large explicit limit.
    pub limit: Option<i64>,
}

/// Sprint 12: `sdm search` / `GET /search` — full-text + filtered search
/// across download history and the active queue. See `SearchQuery` for
/// the filter surface and `docs/SPRINT_PLAN_PHASE2.md` Sprint 12's DoD
/// ("`sdm search --regex` against a seeded history of 100+ past downloads
/// returns correct filtered results in under 100ms").
pub async fn search_jobs(
    pool: &SqlitePool,
    query: &SearchQuery,
) -> anyhow::Result<Vec<SearchResultRecord>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 1000);

    if query.regex {
        return search_jobs_regex(pool, query, limit).await;
    }

    match &query.text {
        Some(text) if !text.trim().is_empty() => search_jobs_fts(pool, query, text, limit).await,
        _ => search_jobs_filtered_only(pool, query, limit).await,
    }
}

/// FTS5 path: `MATCH` against the free-text query, joined back to `jobs`
/// for the status/category/date filters and to materialize the result
/// row. `snippet`/`bm25` ranking isn't exposed to callers (yet) — plain
/// `rank` ordering (FTS5's default bm25) is enough for a search-box-sized
/// result list.
async fn search_jobs_fts(
    pool: &SqlitePool,
    query: &SearchQuery,
    text: &str,
    limit: i64,
) -> anyhow::Result<Vec<SearchResultRecord>> {
    // FTS5 query syntax gives `"`, `*`, `AND`/`OR`/`NOT`, column filters
    // (`col:`), etc. special meaning. A search box shouldn't require
    // users to know FTS5 syntax (or let a stray `"` throw a syntax
    // error), so each whitespace-separated term is quoted individually
    // and re-joined — this makes every term a literal phrase match,
    // AND'ed together (FTS5's implicit default), with no character in
    // the user's input able to change the query's structure.
    let sanitized = text
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    if sanitized.is_empty() {
        return search_jobs_filtered_only(pool, query, limit).await;
    }

    let rows = sqlx::query(
        "SELECT j.id, j.url, j.destination, j.category, j.status, j.job_kind, j.created_at
         FROM jobs_fts f
         JOIN jobs j ON j.id = f.job_id
         WHERE jobs_fts MATCH ?1
           AND (?2 IS NULL OR j.category = ?2)
           AND (?3 IS NULL OR j.status = ?3)
           AND (?4 IS NULL OR j.created_at >= ?4)
           AND (?5 IS NULL OR j.created_at <= ?5)
         ORDER BY rank
         LIMIT ?6",
    )
    .bind(&sanitized)
    .bind(&query.category)
    .bind(query.status.as_ref().map(JobStatus::as_str))
    .bind(&query.date_from)
    .bind(&query.date_to)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_search_result).collect()
}

/// No free-text term: a plain filtered listing (still useful — e.g.
/// "everything in category X from last week").
async fn search_jobs_filtered_only(
    pool: &SqlitePool,
    query: &SearchQuery,
    limit: i64,
) -> anyhow::Result<Vec<SearchResultRecord>> {
    let rows = sqlx::query(
        "SELECT id, url, destination, category, status, job_kind, created_at
         FROM jobs
         WHERE (?1 IS NULL OR category = ?1)
           AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR created_at >= ?3)
           AND (?4 IS NULL OR created_at <= ?4)
         ORDER BY created_at DESC
         LIMIT ?5",
    )
    .bind(&query.category)
    .bind(query.status.as_ref().map(JobStatus::as_str))
    .bind(&query.date_from)
    .bind(&query.date_to)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_search_result).collect()
}

/// Regex path: pushes the cheap structured filters (category/status/date
/// range) into SQL to keep the candidate set small, then applies the
/// user's regex in Rust against filename, URL, category, and status. A
/// full SQLite scan-and-filter like this is trivially fast at the queue/
/// history scale this app targets (thousands of rows, not millions) —
/// the Sprint 12 DoD's "100+ past downloads ... under 100ms" bar is
/// several orders of magnitude below where this would need optimizing
/// (e.g. an on-disk regex-compatible index) instead.
async fn search_jobs_regex(
    pool: &SqlitePool,
    query: &SearchQuery,
    limit: i64,
) -> anyhow::Result<Vec<SearchResultRecord>> {
    let Some(pattern) = query.text.as_deref().filter(|t| !t.trim().is_empty()) else {
        return search_jobs_filtered_only(pool, query, limit).await;
    };
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;

    // Pull every row matching the structured filters (no LIMIT yet — the
    // regex still needs to run over the full candidate set before we can
    // know which `limit` rows to keep), then filter + truncate in Rust.
    let rows = sqlx::query(
        "SELECT id, url, destination, category, status, job_kind, created_at
         FROM jobs
         WHERE (?1 IS NULL OR category = ?1)
           AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR created_at >= ?3)
           AND (?4 IS NULL OR created_at <= ?4)
         ORDER BY created_at DESC",
    )
    .bind(&query.category)
    .bind(query.status.as_ref().map(JobStatus::as_str))
    .bind(&query.date_from)
    .bind(&query.date_to)
    .fetch_all(pool)
    .await?;

    let mut results = Vec::new();
    for row in rows {
        let record = row_to_search_result(row)?;
        let haystack = format!(
            "{} {} {} {}",
            record.filename,
            record.url,
            record.category.as_deref().unwrap_or(""),
            record.status.as_str(),
        );
        if re.is_match(&haystack) {
            results.push(record);
            if results.len() as i64 >= limit {
                break;
            }
        }
    }
    Ok(results)
}

/// Set (or clear, with `None`) a job's category — the write side of
/// Sprint 12's `category` filter. Kept as its own function for the same
/// reason `set_job_status`/`set_job_error` are separate from the general
/// update helpers: one clear, narrow, testable responsibility.
pub async fn set_job_category(
    pool: &SqlitePool,
    id: &str,
    category: Option<&str>,
) -> anyhow::Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE jobs SET category = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(category)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_and_fetch_job_roundtrip() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();

        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.url, "https://example.com/file.zip");
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.downloaded_bytes, 0);
    }

    #[tokio::test]
    async fn probe_then_progress_then_complete() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();
        update_job_probe(&pool, "job-1", Some(1000), true, Some("\"abc\""), None, 4)
            .await
            .unwrap();

        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.total_bytes, Some(1000));
        assert!(job.supports_range);
        assert_eq!(job.connections, 4);
        assert_eq!(job.status, JobStatus::Downloading);

        update_job_downloaded_bytes(&pool, "job-1", 500)
            .await
            .unwrap();
        set_job_status(&pool, "job-1", JobStatus::Completed)
            .await
            .unwrap();

        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.downloaded_bytes, 500);
        assert_eq!(job.status, JobStatus::Completed);
    }

    #[tokio::test]
    async fn segments_round_trip_and_stealing_helpers() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();

        let segs = replace_segments(&pool, "job-1", &[(0, 0, 99), (1, 100, 199)])
            .await
            .unwrap();
        assert_eq!(segs.len(), 2);

        // Simulate segment stealing: shrink segment 1's end, insert a new segment 2.
        shrink_segment_end(&pool, &segs[1].id, 150).await.unwrap();
        let stolen = SegmentRecord {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: "job-1".to_string(),
            seq: 2,
            start_byte: 151,
            end_byte: 199,
            downloaded_bytes: 0,
            status: SegmentStatus::Pending,
            retry_count: 0,
            last_error_class: None,
        };
        insert_segment(&pool, &stolen).await.unwrap();

        let all = get_segments(&pool, "job-1").await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].end_byte, 150);
        assert_eq!(all[2].start_byte, 151);

        update_segment(&pool, &all[0].id, 100, SegmentStatus::Completed, 0, None)
            .await
            .unwrap();
        let refreshed = get_segments(&pool, "job-1").await.unwrap();
        assert_eq!(refreshed[0].status, SegmentStatus::Completed);
    }

    #[tokio::test]
    async fn job_error_is_journaled() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();
        set_job_error(&pool, "job-1", "dns_failure", "could not resolve host")
            .await
            .unwrap();

        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.error_class.as_deref(), Some("dns_failure"));
        assert_eq!(job.error_message.as_deref(), Some("could not resolve host"));
    }

    #[tokio::test]
    async fn checksum_expected_then_result_round_trip() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();

        set_job_expected_checksum(&pool, "job-1", "sha256", "deadbeef")
            .await
            .unwrap();
        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.checksum_algorithm.as_deref(), Some("sha256"));
        assert_eq!(job.checksum_expected.as_deref(), Some("deadbeef"));
        assert!(!job.checksum_verified);

        set_job_checksum_result(&pool, "job-1", "sha256", "deadbeef", true)
            .await
            .unwrap();
        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.checksum_actual.as_deref(), Some("deadbeef"));
        assert!(job.checksum_verified);
    }

    #[tokio::test]
    async fn chunks_round_trip_and_repair_update() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();

        replace_chunks(&pool, "job-1", &[(0, 0, 999, 111), (1, 1000, 1999, 222)])
            .await
            .unwrap();

        let chunks = get_chunks(&pool, "job-1").await.unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].crc32, 111);
        assert_eq!(chunks[1].crc32, 222);

        update_chunk_crc32(&pool, &chunks[1].id, 999).await.unwrap();
        let refreshed = get_chunks(&pool, "job-1").await.unwrap();
        assert_eq!(refreshed[1].crc32, 999);
    }

    #[tokio::test]
    async fn mirrors_round_trip_and_failure_tracking() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://mirror-a.example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();

        let urls = vec![
            "https://mirror-a.example.com/file.zip".to_string(),
            "https://mirror-b.example.com/file.zip".to_string(),
        ];
        let mirrors = replace_mirrors(&pool, "job-1", &urls, &[Some(50), Some(120)])
            .await
            .unwrap();
        assert_eq!(mirrors.len(), 2);
        assert_eq!(mirrors[0].seq, 0);
        assert_eq!(mirrors[0].latency_ms, Some(50));

        record_mirror_failure(&pool, &mirrors[0].id).await.unwrap();
        record_mirror_failure(&pool, &mirrors[0].id).await.unwrap();
        record_mirror_use(&pool, &mirrors[1].id).await.unwrap();

        let refreshed = get_mirrors(&pool, "job-1").await.unwrap();
        assert_eq!(refreshed[0].failure_count, 2);
        assert_eq!(refreshed[1].failure_count, 0);
    }

    #[tokio::test]
    async fn settings_round_trip_and_delete() {
        let pool = connect_in_memory().await.unwrap();
        assert_eq!(get_setting(&pool, "theme").await.unwrap(), None);

        set_setting(&pool, "theme", "dark").await.unwrap();
        assert_eq!(
            get_setting(&pool, "theme").await.unwrap(),
            Some("dark".to_string())
        );

        // Overwrite on conflict.
        set_setting(&pool, "theme", "light").await.unwrap();
        assert_eq!(
            get_setting(&pool, "theme").await.unwrap(),
            Some("light".to_string())
        );

        set_setting(&pool, "download_dir", "/home/user/Downloads")
            .await
            .unwrap();
        let all = list_settings(&pool).await.unwrap();
        assert_eq!(all.len(), 2);

        delete_setting(&pool, "theme").await.unwrap();
        assert_eq!(get_setting(&pool, "theme").await.unwrap(), None);
        assert_eq!(list_settings(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_job_removes_row_and_dependents() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/file.zip",
            "/tmp/file.zip",
        )
        .await
        .unwrap();
        replace_segments(&pool, "job-1", &[(0, 0, 99)])
            .await
            .unwrap();

        delete_job(&pool, "job-1").await.unwrap();
        assert!(get_job(&pool, "job-1").await.unwrap().is_none());
        assert!(get_segments(&pool, "job-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_jobs_by_status_filters_correctly() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(&pool, "job-1", "https://example.com/a.zip", "/tmp/a.zip")
            .await
            .unwrap();
        insert_job(&pool, "job-2", "https://example.com/b.zip", "/tmp/b.zip")
            .await
            .unwrap();
        insert_job(&pool, "job-3", "https://example.com/c.zip", "/tmp/c.zip")
            .await
            .unwrap();
        set_job_status(&pool, "job-2", JobStatus::Downloading)
            .await
            .unwrap();
        set_job_status(&pool, "job-3", JobStatus::Completed)
            .await
            .unwrap();

        let mid_flight = list_jobs_by_status(&pool, &[JobStatus::Downloading, JobStatus::Probing])
            .await
            .unwrap();
        assert_eq!(mid_flight.len(), 1);
        assert_eq!(mid_flight[0].id, "job-2");

        let queued = list_jobs_by_status(&pool, &[JobStatus::Queued])
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, "job-1");
    }

    #[tokio::test]
    async fn duplicate_detection_matches_by_url_filename_and_checksum() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(
            &pool,
            "job-1",
            "https://example.com/movie.mkv",
            "/downloads/movie.mkv",
        )
        .await
        .unwrap();
        set_job_checksum_result(&pool, "job-1", "sha256", "abc123", true)
            .await
            .unwrap();

        // Same URL.
        let by_url = find_duplicate_jobs(&pool, "https://example.com/movie.mkv", "movie.mkv", None)
            .await
            .unwrap();
        assert_eq!(by_url.len(), 1);

        // Different URL, same destination filename.
        let by_name = find_duplicate_jobs(
            &pool,
            "https://mirror.example.com/movie.mkv",
            "movie.mkv",
            None,
        )
        .await
        .unwrap();
        assert_eq!(by_name.len(), 1);

        // Different URL and filename, but matching checksum.
        let by_hash = find_duplicate_jobs(
            &pool,
            "https://another.example.com/renamed.mkv",
            "renamed.mkv",
            Some("abc123"),
        )
        .await
        .unwrap();
        assert_eq!(by_hash.len(), 1);

        // No overlap at all -> no duplicates.
        let none = find_duplicate_jobs(
            &pool,
            "https://another.example.com/other.mkv",
            "other.mkv",
            None,
        )
        .await
        .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn http_jobs_default_to_http_kind() {
        let pool = connect_in_memory().await.unwrap();
        insert_job(&pool, "job-1", "https://example.com/f.zip", "/tmp/f.zip")
            .await
            .unwrap();
        let job = get_job(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(job.job_kind, JobKind::Http);
    }

    #[tokio::test]
    async fn insert_job_with_kind_persists_ftp_and_torrent_kinds() {
        let pool = connect_in_memory().await.unwrap();
        insert_job_with_kind(
            &pool,
            "job-ftp",
            "ftp://example.com/f.zip",
            "/tmp/f.zip",
            JobKind::Ftp,
        )
        .await
        .unwrap();
        insert_job_with_kind(
            &pool,
            "job-torrent",
            "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "/tmp/downloads",
            JobKind::Torrent,
        )
        .await
        .unwrap();

        assert_eq!(
            get_job(&pool, "job-ftp").await.unwrap().unwrap().job_kind,
            JobKind::Ftp
        );
        assert_eq!(
            get_job(&pool, "job-torrent")
                .await
                .unwrap()
                .unwrap()
                .job_kind,
            JobKind::Torrent
        );
    }

    #[tokio::test]
    async fn torrent_meta_roundtrip_and_lookup_by_info_hash() {
        let pool = connect_in_memory().await.unwrap();
        let info_hash = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
        insert_job_with_kind(
            &pool,
            "job-1",
            "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a",
            "/downloads",
            JobKind::Torrent,
        )
        .await
        .unwrap();
        insert_torrent_meta(
            &pool,
            "job-1",
            info_hash,
            Some("magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a"),
            None,
            Some("ubuntu.iso"),
            true,
            None,
        )
        .await
        .unwrap();

        let meta = get_torrent_meta(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(meta.info_hash, info_hash);
        assert_eq!(meta.display_name.as_deref(), Some("ubuntu.iso"));
        assert!(meta.sequential);
        assert_eq!(meta.peer_count, 0);

        update_torrent_swarm_info(&pool, "job-1", Some(64), Some(1), 12)
            .await
            .unwrap();
        let meta = get_torrent_meta(&pool, "job-1").await.unwrap().unwrap();
        assert_eq!(meta.piece_count, Some(64));
        assert_eq!(meta.peer_count, 12);

        let found = find_job_by_info_hash(&pool, info_hash).await.unwrap();
        assert_eq!(found.unwrap().id, "job-1");

        let not_found = find_job_by_info_hash(&pool, "0000000000000000000000000000000000000000")
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn media_meta_round_trips_and_upserts() {
        let pool = connect_in_memory().await.unwrap();
        insert_job_with_kind(
            &pool,
            "job-media-1",
            "https://example.com/watch",
            "/downloads",
            JobKind::Media,
        )
        .await
        .unwrap();

        insert_media_meta(
            &pool,
            "job-media-1",
            Some("A Test Video"),
            Some("https://example.com/thumb.jpg"),
            Some(123.45),
            Some(r#"[{"title":"Intro","start_time":0.0,"end_time":10.0}]"#),
            Some(r#"[{"format_id":"137","height":1080}]"#),
            false,
            Some("137"),
            Some(r#"["en","fr"]"#),
            true,
        )
        .await
        .unwrap();

        let meta = get_media_meta(&pool, "job-media-1").await.unwrap().unwrap();
        assert_eq!(meta.title.as_deref(), Some("A Test Video"));
        assert_eq!(meta.selected_format_id.as_deref(), Some("137"));
        assert!(!meta.is_live);
        assert!(meta.embed_thumbnail);

        // Re-probing (e.g. a retry) should upsert, not fail on the
        // job_id primary key.
        insert_media_meta(
            &pool,
            "job-media-1",
            Some("A Test Video (Updated Title)"),
            None,
            Some(999.0),
            None,
            None,
            true,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        let updated = get_media_meta(&pool, "job-media-1").await.unwrap().unwrap();
        assert_eq!(
            updated.title.as_deref(),
            Some("A Test Video (Updated Title)")
        );
        assert!(updated.is_live);
        assert!(!updated.embed_thumbnail);
    }

    #[tokio::test]
    async fn playlist_children_link_back_to_their_parent_job() {
        let pool = connect_in_memory().await.unwrap();
        insert_job_with_kind(
            &pool,
            "parent-job",
            "https://example.com/playlist",
            "/downloads",
            JobKind::Media,
        )
        .await
        .unwrap();

        for i in 1..=3 {
            insert_child_job_with_kind(
                &pool,
                &format!("child-job-{i}"),
                &format!("https://example.com/watch?v={i}"),
                "/downloads",
                JobKind::Media,
                "parent-job",
            )
            .await
            .unwrap();
        }

        let children = list_child_jobs(&pool, "parent-job").await.unwrap();
        assert_eq!(children.len(), 3);
        for child in &children {
            assert_eq!(child.parent_job_id.as_deref(), Some("parent-job"));
        }

        let parent = get_job(&pool, "parent-job").await.unwrap().unwrap();
        assert_eq!(parent.parent_job_id, None);
    }

    #[tokio::test]
    async fn pairing_token_round_trips_and_reports_recent_activity() {
        let pool = connect_in_memory().await.unwrap();

        assert!(get_pairing_token(&pool, "tok-1").await.unwrap().is_none());
        assert!(!has_recent_pairing_activity(&pool, 60).await.unwrap());

        insert_pairing_token(&pool, "tok-1", "Chrome extension pairing")
            .await
            .unwrap();
        let fetched = get_pairing_token(&pool, "tok-1").await.unwrap().unwrap();
        assert_eq!(fetched.token, "tok-1");
        assert!(fetched.last_seen_at.is_none());
        assert!(fetched.revoked_at.is_none());

        assert!(!has_recent_pairing_activity(&pool, 60).await.unwrap());

        touch_pairing_token(&pool, "tok-1").await.unwrap();
        let touched = get_pairing_token(&pool, "tok-1").await.unwrap().unwrap();
        assert!(touched.last_seen_at.is_some());
        assert!(has_recent_pairing_activity(&pool, 60).await.unwrap());

        let listed = list_pairing_tokens(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);

        revoke_pairing_token(&pool, "tok-1").await.unwrap();
        let revoked = get_pairing_token(&pool, "tok-1").await.unwrap().unwrap();
        assert!(revoked.revoked_at.is_some());
        assert!(list_pairing_tokens(&pool).await.unwrap().is_empty());
        assert!(!has_recent_pairing_activity(&pool, 60).await.unwrap());
    }

    async fn seed_search_fixture(pool: &SqlitePool) {
        insert_job(
            pool,
            "job-report",
            "https://example.com/reports/final-report.pdf",
            "/downloads/final-report.pdf",
        )
        .await
        .unwrap();
        set_job_category(pool, "job-report", Some("documents"))
            .await
            .unwrap();

        insert_job(
            pool,
            "job-linux-iso",
            "https://example.com/os/ubuntu-24.04.iso",
            "/downloads/ubuntu-24.04.iso",
        )
        .await
        .unwrap();
        set_job_category(pool, "job-linux-iso", Some("isos"))
            .await
            .unwrap();
        set_job_status(pool, "job-linux-iso", JobStatus::Completed)
            .await
            .unwrap();

        insert_job(
            pool,
            "job-song",
            "https://example.com/music/summer-report-mix.mp3",
            "/downloads/summer-report-mix.mp3",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn search_fts_matches_filename_and_url_tokens() {
        let pool = connect_in_memory().await.unwrap();
        seed_search_fixture(&pool).await;

        let results = search_jobs(
            &pool,
            &SearchQuery {
                text: Some("report".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let ids: Vec<_> = results.iter().map(|r| r.job_id.as_str()).collect();
        assert!(ids.contains(&"job-report"));
        assert!(ids.contains(&"job-song")); // "summer-report-mix" contains the token too
        assert!(!ids.contains(&"job-linux-iso"));
    }

    #[tokio::test]
    async fn search_respects_category_and_status_filters() {
        let pool = connect_in_memory().await.unwrap();
        seed_search_fixture(&pool).await;

        let by_category = search_jobs(
            &pool,
            &SearchQuery {
                category: Some("isos".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_category.len(), 1);
        assert_eq!(by_category[0].job_id, "job-linux-iso");

        let by_status = search_jobs(
            &pool,
            &SearchQuery {
                status: Some(JobStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_status.len(), 1);
        assert_eq!(by_status[0].job_id, "job-linux-iso");
    }

    #[tokio::test]
    async fn search_regex_mode_matches_pattern_across_fields() {
        let pool = connect_in_memory().await.unwrap();
        seed_search_fixture(&pool).await;

        let results = search_jobs(
            &pool,
            &SearchQuery {
                text: Some(r"\.iso$".to_string()),
                regex: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].job_id, "job-linux-iso");

        // Invalid pattern surfaces as an error, not a silent empty result.
        let err = search_jobs(
            &pool,
            &SearchQuery {
                text: Some("(unclosed".to_string()),
                regex: true,
                ..Default::default()
            },
        )
        .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn search_with_no_text_falls_back_to_plain_filtered_listing() {
        let pool = connect_in_memory().await.unwrap();
        seed_search_fixture(&pool).await;

        let results = search_jobs(&pool, &SearchQuery::default()).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn search_fts_query_is_immune_to_syntax_injection() {
        let pool = connect_in_memory().await.unwrap();
        seed_search_fixture(&pool).await;

        // A raw string containing FTS5-meaningful characters must not
        // error out or be interpreted as query syntax — every term is
        // quoted as a literal phrase before being sent to MATCH.
        let results = search_jobs(
            &pool,
            &SearchQuery {
                text: Some("\"report OR *".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // No exact literal match for that malformed phrase against any
        // seeded row — the important assertion is that this didn't error.
        assert!(results.is_empty());
    }
}
