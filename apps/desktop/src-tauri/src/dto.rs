//! DTOs sent across the Tauri IPC boundary to the React frontend. Kept as
//! plain serde-friendly structs, hand-in-sync with
//! `packages/common-types/src/index.ts` (see that file's header comment).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDto {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub status: String,
    pub total_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    pub connections: i64,
    pub checksum_algorithm: Option<String>,
    pub checksum_actual: Option<String>,
    pub checksum_verified: bool,
    pub error_message: Option<String>,
    /// "http" | "ftp" | "torrent" | "sftp" | "scp" | "webdav" | "hls" |
    /// "dash" | "media" — lets the queue UI show a distinct icon/badge
    /// for "capture any link" (yt-dlp) jobs vs. every other transport.
    pub job_kind: String,
    pub parent_job_id: Option<String>,
    /// Populated for `job_kind == "media"` jobs from the `media_meta`
    /// side table — see `list_jobs`'s enrichment step in `commands.rs`.
    pub media_title: Option<String>,
    pub media_thumbnail: Option<String>,
}

impl From<sdm_storage::JobRecord> for JobDto {
    fn from(r: sdm_storage::JobRecord) -> Self {
        JobDto {
            id: r.id,
            url: r.url,
            destination: r.destination,
            status: r.status.as_str().to_string(),
            total_bytes: r.total_bytes,
            downloaded_bytes: r.downloaded_bytes,
            connections: r.connections,
            checksum_algorithm: r.checksum_algorithm,
            checksum_actual: r.checksum_actual,
            checksum_verified: r.checksum_verified,
            error_message: r.error_message,
            job_kind: r.job_kind.as_str().to_string(),
            parent_job_id: r.parent_job_id,
            media_title: None,
            media_thumbnail: None,
        }
    }
}

/// One selectable quality/codec format from a `probe_media` call — backs
/// the Add Download dialog's quality picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFormatDto {
    pub format_id: String,
    pub quality_label: String,
    pub ext: Option<String>,
    pub has_video: bool,
    pub has_audio: bool,
    pub filesize_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProbeDto {
    pub title: Option<String>,
    pub thumbnail: Option<String>,
    pub duration_seconds: Option<f64>,
    pub is_livestream: bool,
    pub is_playlist: bool,
    pub formats: Vec<MediaFormatDto>,
}

/// Streamed to the frontend on the `job-event` Tauri event channel as a
/// job runs. Mirrors `sdm_engine::ProgressEvent` but flattened to one
/// tagged enum that's easy to switch on in TypeScript, and with a
/// server-computed instantaneous speed added to `Progress`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum JobEventDto {
    Queued {
        job_id: String,
    },
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
        total_bytes: u64,
    },
    Failed {
        job_id: String,
        error_class: String,
        message: String,
    },
    Paused {
        job_id: String,
    },
}

/// Sprint 11: the browser-extension pairing flow, surfaced in the
/// desktop app's settings panel as a first-run pairing card and an
/// "Extension connected" status indicator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingTokenDto {
    pub token: String,
    pub label: String,
    pub created_at: String,
}

impl From<sdm_api_types::PairingTokenIssueResponse> for PairingTokenDto {
    fn from(r: sdm_api_types::PairingTokenIssueResponse) -> Self {
        PairingTokenDto {
            token: r.token,
            label: r.label,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedExtensionDto {
    pub label: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStatusDto {
    pub connected: bool,
    pub paired_extensions: Vec<PairedExtensionDto>,
    /// The port the embedded extension API is *actually* listening on
    /// right now — `None` if it hasn't finished starting yet or failed to
    /// bind (see `api_error`). Previously this always reported the
    /// configured port regardless of whether the bind succeeded, which is
    /// what caused the "Couldn't reach sdmd at this address" bug: the
    /// panel would confidently show `http://127.0.0.1:7890` even when
    /// nothing was listening there.
    pub api_port: Option<u16>,
    /// Set when the embedded server failed to bind (after retries/
    /// fallback-port-scan) or hasn't started yet, so the UI can show a
    /// real diagnostic instead of a silently-wrong address.
    pub api_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairReportDto {
    pub integrity_errors: Vec<String>,
    pub action: String,
    pub detail: Option<String>,
}

impl From<sdm_engine::RepairReport> for RepairReportDto {
    fn from(r: sdm_engine::RepairReport) -> Self {
        let (action, detail) = match r.action {
            sdm_engine::RepairAction::NoneNeeded => ("none_needed".to_string(), None),
            sdm_engine::RepairAction::RestoredFromBackup(p) => (
                "restored_from_backup".to_string(),
                Some(p.to_string_lossy().to_string()),
            ),
            sdm_engine::RepairAction::RecreatedEmpty => ("recreated_empty".to_string(), None),
        };
        RepairReportDto {
            integrity_errors: r.integrity_errors,
            action,
            detail,
        }
    }
}
