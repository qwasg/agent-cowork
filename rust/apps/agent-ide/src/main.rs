use moonlit_agent_ide::app;
use moonlit_api::{MoonlitAgentApi, DEFAULT_API_BASE};
use moonlit_core::store::keys;
use moonlit_core::ConfigStore;

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
    std::mem::forget(rt);

    app::run(api);
    Ok(())
}
