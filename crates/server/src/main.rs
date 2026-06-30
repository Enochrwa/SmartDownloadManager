//! `sdmd` — the SmartDownloadManager daemon binary.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("SDM_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7890);

    let app = sdm_server::router();
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(%port, "sdmd listening (engine wiring lands Sprint 5, see docs/SPRINT_PLAN.md)");
    axum::serve(listener, app).await?;
    Ok(())
}
