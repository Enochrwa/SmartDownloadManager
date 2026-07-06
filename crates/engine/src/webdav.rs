//! WebDAV job orchestration (Sprint 8).
//!
//! WebDAV's `GET`+`Range` semantics are identical to plain HTTP/HTTPS —
//! there's no separate wire behavior to implement — so this module is
//! deliberately thin: translate the `webdav://`/`webdavs://` URL to
//! `http://`/`https://`, insert the job row ourselves (tagged
//! `JobKind::WebDav` so `sdm status`/`sdm list` show the protocol the
//! user actually asked for), and hand off to
//! `crate::download::Engine::run` — the same segmented,
//! segment-stealing, mirror-aware, checksum-verifying core Sprint 1-6
//! built for HTTP. See `docs/SPRINT_PLAN_PHASE2.md` Sprint 8: "WebDAV
//! reuses the Sprint 1-2 range-request and segment-splitting logic
//! almost unchanged."
//!
//! The one thing that *is* WebDAV-specific — `PROPFIND` directory
//! listing and `PUT` upload — lives in `sdm_protocols::webdav` and is
//! used directly by callers (e.g. `crates/cli`) rather than wrapped here,
//! since neither is a "download" in the `Job`/progress-event sense.

use std::path::PathBuf;

use sdm_storage::JobKind;

use crate::download::Engine;
use crate::error::EngineError;
use crate::job::Job;
use crate::mirrors::MirrorSet;
use crate::naming::unique_destination;
use crate::progress::ProgressSender;
use crate::segment::ConnectionsOption;
use crate::verify::ExpectedChecksum;

pub struct WebDavDownloadRequest {
    pub url: String,
    pub destination: PathBuf,
    pub connections: ConnectionsOption,
    pub expected_checksum: Option<ExpectedChecksum>,
}

pub struct WebDavEngine<'a> {
    engine: &'a Engine,
}

impl<'a> WebDavEngine<'a> {
    pub fn new(engine: &'a Engine) -> Self {
        Self { engine }
    }

    pub async fn start_download(
        &self,
        req: WebDavDownloadRequest,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        let http_url =
            sdm_protocols::webdav::to_http_url(&req.url).map_err(EngineError::Protocol)?;

        let destination = unique_destination(&req.destination);
        let job_id = uuid::Uuid::new_v4().to_string();
        let dest_str = destination.to_string_lossy().to_string();
        let pool = self.engine.pool();
        sdm_storage::insert_job_with_kind(pool, &job_id, &http_url, &dest_str, JobKind::WebDav)
            .await?;

        if let Some(expected) = &req.expected_checksum {
            sdm_storage::set_job_expected_checksum(
                pool,
                &job_id,
                expected.algorithm.as_str(),
                &expected.hex,
            )
            .await?;
        }

        let mirror_set = MirrorSet::new(vec![http_url]);
        self.engine
            .run(
                job_id,
                mirror_set,
                destination,
                req.connections,
                false,
                progress,
                req.expected_checksum,
            )
            .await
    }

    /// Resume works through the same generic `Engine::resume_download`
    /// every HTTP job uses — the stored URL is already the translated
    /// `http(s)://` form, so there's nothing WebDAV-specific left to do
    /// once the job row exists. Exposed here anyway (rather than telling
    /// callers to reach for `Engine::resume_download` directly) so
    /// `crates/cli` has one consistent "engine per job kind" shape to
    /// dispatch through.
    pub async fn resume_download(
        &self,
        job_id: &str,
        progress: ProgressSender,
    ) -> Result<Job, EngineError> {
        self.engine.resume_download(job_id, progress).await
    }
}
