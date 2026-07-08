//! Bearer-token authentication for every endpoint the browser extension
//! calls (per docs/SPRINT_PLAN_PHASE2.md Sprint 11: "extension talks
//! exclusively to `sdmd` over the existing REST/WebSocket API" — and only
//! after completing the pairing flow). `/health` and the pairing-token
//! issuance endpoint itself are deliberately left unauthenticated (see
//! `routes::pairing`); everything else requires a valid, non-revoked
//! token minted by that flow.

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::net::SocketAddr;
use std::sync::Arc;

use sdm_api_types::ErrorResponse;

use crate::state::ServerState;

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}

/// Minting a pairing token (and revoking one) must never be reachable
/// from anything but the machine `sdmd` is running on — otherwise anyone
/// on the same network segment could call `POST /pairing/tokens` and
/// pair their own "extension" against a stranger's daemon. The pairing
/// *verify* and *status* endpoints stay reachable from wherever the
/// extension's background script runs (which is loopback anyway, in
/// practice), but token issuance and revocation are gated on the peer
/// address here.
pub async fn require_loopback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if addr.ip().is_loopback() {
        next.run(request).await
    } else {
        unauthorized("this endpoint is only reachable from localhost")
    }
}

/// Extracts `Authorization: Bearer <token>`, validates it against
/// `pairing_tokens`, rejects revoked/unknown tokens, and — on success —
/// stamps `last_seen_at` so the desktop app's connected-status indicator
/// reflects this request.
pub async fn require_pairing_token(
    State(state): State<Arc<ServerState>>,
    request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    let Some(token) = token else {
        return unauthorized("missing Authorization: Bearer <token> header");
    };

    match sdm_storage::get_pairing_token(&state.pool, token).await {
        Ok(Some(record)) if record.revoked_at.is_none() => {
            if let Err(e) = sdm_storage::touch_pairing_token(&state.pool, token).await {
                tracing::warn!("failed to update pairing token last_seen_at: {e}");
            }
            next.run(request).await
        }
        Ok(Some(_)) => unauthorized("pairing token has been revoked"),
        Ok(None) => unauthorized("unknown pairing token"),
        Err(e) => {
            tracing::error!("pairing token lookup failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal error validating pairing token".to_string(),
                }),
            )
                .into_response()
        }
    }
}
