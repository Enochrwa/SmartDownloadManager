//! Shared DTOs between sdm-server (the daemon) and sdm-cli / future SDKs.
//! No business logic lives here — just serde types, kept in lockstep with
//! packages/common-types on the TypeScript side and, since Sprint 11, with
//! `extensions/shared`'s hand-written TS mirror of the capture/pairing
//! shapes (the browser extension has no Cargo access, so that copy is kept
//! in sync by hand — see `extensions/shared/src/api-types.ts`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJobRequest {
    pub url: String,
    pub destination: Option<String>,
    pub connections: Option<u8>,
    /// Additional mirror URLs serving the same content (Sprint 4 mirror
    /// support, reachable over the network API since Sprint 11).
    #[serde(default)]
    pub mirrors: Vec<String>,
    /// "algorithm:hex", e.g. "sha256:abcd...".
    pub checksum: Option<String>,
    /// One of "overwrite" | "rename" | "skip" (default "rename").
    pub on_duplicate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub status: String,
    pub job_kind: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub connections: u32,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
    pub parent_job_id: Option<String>,
}

impl From<sdm_storage::JobRecord> for JobResponse {
    fn from(r: sdm_storage::JobRecord) -> Self {
        JobResponse {
            id: r.id,
            url: r.url,
            destination: r.destination,
            status: r.status.as_str().to_string(),
            job_kind: r.job_kind.as_str().to_string(),
            downloaded_bytes: r.downloaded_bytes as u64,
            total_bytes: r.total_bytes.map(|v| v as u64),
            connections: r.connections as u32,
            error_class: r.error_class,
            error_message: r.error_message,
            parent_job_id: r.parent_job_id,
        }
    }
}

/// A live progress event, broadcast to every authenticated WebSocket
/// client subscribed on `/ws/progress` (Sprint 11 — this is what lets the
/// desktop app's "Extension connected" queue view and the browser
/// extension's popup both watch the same download update live).
/// Mirrors `sdm_engine::ProgressEvent` field-for-field, but as a
/// self-contained, stable-over-the-wire DTO rather than the engine's
/// internal type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JobEvent {
    Probing {
        job_id: String,
    },
    Started {
        job_id: String,
        total_bytes: Option<u64>,
        connections: u32,
    },
    Progress {
        job_id: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        speed_bps: f64,
    },
    Verifying {
        job_id: String,
    },
    Retrying {
        job_id: String,
        error_class: String,
        attempt: u32,
        delay_ms: u64,
    },
    Completed {
        job_id: String,
        destination: String,
        total_bytes: Option<u64>,
    },
    Failed {
        job_id: String,
        error_class: String,
        message: String,
    },
}

/// Sprint 11: a browser-extension "capture" — a URL the extension noticed
/// (via download interception, a context-menu click, a clipboard-detected
/// link, a drag-and-drop, or a batch paste) and wants handed to the
/// engine. Distinct from `CreateJobRequest` so the extension can pass
/// browser-only context (the page it came from, a size hint the browser
/// already knows from the `Content-Length` it intercepted) without the
/// CLI/desktop paths needing to care about those fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub url: String,
    /// The page the link/download was found on, if any — stored for
    /// context but not currently interpreted by the engine.
    pub page_url: Option<String>,
    /// Filename the browser suggested (from a `Content-Disposition`
    /// header or the URL path), used as a naming hint only; the engine's
    /// own naming/duplicate-detection logic (Sprint 3/4) still decides
    /// the final destination.
    pub suggested_filename: Option<String>,
    /// Byte size the browser already observed (e.g. from
    /// `chrome.downloads.DownloadItem.fileSize`), used only for the
    /// size-threshold interception decision on the extension side — not
    /// trusted as authoritative by the engine, which still probes the URL
    /// itself.
    pub size_hint_bytes: Option<u64>,
    /// One of "download-intercept" | "context-menu" | "clipboard" |
    /// "drag-drop" | "batch-paste" — informational, surfaced in the
    /// desktop queue view so the user can see where a capture came from.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureResponse {
    pub job: JobResponse,
    /// True if this capture matched an existing queued/completed job
    /// (Sprint 4 duplicate detection) and no new job was started.
    pub deduplicated: bool,
}

/// One batch entry's outcome, for `POST /capture/batch` (pasting a block
/// of text containing multiple URLs into the extension popup).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCaptureResult {
    pub url: String,
    pub job: Option<JobResponse>,
    pub deduplicated: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCaptureRequest {
    pub urls: Vec<String>,
    pub page_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCaptureResponse {
    pub results: Vec<BatchCaptureResult>,
}

/// Returned by `POST /pairing/tokens` (first-run pairing flow): a
/// freshly-minted token the user copies from the desktop app into the
/// extension's options page (or that the extension exchanges directly if
/// paired via a one-click local flow — see `docs/SPRINT_PLAN_PHASE2.md`
/// Sprint 11's pairing-token note).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingTokenIssueResponse {
    pub token: String,
    pub label: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingVerifyRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingVerifyResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedExtensionInfo {
    pub label: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

/// Polled by the desktop app's "Extension connected" status indicator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingStatusResponse {
    pub connected: bool,
    pub paired_extensions: Vec<PairedExtensionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Sprint 12 `GET /search` — mirrors `sdm_storage::SearchQuery` field for
/// field but as a stable wire type (query-string deserializable, since
/// this is a GET), and `SearchResultResponse` mirrors
/// `sdm_storage::SearchResultRecord`. Kept separate from the storage
/// types for the same reason `JobResponse` is separate from `JobRecord`
/// — storage internals (typed `JobStatus`/`JobKind` enums, `anyhow`
/// errors) shouldn't leak across the wire.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchRequest {
    pub text: Option<String>,
    #[serde(default)]
    pub regex: bool,
    pub category: Option<String>,
    pub status: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultResponse {
    pub job_id: String,
    pub url: String,
    pub filename: String,
    pub category: Option<String>,
    pub status: String,
    pub job_kind: String,
    pub created_at: String,
}

impl From<sdm_storage::SearchResultRecord> for SearchResultResponse {
    fn from(r: sdm_storage::SearchResultRecord) -> Self {
        SearchResultResponse {
            job_id: r.job_id,
            url: r.url,
            filename: r.filename,
            category: r.category,
            status: r.status.as_str().to_string(),
            job_kind: r.job_kind.as_str().to_string(),
            created_at: r.created_at,
        }
    }
}
