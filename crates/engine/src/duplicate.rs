//! Duplicate detection (Sprint 4).
//!
//! Before starting a new download, check whether it looks like a duplicate
//! of something already in the queue/history: same source URL, same
//! destination filename, or (if we already know the expected checksum)
//! same content hash. The caller decides what to do about it via
//! [`DuplicatePolicy`] — overwrite the existing file, rename the new one
//! (the Sprint 3 auto-rename path), or skip the new download entirely.

use std::path::Path;

use sdm_storage::{JobRecord, SqlitePool};

use crate::error::EngineError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicatePolicy {
    /// Overwrite the existing destination file; don't auto-rename.
    Overwrite,
    /// Default: keep the existing file, auto-rename the new download
    /// ("file (1).zip") — this is the Sprint 3 naming behavior.
    #[default]
    Rename,
    /// Don't start the new download at all.
    Skip,
}

impl DuplicatePolicy {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "overwrite" => DuplicatePolicy::Overwrite,
            "rename" => DuplicatePolicy::Rename,
            "skip" => DuplicatePolicy::Skip,
            other => anyhow::bail!(
                "unknown duplicate policy: {other} (expected one of overwrite, rename, skip)"
            ),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateReason {
    SameUrl,
    SameFilename,
    SameChecksum,
}

#[derive(Debug, Clone)]
pub struct DuplicateMatch {
    pub job: JobRecord,
    pub reason: DuplicateReason,
}

fn classify(job: &JobRecord, url: &str, filename: &str, checksum: Option<&str>) -> DuplicateReason {
    if job.url == url {
        return DuplicateReason::SameUrl;
    }
    if let Some(sum) = checksum {
        if job.checksum_actual.as_deref() == Some(sum) {
            return DuplicateReason::SameChecksum;
        }
    }
    let job_filename = Path::new(&job.destination)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(&job.destination);
    if job_filename == filename {
        return DuplicateReason::SameFilename;
    }
    // Shouldn't normally happen (storage only returns matches for one of
    // the above reasons) but default sensibly if it does.
    DuplicateReason::SameFilename
}

/// Look for existing jobs that look like duplicates of a prospective new
/// download.
pub async fn find_duplicates(
    pool: &SqlitePool,
    url: &str,
    destination: &Path,
    expected_checksum: Option<&str>,
) -> Result<Vec<DuplicateMatch>, EngineError> {
    let filename = destination
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("download");

    let matches = sdm_storage::find_duplicate_jobs(pool, url, filename, expected_checksum).await?;
    Ok(matches
        .into_iter()
        .map(|job| {
            let reason = classify(&job, url, filename, expected_checksum);
            DuplicateMatch { job, reason }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finds_duplicate_by_url() {
        let pool = sdm_storage::connect_in_memory().await.unwrap();
        sdm_storage::insert_job(
            &pool,
            "job-1",
            "https://example.com/movie.mkv",
            "/downloads/movie.mkv",
        )
        .await
        .unwrap();

        let matches = find_duplicates(
            &pool,
            "https://example.com/movie.mkv",
            Path::new("/downloads/movie.mkv"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].reason, DuplicateReason::SameUrl);
    }

    #[tokio::test]
    async fn no_duplicates_for_a_genuinely_new_download() {
        let pool = sdm_storage::connect_in_memory().await.unwrap();
        sdm_storage::insert_job(
            &pool,
            "job-1",
            "https://example.com/movie.mkv",
            "/downloads/movie.mkv",
        )
        .await
        .unwrap();

        let matches = find_duplicates(
            &pool,
            "https://example.com/other.mkv",
            Path::new("/downloads/other.mkv"),
            None,
        )
        .await
        .unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn duplicate_policy_parses_known_values() {
        assert_eq!(
            DuplicatePolicy::parse("overwrite").unwrap(),
            DuplicatePolicy::Overwrite
        );
        assert_eq!(
            DuplicatePolicy::parse("SKIP").unwrap(),
            DuplicatePolicy::Skip
        );
        assert!(DuplicatePolicy::parse("nonsense").is_err());
    }
}
