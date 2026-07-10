//! Sprint 12: `POST /auth/cookies` — the browser-extension cookie-import
//! endpoint. The Sprint 11 extension already has a paired, authenticated
//! channel to `sdmd` (the same `Authorization: Bearer <pairing token>`
//! every other `protected` route requires — see `crate::lib`); this
//! route lets it hand over a page's session cookies so `sdm download`/the
//! desktop app can reuse that logged-in session without the user pasting
//! a `Cookie:` header by hand.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use sdm_api_types::ErrorResponse;

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
pub struct ImportCookiesRequest {
    /// The domain these cookies apply to, e.g. "example.com" — matched
    /// against a download URL's host the same way
    /// `sdm_storage::auth::resolve_auth_config` does for a manually-saved
    /// cookie.
    pub domain: String,
    /// A raw `Cookie:` header value, e.g.
    /// "sessionid=abc123; csrftoken=xyz" — the extension is expected to
    /// have already joined the individual cookies it read via
    /// `chrome.cookies` into this single header format.
    pub cookie: String,
}

#[derive(serde::Serialize)]
pub struct ImportCookiesResponse {
    pub ok: bool,
}

pub async fn import_cookies(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<ImportCookiesRequest>,
) -> axum::response::Response {
    let domain = req.domain.trim();
    if domain.is_empty() {
        return err(StatusCode::BAD_REQUEST, "domain must not be empty");
    }
    if req.cookie.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "cookie must not be empty");
    }

    let store = sdm_storage::CredentialStore::new(state.pool.clone());
    let scope = sdm_storage::auth::AuthScope::Domain(domain.to_string());
    let mut cfg = match sdm_storage::auth::get_auth_config(&state.pool, &store, &scope).await {
        Ok(existing) => existing.unwrap_or_default(),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    cfg.cookie = Some(req.cookie.clone());

    match sdm_storage::auth::set_auth_config(&state.pool, &store, &scope, &cfg).await {
        Ok(()) => Json(ImportCookiesResponse { ok: true }).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
