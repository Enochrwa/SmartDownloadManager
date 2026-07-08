//! `GET /ws/progress` — every authenticated client (desktop queue view,
//! extension popup) gets its own broadcast subscription and receives
//! every `JobEvent` as a JSON text frame, live, as jobs progress. This is
//! the "live in the desktop app's queue" half of Sprint 11's DoD.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;

use crate::state::ServerState;

pub async fn ws_progress(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ServerState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<ServerState>) {
    let mut rx = state.progress_tx.subscribe();
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        let payload = match serde_json::to_string(&event) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!("failed to serialize JobEvent for WS: {e}");
                                continue;
                            }
                        };
                        if socket.send(Message::Text(payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "WS progress subscriber lagged; some events dropped, client should re-sync via GET /jobs");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            // If the client sends anything (including a Close frame) or
            // disconnects, `next()` resolving at all is enough signal to
            // stop — this endpoint is receive-only from the client's
            // perspective.
            msg = socket.recv() => {
                if msg.is_none() {
                    break;
                }
                if let Some(Ok(Message::Close(_))) = msg {
                    break;
                }
            }
        }
    }
}
