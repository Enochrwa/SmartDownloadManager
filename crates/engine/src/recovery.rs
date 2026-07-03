//! Sprint 6: recovery and hardening.
//!
//! - Corrupted-database repair: `PRAGMA integrity_check`, and if that fails,
//!   fall back to the most recent snapshot backup (or a fresh empty
//!   database if none exists) rather than refusing to start.
//! - Orphaned temp-file cleanup: files sitting in a person's download
//!   directories that don't correspond to any known job (left behind by a
//!   deleted job row, a wiped database, or a crash before the job row was
//!   written).
//! - Automatic settings/queue backups: `VACUUM INTO` snapshots on a
//!   schedule, with old snapshots pruned.
//! - Session restore: on launch, find jobs that were `Downloading`,
//!   `Probing`, or `Verifying` when the process last exited — since only a
//!   live process updates those states, an interrupted one means the app
//!   quit (or crashed) mid-download — so the desktop app can resume them
//!   automatically.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sdm_storage::{JobRecord, JobStatus, SqlitePool};

use crate::error::EngineError;

/// How many timestamped backup snapshots to keep before pruning the
/// oldest. Chosen to cover roughly a day of hourly backups without
/// growing the backup directory unboundedly.
pub const DEFAULT_BACKUP_RETENTION: usize = 24;

/// Result of a database integrity check + (if needed) repair attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    /// `PRAGMA integrity_check` problems found, if any. Empty means the
    /// database was healthy and no repair action was taken.
    pub integrity_errors: Vec<String>,
    /// If a repair was performed, what it did.
    pub action: RepairAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairAction {
    /// The database was healthy; nothing was done.
    NoneNeeded,
    /// The database was corrupt and was replaced with the named backup
    /// snapshot. The corrupt file is preserved alongside the restored one
    /// with a `.corrupt` suffix, in case manual recovery is ever needed.
    RestoredFromBackup(PathBuf),
    /// The database was corrupt and no usable backup snapshot existed, so
    /// a fresh empty (migrated) database was created in its place. All
    /// queue/job history was lost, but the app can start.
    RecreatedEmpty,
}

/// Run `PRAGMA integrity_check` against an already-open pool and return the
/// list of problems reported (empty = healthy).
pub async fn check_integrity(pool: &SqlitePool) -> Result<Vec<String>, EngineError> {
    let rows: Vec<(String,)> = sqlx::query_as("PRAGMA integrity_check")
        .fetch_all(pool)
        .await
        .map_err(|e| EngineError::Storage(e.into()))?;

    Ok(rows
        .into_iter()
        .map(|(msg,)| msg)
        .filter(|msg| msg != "ok")
        .collect())
}

/// Snapshot the database to a timestamped file in `backup_dir` using
/// SQLite's `VACUUM INTO`, which produces a consistent, compacted copy in
/// one atomic step (safe to run against a live database). Prunes old
/// snapshots beyond `retention`.
pub async fn backup_database(
    pool: &SqlitePool,
    backup_dir: &Path,
    retention: usize,
) -> Result<PathBuf, EngineError> {
    tokio::fs::create_dir_all(backup_dir).await?;

    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let backup_path = backup_dir.join(format!("jobs-{timestamp}.db"));
    // VACUUM INTO refuses to overwrite an existing file, but our filename
    // already carries millisecond precision, so collisions are not a
    // practical concern.
    let backup_path_str = backup_path.to_string_lossy().to_string();

    sqlx::query("VACUUM INTO ?1")
        .bind(&backup_path_str)
        .execute(pool)
        .await
        .map_err(|e| EngineError::Storage(e.into()))?;

    prune_old_backups(backup_dir, retention).await?;
    Ok(backup_path)
}

