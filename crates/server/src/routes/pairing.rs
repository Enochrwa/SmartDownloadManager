//! Sprint 11 pairing flow: `sdmd` mints a local pairing token the desktop
//! app displays on first run; the user enters it into the browser
//! extension's options page, which calls `POST /pairing/verify` to
//! confirm it and then stores it for every future request's
//! `Authorization: Bearer` header. `GET /pairing/status` is what the
//! desktop app's "Extension connected" indicator polls.
//!
//! `POST /pairing/tokens` and `POST /pairing/verify` are intentionally
//! reachable without an existing token (that's the whole point of a
//! first-run flow) — see `crate::lib` for exactly which routes bypass
//! the `require_pairing_token` middleware and why that's safe (loopback-
//! only by default; see the module doc there).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use sdm_api_types::{
    ErrorResponse, PairedExtensionInfo, PairingStatusResponse, PairingTokenIssueResponse,
    PairingVerifyRequest, PairingVerifyResponse,
};

use crate::state::ServerState;

fn err(status: StatusCode, message: impl Into<String>) -> axum::response::Response {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct IssueTokenRequest {
    /// Human-readable label shown in the desktop app's paired-extensions
    /// list, e.g. "Chrome on Just's laptop". Defaults to a timestamp if
    /// omitted.
    pub label: Option<String>,
}

/// Mint a brand-new pairing token. Called by the desktop app's first-run
/// pairing UI (which then displays the token/QR code to the user) — not
/// by the extension itself, which only ever *verifies* a token a human
/// typed or scanned in.
pub async fn issue_token(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<IssueTokenRequest>,
) -> axum::response::Response {
    let token = generate_token();
    let label = req
        .label
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| format!("Pairing {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")));

    match sdm_storage::insert_pairing_token(&state.pool, &token, &label).await {
        Ok(()) => {
            let created_at = chrono::Utc::now().to_rfc3339();
            (
                StatusCode::CREATED,
                Json(PairingTokenIssueResponse {
                    token,
                    label,
                    created_at,
                }),
            )
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// The extension calls this once, right after the user pastes in the
/// token the desktop app showed them, to confirm it's valid before
/// storing it for future use.
pub async fn verify_token(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<PairingVerifyRequest>,
) -> axum::response::Response {
    match sdm_storage::get_pairing_token(&state.pool, &req.token).await {
        Ok(Some(record)) if record.revoked_at.is_none() => {
            let _ = sdm_storage::touch_pairing_token(&state.pool, &req.token).await;
            Json(PairingVerifyResponse { ok: true }).into_response()
        }
        Ok(_) => Json(PairingVerifyResponse { ok: false }).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Polled by the desktop app's "Extension connected" status indicator.
pub async fn status(State(state): State<Arc<ServerState>>) -> axum::response::Response {
    let connected =
        match sdm_storage::has_recent_pairing_activity(&state.pool, state.connected_window_secs)
            .await
        {
            Ok(c) => c,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
    let tokens = match sdm_storage::list_pairing_tokens(&state.pool).await {
        Ok(t) => t,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let paired_extensions = tokens
        .into_iter()
        .map(|t| PairedExtensionInfo {
            label: t.label,
            created_at: t.created_at,
            last_seen_at: t.last_seen_at,
        })
        .collect();

    Json(PairingStatusResponse {
        connected,
        paired_extensions,
    })
    .into_response()
}

#[derive(serde::Deserialize)]
pub struct RevokeTokenRequest {
    pub token: String,
}

/// Revoke a paired extension — e.g. from the desktop app's settings, if
/// the user uninstalled the extension or wants to re-pair from scratch.
pub async fn revoke_token(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<RevokeTokenRequest>,
) -> axum::response::Response {
    match sdm_storage::revoke_pairing_token(&state.pool, &req.token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// 32 random bytes, hex-encoded — plenty of entropy for a bearer token
/// that's only ever transmitted over loopback HTTP and stored locally by
/// the extension, no different in kind from e.g. a Jupyter notebook
/// server's token-based auth.
fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
