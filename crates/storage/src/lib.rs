//! sdm-storage: SQLite-backed persistence for jobs, segments, and settings.
//!
//! Sprint 1: schema + migrations, basic job CRUD.
//! Sprint 2: segment rows (one per connection).
//! Sprint 3: segment-state journaling — every status transition for a
//! segment or job is written through to SQLite immediately, so a crash at
//! any point leaves a recoverable, consistent on-disk state.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub status: JobStatus,
    pub total_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    pub connections: i64,
    pub supports_range: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
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
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO jobs (id, url, destination, status, downloaded_bytes, connections, supports_range, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, 1, 0, ?5, ?5)",
    )
    .bind(id)
    .bind(url)
    .bind(destination)
    .bind(JobStatus::Queued.as_str())
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Persist probe results (HEAD request): content length, range support,
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
        "SELECT id, url, destination, status, total_bytes, downloaded_bytes, connections,
                supports_range, etag, last_modified, error_class, error_message
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
        "SELECT id, url, destination, status, total_bytes, downloaded_bytes, connections,
                supports_range, etag, last_modified, error_class, error_message
         FROM jobs ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_job).collect()
}

fn row_to_job(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<JobRecord> {
    let status_str: String = row.try_get("status")?;
    Ok(JobRecord {
        id: row.try_get("id")?,
        url: row.try_get("url")?,
        destination: row.try_get("destination")?,
        status: status_str.parse()?,
        total_bytes: row.try_get("total_bytes")?,
        downloaded_bytes: row.try_get("downloaded_bytes")?,
        connections: row.try_get("connections")?,
        supports_range: row.try_get::<i64, _>("supports_range")? != 0,
        etag: row.try_get("etag")?,
        last_modified: row.try_get("last_modified")?,
        error_class: row.try_get("error_class")?,
        error_message: row.try_get("error_message")?,
    })
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
}
