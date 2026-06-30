//! Shared DTOs between sdm-server (the daemon) and sdm-cli / future SDKs.
//! No business logic lives here — just serde types, kept in lockstep with
//! packages/common-types on the TypeScript side.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJobRequest {
    pub url: String,
    pub destination: Option<String>,
    pub connections: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: String,
    pub url: String,
    pub status: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}
