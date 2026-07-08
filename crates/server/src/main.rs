//! `sdmd` — the SmartDownloadManager daemon binary.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("SDM_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7890);

    let app = sdm_server::router_from_env().await?;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(
        %port,
        "sdmd listening (REST + WebSocket API for sdm-cli, remote clients, and the Sprint 11 browser extension pairing flow)"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
