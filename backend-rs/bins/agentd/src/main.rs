//! agentd — Rust core service for the Agent Debug backend.
//!
//! Serves the full `/api/agent-debug/*` REST surface + SSE event stream on an
//! internal port. The Go edge gateway terminates client HTTP/WS/SSE, handles
//! auth/CORS, and proxies to this service.

mod api;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;

use api::AppServices;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = agent_config::Config::load();
    // The Rust core listens on the configured port + 1 by default (the Go edge
    // gateway owns the public port 8002). Override with AGENT_CORE_PORT.
    let core_port: u16 = std::env::var("AGENT_CORE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cfg.http_port + 1);
    let host = cfg.host.clone();

    let app_services = AppServices::build(cfg)?;
    tracing::info!("providers: {:?}", app_services.providers.provider_names());

    let router = api::routes::router(app_services.clone());
    let addr: SocketAddr = format!("{host}:{core_port}").parse()?;
    tracing::info!("agentd (Rust core) listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(app_services.clone()))
        .await?;

    // Final safety net: make sure queued JSONL writes hit disk before exit.
    app_services.bus.flush(std::time::Duration::from_secs(2));
    tracing::info!("agentd shut down cleanly");
    Ok(())
}

/// Resolves on SIGINT (Ctrl-C) or SIGTERM; cancels active runs so in-flight
/// HTTP requests (long agent loops) can finish quickly and the server drains.
async fn shutdown_signal(app: std::sync::Arc<AppServices>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; cancelling active runs");
    app.runtime.cancel_all();
    app.bus.flush(std::time::Duration::from_secs(2));
}
