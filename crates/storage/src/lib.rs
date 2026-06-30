//! sdm-storage: SQLite-backed persistence for jobs, segments, settings,
//! and history. See migrations/ for schema (added in Sprint 1).

pub async fn connect(db_path: &str) -> anyhow::Result<sqlx::SqlitePool> {
    let url = format!("sqlite://{db_path}?mode=rwc");
    let pool = sqlx::SqlitePool::connect(&url).await?;
    Ok(pool)
}
