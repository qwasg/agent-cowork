//! Provider execution service: ordered fallback chain with per-provider
//! circuit breaker + bounded retry with exponential backoff.
//!
//! Additions vs. the first Rust port:
//! - `channel:{id}:{model}` model ids route to the matching channel provider
//!   (with the channel-local model name), falling back through the chain.
//! - Cancellation token support so a run cancel interrupts in-flight LLM calls.
//! - Streaming retries emit `StreamEvent::Reset` so consumers can discard
//!   partial deltas from the failed attempt instead of double-rendering.
//! - Mock fallback after a real-provider failure is flagged (`degraded`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::anthropic::AnthropicProvider;
use crate::channels::{self, Channel, Protocol};
use crate::google::GoogleProvider;
use crate::mock::MockProvider;
use crate::openai::OpenAiCompatProvider;
use crate::types::{DeltaSink, ProviderRequest, ProviderResponse, StreamEvent};
use crate::LLMProvider;
use agent_config::Config;
use agent_protocol::ApiError;
use agent_protocol::ApiResult;
use agent_store::{CryptoStore, Store};

struct BreakerState {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

#[derive(Clone)]
struct ChainEntry {
    provider: Arc<dyn LLMProvider>,
    /// Model to use when the request's model id doesn't apply to this provider
    /// (e.g. it targets another channel, or is "default"/"mock").
    default_model: Option<String>,
}

pub struct ProviderExecutionService {
    chain: Mutex<Vec<ChainEntry>>,
    breakers: Mutex<HashMap<String, BreakerState>>,
    max_retries: u32,
    failure_threshold: u32,
}

impl ProviderExecutionService {
    pub fn build(cfg: &Config, store: &Store, crypto: &Arc<CryptoStore>) -> Arc<Self> {
        Arc::new(ProviderExecutionService {
            chain: Mutex::new(Self::build_chain(cfg, store, crypto)),
            breakers: Mutex::new(HashMap::new()),
            max_retries: 2,
            failure_threshold: 3,
        })
    }

    pub fn reload(&self, cfg: &Config, store: &Store, crypto: &Arc<CryptoStore>) {
        *self.chain.lock().unwrap() = Self::build_chain(cfg, store, crypto);
    }

    fn build_chain(cfg: &Config, store: &Store, crypto: &Arc<CryptoStore>) -> Vec<ChainEntry> {
        let mut chain: Vec<ChainEntry> = Vec::new();
        let timeout = cfg.provider_timeout_secs;

        // 1) Configured channels from the durable store (highest priority).
        if let Ok(list) = store.list::<Channel>(agent_store::store::T_CHANNELS) {
            for ch in list.into_iter().filter(|c| c.enabled && c.has_key()) {
                let info = channels::provider_type(&ch.provider_type);
                let base = if !ch.base_url.is_empty() {
                    ch.base_url.clone()
                } else {
                    info.map(|i| i.default_base_url.to_string())
                        .unwrap_or_default()
                };
                if base.is_empty() {
                    continue;
                }
                let key = crypto.decrypt(&ch.api_key_enc);
                if key.is_empty() {
                    tracing::warn!("channel {} has an undecryptable API key; skipping", ch.id);
                    continue;
                }
                let name = format!("channel:{}", ch.id);
                let default_model = ch.models.first().map(|m| m.id.clone());
                let provider: Arc<dyn LLMProvider> = match info.map(|i| i.protocol) {
                    Some(Protocol::Anthropic) => {
                        Arc::new(AnthropicProvider::new(name, base, key, timeout))
                    }
                    Some(Protocol::Google) => {
                        Arc::new(GoogleProvider::new(name, base, key, timeout))
                    }
                    // OpenAI protocol and unknown provider types use the
                    // OpenAI-compatible adapter.
                    _ => Arc::new(
                        OpenAiCompatProvider::new(name, base, key, timeout)
                            .with_provider_type(ch.provider_type.clone()),
                    ),
                };
                chain.push(ChainEntry {
                    provider,
                    default_model,
                });
            }
        }

        // 2) Env-configured providers.
        if let Some(key) = &cfg.openai_api_key {
            chain.push(ChainEntry {
                provider: Arc::new(OpenAiCompatProvider::new(
                    "openai",
                    cfg.openai_base_url.clone(),
                    key.clone(),
                    timeout,
                )),
                default_model: Some(cfg.openai_model.clone()),
            });
        }
        if let Some(key) = &cfg.anthropic_api_key {
            chain.push(ChainEntry {
                provider: Arc::new(AnthropicProvider::new(
                    "anthropic",
                    cfg.anthropic_base_url.clone(),
                    key.clone(),
                    timeout,
                )),
                default_model: None,
            });
        }

        // 3) Mock fallback always present so the service never hard-fails.
        chain.push(ChainEntry {
            provider: Arc::new(MockProvider::new()),
            default_model: None,
        });

        chain
    }

