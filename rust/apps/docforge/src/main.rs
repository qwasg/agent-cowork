use moonlit_sync::{SyncServer, SyncServerConfig};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // The embedded y-websocket server runs on a background Tokio runtime so the
    // GPUI event loop can own the main thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match runtime.block_on(async { SyncServer::listen(SyncServerConfig::default()).await }) {
        Ok(addr) => tracing::info!("embedded sync server listening on {addr}"),
        Err(err) => tracing::warn!("embedded sync server unavailable: {err}"),
    }
    // Expose the runtime handle so the GPUI app can spawn sync clients, then keep
    // the runtime (and its accept loop) alive for the process lifetime.
    let _ = moonlit_docforge::app::RUNTIME.set(runtime.handle().clone());
    std::mem::forget(runtime);

    moonlit_docforge::app::run();
    Ok(())
}
