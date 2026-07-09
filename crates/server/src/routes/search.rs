//! `GET /search` — Sprint 12 full-text + filtered search across download
//! history and the active queue. Thin translation layer: parse the query
//! string into `sdm_storage::SearchQuery`, delegate to
//! `sdm_storage::search_jobs`, map results to the wire DTO. See
//! `docs/SPRINT_PLAN_PHASE2.md` Sprint 12 for the filter surface this is
//! required to expose (filename, URL, category, status, date range,
//! regex mode).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use sdm_api_types::{ErrorResponse, SearchRequest, SearchResultResponse};

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

pub async fn search(
    State(state): State<Arc<ServerState>>,
    Query(req): Query<SearchRequest>,
) -> axum::response::Response {
    let status = match req.status.as_deref() {
        Some(s) => match s.parse::<sdm_storage::JobStatus>() {
            Ok(status) => Some(status),
            Err(_) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("unknown status filter: {s}"),
                )
            }
        },
        None => None,
    };

    let query = sdm_storage::SearchQuery {
        text: req.text,
        regex: req.regex,
        category: req.category,
        status,
        date_from: req.date_from,
        date_to: req.date_to,
        limit: req.limit,
    };

    match sdm_storage::search_jobs(&state.pool, &query).await {
        Ok(results) => {
            let out: Vec<SearchResultResponse> = results
                .into_iter()
                .map(SearchResultResponse::from)
                .collect();
            Json(out).into_response()
        }
        // An invalid regex is a client error (bad input), not a server
        // fault — but `search_jobs` returns a plain `anyhow::Error` for
        // both cases (storage layer, deliberately, doesn't know about
        // HTTP status codes). Regex mode is the only failure path that
        // can originate from user input rather than the DB itself, so
        // it's the one case worth a text-based distinction here.
        Err(e) if req.regex && e.to_string().starts_with("invalid regex") => {
            err(StatusCode::BAD_REQUEST, e.to_string())
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
