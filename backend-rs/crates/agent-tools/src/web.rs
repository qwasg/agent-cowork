//! Web tools: `web_search` (Tavily) and `web_fetch` (HTTP GET + text strip).
//! Port of `web_search_service.py` / `web_tools.py`.
//!
//! `web_fetch` is SSRF-hardened: scheme whitelist, DNS resolution with
//! private / link-local / loopback rejection, manual redirect following with
//! per-hop re-validation, a 30s timeout and a 2 MiB response cap.

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::contracts::{ApiError, ApiResult};
use crate::infra::http::{no_redirect_client, shared_client};
use crate::tools::{AgentTool, ToolContext};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const FETCH_MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

pub struct WebSearch;

#[async_trait]
impl AgentTool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web via Tavily and return result titles, urls and snippets."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "query": {"type": "string"} },
            "required": ["query"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if query.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "query required"));
        }
        let Some(key) = ctx.web.api_key.clone() else {
            return Ok("(web_search unavailable: no TAVILY_API_KEY configured)".to_string());
        };
        let resp = shared_client()
            .post(format!("{}/search", ctx.web.base_url.trim_end_matches('/')))
            .timeout(FETCH_TIMEOUT)
            .json(&json!({ "api_key": key, "query": query, "max_results": 5 }))
            .send()
            .await
            .map_err(|e| ApiError::new("WEB_SEARCH_ERROR", e.to_string()))?;
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ApiError::new("WEB_SEARCH_ERROR", e.to_string()))?;
        let mut out = String::new();
        if let Some(results) = body.get("results").and_then(|r| r.as_array()) {
            for r in results {
                let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let snippet = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!(
                    "- {title}\n  {url}\n  {}\n",
                    snippet.chars().take(300).collect::<String>()
                ));
            }
        }
        if out.is_empty() {
            out.push_str("(no results)");
        }
        Ok(out)
    }
}

pub struct WebFetch;

#[async_trait]
impl AgentTool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }
    fn description(&self) -> &str {
        "Fetch a URL and return its readable text content."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "url": {"type": "string"} },
            "required": ["url"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let raw = args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let mut url = reqwest::Url::parse(&raw)
            .map_err(|_| ApiError::new("TOOL_INVALID_ARGS", "valid http(s) url required"))?;

        for _hop in 0..=MAX_REDIRECTS {
            validate_url(&url, ctx.web.allow_private).await?;
            let resp = no_redirect_client()
                .get(url.clone())
                .timeout(FETCH_TIMEOUT)
                .header("User-Agent", "agentd/0.1 (+https://localhost)")
                .send()
                .await
                .map_err(|e| ApiError::new("WEB_FETCH_ERROR", e.to_string()))?;

            if resp.status().is_redirection() {
                let loc = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        ApiError::new("WEB_FETCH_ERROR", "redirect without Location header")
                    })?;
                url = url
                    .join(loc)
                    .map_err(|_| ApiError::new("WEB_FETCH_ERROR", "invalid redirect target"))?;
                continue;
            }

            let mut buf: Vec<u8> = Vec::new();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|e| ApiError::new("WEB_FETCH_ERROR", e.to_string()))?;
                if buf.len() + chunk.len() > FETCH_MAX_BYTES {
                    buf.extend_from_slice(&chunk[..FETCH_MAX_BYTES - buf.len()]);
                    break;
                }
                buf.extend_from_slice(&chunk);
            }
            let text = String::from_utf8_lossy(&buf);
            let stripped = strip_html(&text);
            return Ok(stripped.chars().take(ctx.web.fetch_max_chars).collect());
        }
        Err(ApiError::new("WEB_FETCH_ERROR", "too many redirects"))
    }
}

/// Reject non-http(s) schemes and (unless `allow_private`) URLs whose host
/// resolves to a private / loopback / link-local / CGNAT address.
async fn validate_url(url: &reqwest::Url, allow_private: bool) -> ApiResult<()> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ApiError::new(
                "TOOL_INVALID_ARGS",
                format!("unsupported url scheme: {other}"),
            ))
        }
    }
    if allow_private {
        return Ok(());
    }
    let blocked = || {
        ApiError::new(
            "WEB_FETCH_ERROR",
            "fetching private or internal addresses is not allowed",
        )
    };
    let ips: Vec<IpAddr> = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![IpAddr::V4(ip)],
        Some(url::Host::Ipv6(ip)) => vec![IpAddr::V6(ip)],
        Some(url::Host::Domain(domain)) => {
            let port = url.port_or_known_default().unwrap_or(80);
            tokio::net::lookup_host((domain, port))
                .await
                .map_err(|e| ApiError::new("WEB_FETCH_ERROR", format!("dns: {e}")))?
                .map(|sa| sa.ip())
                .collect()
        }
        None => return Err(blocked()),
    };
    if ips.is_empty() || ips.iter().any(is_private_ip) {
        return Err(blocked());
    }
    Ok(())
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // CGNAT 100.64.0.0/10
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(&IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                // unique-local fc00::/7
                || (seg0 & 0xfe00) == 0xfc00
                // link-local fe80::/10
                || (seg0 & 0xffc0) == 0xfe80
        }
    }
}

/// Very small HTML-to-text: drop tags and collapse whitespace.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let lower = html.to_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if lower[i..].starts_with("<script") || lower[i..].starts_with("<style") {
            in_script = true;
        }
        if in_script {
            if lower[i..].starts_with("</script>") || lower[i..].starts_with("</style>") {
                in_script = false;
                i += 9;
                continue;
            }
            i += 1;
            continue;
        }
        let c = bytes[i] as char;
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_ips() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.5.5",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(is_private_ip(&ip.parse().unwrap()), "{ip} should be private");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(!is_private_ip(&ip.parse().unwrap()), "{ip} should be public");
        }
    }

    #[tokio::test]
    async fn rejects_bad_schemes_and_private_hosts() {
        let u = reqwest::Url::parse("ftp://example.com/x").unwrap();
        assert!(validate_url(&u, false).await.is_err());
        let u = reqwest::Url::parse("http://127.0.0.1:8080/admin").unwrap();
        assert!(validate_url(&u, false).await.is_err());
        let u = reqwest::Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
        assert!(validate_url(&u, false).await.is_err());
        // allow_private opts out
        let u = reqwest::Url::parse("http://127.0.0.1:8080/ok").unwrap();
        assert!(validate_url(&u, true).await.is_ok());
    }
}