/// Delete the oldest backup snapshots beyond `retention`, keeping the most
/// recent ones (sorted by filename, which is timestamp-prefixed so
/// lexicographic order matches chronological order).
async fn prune_old_backups(backup_dir: &Path, retention: usize) -> Result<(), EngineError> {
    let mut backups = list_backups(backup_dir)?;
    if backups.len() <= retention {
        return Ok(());
    }
    backups.sort();
    let excess = backups.len() - retention;
    for path in &backups[..excess] {
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(())
}

/// List all backup snapshot files in `backup_dir` (oldest-to-newest by
/// filename, which is timestamp-prefixed).
pub fn list_backups(backup_dir: &Path) -> Result<Vec<PathBuf>, EngineError> {
    if !backup_dir.exists() {
        return Ok(vec![]);
    }
    let mut backups: Vec<PathBuf> = std::fs::read_dir(backup_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("jobs-") && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();
    backups.sort();
    Ok(backups)
}

/// The most recent backup snapshot in `backup_dir`, if any.
pub fn find_latest_backup(backup_dir: &Path) -> Result<Option<PathBuf>, EngineError> {
    Ok(list_backups(backup_dir)?.into_iter().next_back())
}

/// Check the database at `db_path` for corruption and repair it in place
/// if needed: restore the most recent backup snapshot, or (if none
/// exists) create a fresh empty database. Safe to call on every app
/// launch — it's a no-op when the database is healthy.
///
/// `db_path` and `backup_dir` are plain filesystem paths (not sqlite
/// connection URLs).
pub async fn repair_database_if_corrupt(
    db_path: &Path,
    backup_dir: &Path,
) -> Result<RepairReport, EngineError> {
    // A missing database is not corruption — it just means this is a
    // fresh install. `sdm_storage::connect` will create + migrate it.
    if !db_path.exists() {
        return Ok(RepairReport {
            integrity_errors: vec![],
            action: RepairAction::NoneNeeded,
        });
    }

    let integrity_errors = {
        let url = format!("sqlite://{}?mode=rwc", db_path.to_string_lossy());
        match sqlx::SqlitePool::connect(&url).await {
            Ok(pool) => {
                let result = check_integrity(&pool).await;
                pool.close().await;
                match result {
                    Ok(errors) => errors,
                    // A query failure here (e.g. "file is not a database")
                    // is itself unambiguous evidence of corruption, not a
                    // reason to bail out of the repair routine.
                    Err(e) => vec![format!("integrity check failed: {e}")],
                }
            }
            // If we can't even open it, treat that as maximally corrupt.
            Err(e) => vec![format!("failed to open database: {e}")],
        }
    };

    if integrity_errors.is_empty() {
        return Ok(RepairReport {
            integrity_errors,
            action: RepairAction::NoneNeeded,
        });
    }

    // Preserve the corrupt file for forensics/manual recovery, then
    // replace it.
    let corrupt_aside = db_path.with_extension("db.corrupt");
    let _ = tokio::fs::remove_file(&corrupt_aside).await;
    tokio::fs::rename(db_path, &corrupt_aside).await?;

    let action = match find_latest_backup(backup_dir)? {
        Some(backup) => {
            tokio::fs::copy(&backup, db_path).await?;
            RepairAction::RestoredFromBackup(backup)
        }
        None => {
            // No backup to fall back on: start clean rather than refusing
            // to launch at all.
            sdm_storage::connect(&db_path.to_string_lossy())
                .await
                .map_err(EngineError::from)?;
            RepairAction::RecreatedEmpty
        }
    };

    Ok(RepairReport {
        integrity_errors,
        action,
    })
}

/// Scan `scan_dirs` (non-recursive) for regular files that don't match
/// any known job's `destination` path. These are "orphaned" artifacts:
/// leftovers from a job whose database row was lost (deleted job, wiped
/// or repaired database) rather than part of any tracked download.
///
/// Returns the list of orphan paths found. Pass `delete = true` to remove
/// them; otherwise this is a dry run that only reports candidates, which
/// is the safer default for anything wired up to a UI button.
pub async fn find_orphaned_files(
    pool: &SqlitePool,
    scan_dirs: &[PathBuf],
    delete: bool,
) -> Result<Vec<PathBuf>, EngineError> {
    let jobs = sdm_storage::list_jobs(pool)
        .await
        .map_err(EngineError::from)?;
    let known: HashSet<PathBuf> = jobs.iter().map(|j| PathBuf::from(&j.destination)).collect();

    let mut orphans = Vec::new();
    for dir in scan_dirs {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            if !known.contains(&path) {
                orphans.push(path);
            }
        }
    }

    if delete {
        for path in &orphans {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    Ok(orphans)
}

/// Jobs that were mid-flight (`Downloading`, `Probing`, or `Verifying`)
/// when the process last exited. A live engine keeps these states current
/// only while it's actually running the job, so finding one at startup
/// means the previous run ended abnormally (crash, force-quit, power
/// loss) rather than completing or being explicitly paused.
///
/// The desktop app calls this once on launch and resumes each returned
/// job automatically, matching the Sprint 6 DoD ("quit mid-download,
/// relaunch, and see it resume automatically").
pub async fn restore_session(pool: &SqlitePool) -> Result<Vec<JobRecord>, EngineError> {
    sdm_storage::list_jobs_by_status(
        pool,
        &[
            JobStatus::Downloading,
            JobStatus::Probing,
            JobStatus::Verifying,
        ],
    )
    .await
    .map_err(EngineError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdm_storage::connect;

    async fn db_at(dir: &Path, name: &str) -> (SqlitePool, PathBuf) {
        let path = dir.join(name);
        let pool = connect(&path.to_string_lossy()).await.unwrap();
        (pool, path)
    }

    #[tokio::test]
    async fn healthy_database_reports_no_integrity_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, _path) = db_at(tmp.path(), "jobs.db").await;
        let errors = check_integrity(&pool).await.unwrap();
        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn backup_then_restore_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, db_path) = db_at(tmp.path(), "jobs.db").await;
        sdm_storage::insert_job(&pool, "job-1", "https://example.com/a.zip", "/tmp/a.zip")
            .await
            .unwrap();

        let backup_dir = tmp.path().join("backups");
        let backup_path = backup_database(&pool, &backup_dir, DEFAULT_BACKUP_RETENTION)
            .await
            .unwrap();
        assert!(backup_path.exists());

        // Corrupt the live database by truncating it, then repair.
        pool.close().await;
        tokio::fs::write(&db_path, b"not a sqlite file")
            .await
            .unwrap();

        let report = repair_database_if_corrupt(&db_path, &backup_dir)
            .await
            .unwrap();
        assert!(!report.integrity_errors.is_empty());
        assert!(matches!(report.action, RepairAction::RestoredFromBackup(_)));

        // The restored database should have the job we backed up.
        let restored_pool = connect(&db_path.to_string_lossy()).await.unwrap();
        let job = sdm_storage::get_job(&restored_pool, "job-1").await.unwrap();
        assert!(job.is_some());

        // The corrupt file should have been preserved alongside it.
        assert!(db_path.with_extension("db.corrupt").exists());
    }

    #[tokio::test]
    async fn repair_with_no_backup_recreates_empty_database() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, db_path) = db_at(tmp.path(), "jobs.db").await;
        pool.close().await;
        tokio::fs::write(&db_path, b"garbage").await.unwrap();

        let backup_dir = tmp.path().join("backups"); // never created, no backups exist
        let report = repair_database_if_corrupt(&db_path, &backup_dir)
            .await
            .unwrap();
        assert!(!report.integrity_errors.is_empty());
        assert_eq!(report.action, RepairAction::RecreatedEmpty);

        // The fresh database should be openable and empty.
        let fresh_pool = connect(&db_path.to_string_lossy()).await.unwrap();
        let jobs = sdm_storage::list_jobs(&fresh_pool).await.unwrap();
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn missing_database_is_not_treated_as_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("does-not-exist.db");
        let backup_dir = tmp.path().join("backups");
        let report = repair_database_if_corrupt(&db_path, &backup_dir)
            .await
            .unwrap();
        assert!(report.integrity_errors.is_empty());
        assert_eq!(report.action, RepairAction::NoneNeeded);
    }

    #[tokio::test]
    async fn backup_pruning_keeps_only_the_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, _path) = db_at(tmp.path(), "jobs.db").await;
        let backup_dir = tmp.path().join("backups");

        for _ in 0..5 {
            backup_database(&pool, &backup_dir, 3).await.unwrap();
            // Ensure distinct millisecond timestamps between snapshots.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let remaining = list_backups(&backup_dir).unwrap();
        assert_eq!(remaining.len(), 3);
    }

    #[tokio::test]
    async fn find_orphaned_files_ignores_known_destinations() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, _path) = db_at(tmp.path(), "jobs.db").await;

        let downloads = tmp.path().join("downloads");
        tokio::fs::create_dir_all(&downloads).await.unwrap();
        let known_file = downloads.join("known.zip");
        let orphan_file = downloads.join("orphan.zip");
        tokio::fs::write(&known_file, b"data").await.unwrap();
        tokio::fs::write(&orphan_file, b"data").await.unwrap();

        sdm_storage::insert_job(
            &pool,
            "job-1",
            "https://example.com/known.zip",
            &known_file.to_string_lossy(),
        )
        .await
        .unwrap();

        let orphans = find_orphaned_files(&pool, std::slice::from_ref(&downloads), false)
            .await
            .unwrap();
        assert_eq!(orphans, vec![orphan_file.clone()]);
        // Dry run: nothing actually deleted.
        assert!(orphan_file.exists());

        let deleted = find_orphaned_files(&pool, &[downloads], true)
            .await
            .unwrap();
        assert_eq!(deleted, vec![orphan_file.clone()]);
        assert!(!orphan_file.exists());
        assert!(known_file.exists());
    }

    #[tokio::test]
    async fn restore_session_finds_only_mid_flight_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let (pool, _path) = db_at(tmp.path(), "jobs.db").await;

        sdm_storage::insert_job(&pool, "job-1", "https://example.com/a.zip", "/tmp/a.zip")
            .await
            .unwrap();
        sdm_storage::insert_job(&pool, "job-2", "https://example.com/b.zip", "/tmp/b.zip")
            .await
            .unwrap();
        sdm_storage::insert_job(&pool, "job-3", "https://example.com/c.zip", "/tmp/c.zip")
            .await
            .unwrap();
        sdm_storage::set_job_status(&pool, "job-1", JobStatus::Downloading)
            .await
            .unwrap();
        sdm_storage::set_job_status(&pool, "job-2", JobStatus::Completed)
            .await
            .unwrap();
        // job-3 stays Queued (never started).

        let mid_flight = restore_session(&pool).await.unwrap();
        assert_eq!(mid_flight.len(), 1);
        assert_eq!(mid_flight[0].id, "job-1");
    }
}
