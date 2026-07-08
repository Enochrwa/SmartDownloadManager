use std::path::PathBuf;

use sdm_api_types::JobResponse;
use sdm_engine::error::EngineError;
use sdm_engine::{DownloadRequest, DuplicatePolicy, Engine, ExpectedChecksum};
use sdm_storage::SqlitePool;

use crate::state::ServerState;

/// Same URL -> filename fallback used by the CLI and desktop app
/// (`crates/cli/src/main.rs::default_destination`,
/// `apps/desktop/src-tauri/src/commands.rs::default_destination_name`),
/// kept here rather than imported since none of those crates expose it as
/// a library function.
pub fn default_destination_name(url: &str) -> String {
    let name = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let name = name.split(['?', '#']).next().unwrap_or(name);
    if name.is_empty() {
        "download".to_string()
    } else {
        name.to_string()
    }
}

pub struct ParsedCreateRequest {
    pub destination: PathBuf,
    pub expected_checksum: Option<ExpectedChecksum>,
    pub duplicate_policy: DuplicatePolicy,
}

/// Shared request-parsing for `POST /jobs` and `POST /capture` — both
/// ultimately build the same `sdm_engine::DownloadRequest`, just from
/// slightly different wire shapes. Connections is deliberately handled
/// by each caller directly: `/jobs` gets an optional numeric
/// `connections` field straight off the JSON body, while `/capture`
/// always defaults to `Auto` since the browser extension has no UI for
/// picking a connection count.
pub fn parse_create_request(
    state: &ServerState,
    url: &str,
    destination: Option<&str>,
    checksum: Option<&str>,
    on_duplicate: Option<&str>,
) -> anyhow::Result<ParsedCreateRequest> {
    let destination = match destination {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d),
        _ => state
            .default_download_dir
            .join(default_destination_name(url)),
    };
    let expected_checksum = checksum.map(ExpectedChecksum::parse).transpose()?;
    // Sprint 11 capture default: extension-originated downloads default to
    // "rename" too, same as the CLI/desktop default — an extension click
    // is a fresh user action, not a resume, so silently skipping would be
    // surprising.
    let duplicate_policy = DuplicatePolicy::parse(on_duplicate.unwrap_or("rename"))?;

    Ok(ParsedCreateRequest {
        destination,
        expected_checksum,
        duplicate_policy,
    })
}

/// `Some(existing job)` if `find_duplicate_jobs` (Sprint 4) already has a
/// row matching this URL/destination/checksum — used by `/capture` and
/// `/capture/batch` to avoid queuing the same download twice when a
/// clipboard poll or batch paste re-sees a URL that's already in the
/// queue or history, per Sprint 11's "deduplicated against the
/// existing-queue/history check from Sprint 4" scope note.
pub async fn find_existing_duplicate(
    pool: &SqlitePool,
    url: &str,
    destination: &std::path::Path,
    checksum: Option<&str>,
) -> anyhow::Result<Option<JobResponse>> {
    let filename = destination
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let matches = sdm_storage::find_duplicate_jobs(pool, url, &filename, checksum).await?;
    Ok(matches.into_iter().next().map(JobResponse::from))
}

pub async fn start_download(
    state: &std::sync::Arc<ServerState>,
    req: DownloadRequest,
) -> Result<(), EngineError> {
    let state = state.clone();
    crate::runner::spawn_job_runner(
        state,
        move |engine: std::sync::Arc<Engine>, tx| async move { engine.start_download(req, tx).await },
    );
    Ok(())
}
