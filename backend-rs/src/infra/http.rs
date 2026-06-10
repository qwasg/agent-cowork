//! Shared `reqwest` clients (connection pooling + sane timeouts) so tools and
//! gateway helpers don't build a fresh client per call.

use std::sync::OnceLock;
use std::time::Duration;

/// General-purpose client with connect timeout; per-request timeouts are set
/// by callers via `RequestBuilder::timeout`.
pub fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(8)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Client that never follows redirects — used by `web_fetch` so every redirect
/// hop can be re-validated against the SSRF policy.
pub fn no_redirect_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}