    #[cfg(test)]
    pub fn for_test(providers: Vec<Arc<dyn LLMProvider>>) -> Arc<Self> {
        let svc = Arc::new(ProviderExecutionService {
            chain: Mutex::new(Vec::new()),
            breakers: Mutex::new(HashMap::new()),
            max_retries: 1,
            failure_threshold: 3,
        });
        svc.override_chain(providers);
        svc
    }

    /// Replace the whole provider chain with the given providers (no default
    /// models). Test hook used by the engine integration tests to drive the
    /// tool loop with scripted responses.
    pub fn override_chain(&self, providers: Vec<Arc<dyn LLMProvider>>) {
        *self.chain.lock().unwrap() = providers
            .into_iter()
            .map(|provider| ChainEntry {
                provider,
                default_model: None,
            })
            .collect();
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.chain
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.provider.name().to_string())
            .collect()
    }

    pub fn has_real_provider(&self) -> bool {
        self.chain
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.provider.name() != "mock" && e.provider.is_ready())
    }

    fn breaker_open(&self, name: &str) -> bool {
        let mut guard = self.breakers.lock().unwrap();
        if let Some(state) = guard.get_mut(name) {
            if let Some(until) = state.open_until {
                if Instant::now() < until {
                    return true;
                }
                state.open_until = None;
                state.consecutive_failures = 0;
            }
        }
        false
    }

    fn record_success(&self, name: &str) {
        let mut guard = self.breakers.lock().unwrap();
        guard.insert(
            name.to_string(),
            BreakerState {
                consecutive_failures: 0,
                open_until: None,
            },
        );
    }

    fn record_failure(&self, name: &str) {
        let mut guard = self.breakers.lock().unwrap();
        let state = guard.entry(name.to_string()).or_insert(BreakerState {
            consecutive_failures: 0,
            open_until: None,
        });
        state.consecutive_failures += 1;
        if state.consecutive_failures >= self.failure_threshold {
            state.open_until = Some(Instant::now() + Duration::from_secs(30));
        }
    }

    /// Execute against the chain, falling back across providers on failure.
    /// `cancel` (when provided) aborts in-flight provider calls immediately.
    pub async fn execute(
        &self,
        req: &ProviderRequest,
        sink: DeltaSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> ApiResult<ProviderResponse> {
        let target = parse_channel_target(&req.model);
        let chain = self.chain.lock().unwrap().clone();
        // Targeted channel (if any) goes first; rest keep chain order.
        let mut ordered: Vec<ChainEntry> = Vec::with_capacity(chain.len());
        if let Some((pname, _)) = &target {
            if let Some(entry) = chain.iter().find(|e| e.provider.name() == pname) {
                ordered.push(entry.clone());
            }
        }
        for entry in &chain {
            if !ordered
                .iter()
                .any(|e| e.provider.name() == entry.provider.name())
            {
                ordered.push(entry.clone());
            }
        }

        // Track whether the *current* attempt streamed any deltas so a retry
        // can tell the consumer to reset before re-streaming.
        let emitted = AtomicBool::new(false);
        let wrapped = |ev: StreamEvent| {
            if !matches!(ev, StreamEvent::Done { .. }) {
                emitted.store(true, Ordering::SeqCst);
            }
            sink(ev);
        };

        let mut last_err: Option<ApiError> = None;
        for entry in &ordered {
            let name = entry.provider.name().to_string();
            if !entry.provider.is_ready() || self.breaker_open(&name) {
                continue;
            }
            let mut attempt_req = req.clone();
            attempt_req.model = effective_model(entry, &req.model, &target);

            for attempt in 0..=self.max_retries {
                if let Some(tok) = cancel {
                    if tok.is_cancelled() {
                        return Err(ApiError::new("RUN_CANCELLED", "run cancelled"));
                    }
                }
                if emitted.swap(false, Ordering::SeqCst) {
                    // The previous attempt streamed partial output; tell the
                    // consumer to discard it before this attempt streams anew.
                    sink(StreamEvent::Reset);
                }
                let fut = entry.provider.chat(&attempt_req, &wrapped);
                let result = match cancel {
                    Some(tok) => tokio::select! {
                        _ = tok.cancelled() => {
                            return Err(ApiError::new(
                                "RUN_CANCELLED",
                                "run cancelled during provider call",
                            ))
                        }
                        r = fut => r,
                    },
                    None => fut.await,
                };
                match result {
                    Ok(mut resp) => {
                        self.record_success(&name);
                        if name == "mock" && last_err.is_some() {
                            resp.degraded = true;
                            tracing::warn!(
                                "all real providers failed ({}); serving mock fallback",
                                last_err.as_ref().map(|e| e.message.as_str()).unwrap_or("")
                            );
                        }
                        return Ok(resp);
                    }
                    Err(e) => {
                        last_err = Some(e);
                        if attempt < self.max_retries {
                            let backoff = Duration::from_millis(200 * (1 << attempt));
                            tokio::time::sleep(backoff).await;
                        }
                    }
                }
            }
            self.record_failure(&name);
            tracing::warn!("provider {} exhausted retries, falling back", name);
        }
        Err(last_err.unwrap_or_else(|| {
            ApiError::new(
                "PROVIDER_UNAVAILABLE",
                "no provider could service the request",
            )
        }))
    }
}

