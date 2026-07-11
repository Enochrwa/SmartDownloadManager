//! `POST /media/probe` — lets the desktop UI (and any other REST client)
//! show a real title/thumbnail/quality picker for a "capture any link"
//! media URL *before* committing to a download, the same information
//! `sdm download --via-ytdlp <url>` prints from `YtDlpClient::probe` on
//! the CLI side (`crates/cli/src/main.rs`'s `probe` subcommand). Doesn't
//! touch storage or start a job — purely a read-only yt-dlp metadata
//! fetch.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use sdm_api_types::{ErrorResponse, MediaFormatResponse, MediaProbeRequest, MediaProbeResponse};
use sdm_media::{YtDlpBinary, YtDlpClient};

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

pub async fn probe(
    State(_state): State<Arc<ServerState>>,
    Json(req): Json<MediaProbeRequest>,
) -> axum::response::Response {
    if req.url.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "url must not be empty");
    }

    let client = YtDlpClient::new(YtDlpBinary::default());

    // A playlist URL still resolves to one flat entry per video via
    // `probe_playlist` (see `crates/engine::media`'s module docs on why
    // "more than one entry" is the actual playlist signal) — cheap
    // enough to check first so the UI can show "N videos" instead of
    // trying (and failing, since `probe` wants exactly one item) to
    // fully resolve every entry's formats up front.
    let entries = match client.probe_playlist(&req.url).await {
        Ok(e) => e,
        Err(e) => return err(StatusCode::BAD_REQUEST, e.to_string()),
    };

    if entries.len() > 1 {
        return Json(MediaProbeResponse {
            title: Some(format!("Playlist ({} videos)", entries.len())),
            thumbnail: None,
            duration_seconds: None,
            is_livestream: false,
            is_playlist: true,
            formats: Vec::new(),
        })
        .into_response();
    }

    match client.probe(&req.url).await {
        Ok(meta) => {
            let formats = meta
                .formats
                .iter()
                .map(|f| MediaFormatResponse {
                    format_id: f.format_id.clone(),
                    ext: f.ext.clone(),
                    height: f.height,
                    width: f.width,
                    vcodec: f.vcodec.clone(),
                    acodec: f.acodec.clone(),
                    filesize_bytes: f.filesize.or(f.filesize_approx),
                    tbr: f.tbr,
                    quality_label: f.quality_label(),
                })
                .collect();
            Json(MediaProbeResponse {
                title: meta.title.clone(),
                thumbnail: meta.thumbnail.clone(),
                duration_seconds: meta.duration,
                is_livestream: meta.is_livestream(),
                is_playlist: false,
                formats,
            })
            .into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}
