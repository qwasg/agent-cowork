use moonlit_agent_ide::app;
use moonlit_api::{MoonlitAgentApi, DEFAULT_API_BASE};
use moonlit_core::store::keys;
use moonlit_core::ConfigStore;
use moonlit_sync::{SyncServer, SyncServerConfig};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let store = ConfigStore::open_for_app("MoonlitAgentIde")?;
    let api_base = store.get_string_or(keys::API_BASE_URL, DEFAULT_API_BASE);
    let api = MoonlitAgentApi::new(api_base);

    // Background Tokio runtime for all networking (REST + event WS). GPUI owns
    // the main thread, so we hand its handle to the app via a `OnceLock`.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let _ = app::RUNTIME.set(rt.handle().clone());

    // Embedded y-websocket server backing the in-app DocForge editor tab. The
    // DocForge view spawns its sync clients via `moonlit_docforge::app::RUNTIME`,
    // so share the same Tokio handle here.
    match rt.block_on(async { SyncServer::listen(SyncServerConfig::default()).await }) {
        Ok(addr) => tracing::info!("embedded sync server listening on {addr}"),
        Err(err) => tracing::warn!("embedded sync server unavailable: {err}"),
    }
    let _ = moonlit_docforge::app::RUNTIME.set(rt.handle().clone());

    std::mem::forget(rt);

    app::run(api);
    Ok(())
}