/// `channel:{channel_id}:{model}` → (`channel:{channel_id}`, `{model}`).
fn parse_channel_target(model: &str) -> Option<(String, String)> {
    let rest = model.strip_prefix("channel:")?;
    let (channel_id, model_name) = rest.split_once(':')?;
    if channel_id.is_empty() || model_name.is_empty() {
        return None;
    }
    Some((format!("channel:{channel_id}"), model_name.to_string()))
}

fn effective_model(
    entry: &ChainEntry,
    requested: &str,
    target: &Option<(String, String)>,
) -> String {
    if let Some((pname, model)) = target {
        if entry.provider.name() == pname {
            return model.clone();
        }
    }
    // A raw channel id or placeholder model doesn't apply to other providers;
    // use the entry's own default if it has one.
    if requested.is_empty()
        || requested == "default"
        || requested == "mock"
        || requested.starts_with("channel:")
    {
        return entry
            .default_model
            .clone()
            .unwrap_or_else(|| requested.to_string());
    }
    requested.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::noop_sink;
    use async_trait::async_trait;

    #[test]
    fn parses_channel_target() {
        assert_eq!(
            parse_channel_target("channel:ch_1:gpt-4o"),
            Some(("channel:ch_1".to_string(), "gpt-4o".to_string()))
        );
        assert_eq!(parse_channel_target("gpt-4o"), None);
        assert_eq!(parse_channel_target("channel:only-id"), None);
    }

    struct FailingProvider;

    #[async_trait]
    impl LLMProvider for FailingProvider {
        fn name(&self) -> &str {
            "failing"
        }
        async fn chat(
            &self,
            _req: &ProviderRequest,
            _sink: DeltaSink<'_>,
        ) -> ApiResult<ProviderResponse> {
            Err(ApiError::new("PROVIDER_HTTP_ERROR", "boom"))
        }
    }

    fn req() -> ProviderRequest {
        ProviderRequest {
            model: "default".to_string(),
            messages: vec![agent_protocol::models::ChatMessage::user("hi")],
            tools: vec![],
            temperature: None,
            stream: false,
            max_tokens: None,
        }
    }

    #[tokio::test]
    async fn falls_back_to_mock_and_marks_degraded() {
        let svc = ProviderExecutionService::for_test(vec![
            Arc::new(FailingProvider),
            Arc::new(MockProvider::new()),
        ]);
        let sink = noop_sink();
        let resp = svc.execute(&req(), &sink, None).await.unwrap();
        assert_eq!(resp.provider, "mock");
        assert!(resp.degraded, "mock after a real failure must be flagged");
    }

    #[tokio::test]
    async fn mock_without_prior_failure_is_not_degraded() {
        let svc = ProviderExecutionService::for_test(vec![Arc::new(MockProvider::new())]);
        let sink = noop_sink();
        let resp = svc.execute(&req(), &sink, None).await.unwrap();
        assert!(!resp.degraded);
    }

    #[tokio::test]
    async fn breaker_opens_after_repeated_failures() {
        let svc = ProviderExecutionService::for_test(vec![Arc::new(FailingProvider)]);
        let sink = noop_sink();
        for _ in 0..3 {
            let _ = svc.execute(&req(), &sink, None).await;
        }
        assert!(svc.breaker_open("failing"));
    }

    #[tokio::test]
    async fn cancelled_token_short_circuits() {
        let svc = ProviderExecutionService::for_test(vec![Arc::new(MockProvider::new())]);
        let tok = CancellationToken::new();
        tok.cancel();
        let sink = noop_sink();
        let err = svc.execute(&req(), &sink, Some(&tok)).await.unwrap_err();
        assert_eq!(err.code, "RUN_CANCELLED");
    }
}
