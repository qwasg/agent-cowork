//! OpenAI-compatible adapter (covers OpenAI, DeepSeek, 智谱, 豆包, 通义 …).
//! Streaming uses `reqwest` byte streaming with an incremental SSE parser,
//! reusing a single pooled `Client`.

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::types::{DeltaSink, ProviderRequest, ProviderResponse, StreamEvent, Usage};
use crate::LLMProvider;
use agent_protocol::models::{ChatMessage, ToolCall};
use agent_protocol::ApiError;
use agent_protocol::ApiResult;

pub struct OpenAiCompatProvider {
    name: String,
    base_url: String,
    api_key: String,
    /// Vendor type ("openai" / "deepseek" / "qwen" / "zhipu" / …) used to
    /// pick the right thinking-mode request fields.
    provider_type: String,
    client: reqwest::Client,
}

impl OpenAiCompatProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
    ) -> Self {
        OpenAiCompatProvider {
            name: name.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            provider_type: "openai".to_string(),
            client: reqwest::Client::builder()
                .pool_max_idle_per_host(8)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(timeout_secs.max(10)))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn with_provider_type(mut self, provider_type: impl Into<String>) -> Self {
        self.provider_type = provider_type.into();
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn build_body(&self, req: &ProviderRequest) -> Value {
        let messages: Vec<Value> = req.messages.iter().map(message_to_openai).collect();
        let mut body = json!({
            "model": req.model,
            "messages": messages,
            "stream": req.stream,
        });
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(mt) = req.max_tokens {
            body["max_tokens"] = json!(mt);
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.iter().map(|t| t.to_openai_json()).collect());
        }
        // Vendor-specific thinking fields. Streaming requests enable thinking
        // when the model supports it (reasoning deltas flow back as
        // `reasoning_content`); non-streaming requests follow the vendor's
        // disable strategy (some require an explicit `disabled`).
        use crate::thinking::{
            apply_thinking_to_openai_request, detect_thinking_capability, DisableStrategy,
        };
        let cap = detect_thinking_capability(&self.provider_type, &req.model);
        if cap.supports_thinking() && req.stream {
            apply_thinking_to_openai_request(&mut body, cap, true);
        } else if cap.disable_strategy == DisableStrategy::ExplicitDisabled {
            apply_thinking_to_openai_request(&mut body, cap, false);
        }
        body
    }
}

fn message_to_openai(m: &ChatMessage) -> Value {
    match m.role.as_str() {
        "tool" => json!({
            "role": "tool",
            "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
            "content": m.content,
        }),
        "assistant" if !m.tool_calls.is_empty() => {
            let calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.arguments }
                    })
                })
                .collect();
            json!({
                "role": "assistant",
                "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                "tool_calls": calls,
            })
        }
        _ => json!({ "role": m.role, "content": m.content }),
    }
}

#[async_trait]
impl LLMProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_ready(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn chat(
        &self,
        req: &ProviderRequest,
        sink: DeltaSink<'_>,
    ) -> ApiResult<ProviderResponse> {
        let body = self.build_body(req);
        let resp = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::new("PROVIDER_HTTP_ERROR", format!("{}: {e}", self.name)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(ApiError::new(
                "PROVIDER_HTTP_ERROR",
                format!(
                    "{} returned {}: {}",
                    self.name,
                    status,
                    detail.chars().take(400).collect::<String>()
                ),
            ));
        }

        if req.stream {
            self.consume_stream(resp, &req.model, sink).await
        } else {
            let value: Value = resp
                .json()
                .await
                .map_err(|e| ApiError::new("PROVIDER_DECODE_ERROR", e.to_string()))?;
            Ok(parse_full_response(&value, &self.name, &req.model))
        }
    }
}

/// Incremental accumulator for an OpenAI-style SSE chat stream. Extracted so
/// the merge logic (tool_call deltas by index, content/reasoning append) is
/// unit-testable without a live HTTP stream.
pub(crate) struct SseAccumulator {
    pub text: String,
    pub reasoning: String,
    pub tool_acc: BTreeMap<usize, (Option<String>, Option<String>, String)>,
    pub finish_reason: String,
    pub usage: Usage,
    pub done: bool,
}

impl SseAccumulator {
    pub fn new() -> Self {
        SseAccumulator {
            text: String::new(),
            reasoning: String::new(),
            tool_acc: BTreeMap::new(),
            finish_reason: "stop".to_string(),
            usage: Usage::default(),
            done: false,
        }
    }

    /// Ingest one `data:` payload. Returns true when the stream is finished.
    pub fn ingest(&mut self, data: &str, sink: DeltaSink<'_>) -> bool {
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            sink(StreamEvent::Done {
                finish_reason: self.finish_reason.clone(),
            });
            return true;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return false;
        };
        if let Some(u) = v.get("usage") {
            self.usage = parse_usage(Some(u));
        }
        if let Some(choice) = v.get("choices").and_then(|c| c.get(0)) {
            if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                self.finish_reason = fr.to_string();
            }
            if let Some(delta) = choice.get("delta") {
                if let Some(c) = delta.get("content").and_then(|c| c.as_str()) {
                    if !c.is_empty() {
                        self.text.push_str(c);
                        sink(StreamEvent::Text(c.to_string()));
                    }
                }
                if let Some(r) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                    if !r.is_empty() {
                        self.reasoning.push_str(r);
                        sink(StreamEvent::Reasoning(r.to_string()));
                    }
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let entry = self
                            .tool_acc
                            .entry(idx)
                            .or_insert((None, None, String::new()));
                        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                            entry.0 = Some(id.to_string());
                        }
                        if let Some(f) = tc.get("function") {
                            if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                                entry.1 = Some(n.to_string());
                            }
                            if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) {
                                entry.2.push_str(a);
                            }
                        }
                        sink(StreamEvent::ToolCall {
                            index: idx,
                            id: entry.0.clone(),
                            name: entry.1.clone(),
                            arguments: tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                                .unwrap_or("")
                                .to_string(),
                        });
                    }
                }
            }
        }
        false
    }

    pub fn finish(self, provider: &str, model: &str) -> ProviderResponse {
        let usage = self.usage.clone();
        let mut resp = assemble(
            self.text,
            self.reasoning,
            self.tool_acc,
            self.finish_reason,
            provider,
            model,
        );
        resp.usage = usage;
        resp
    }
}

impl OpenAiCompatProvider {
    async fn consume_stream(
        &self,
        resp: reqwest::Response,
        model: &str,
        sink: DeltaSink<'_>,
    ) -> ApiResult<ProviderResponse> {
        let mut acc = SseAccumulator::new();
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::new("PROVIDER_STREAM_ERROR", e.to_string()))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // Process complete lines, keep remainder.
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim().to_string();
                buf.drain(..=nl);
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                if acc.ingest(data, sink) {
                    return Ok(acc.finish(&self.name, model));
                }
            }
        }
        sink(StreamEvent::Done {
            finish_reason: acc.finish_reason.clone(),
        });
        Ok(acc.finish(&self.name, model))
    }
}

pub(crate) fn assemble(
    text: String,
    reasoning: String,
    tool_acc: BTreeMap<usize, (Option<String>, Option<String>, String)>,
    finish_reason: String,
    provider: &str,
    model: &str,
) -> ProviderResponse {
    let tool_calls: Vec<ToolCall> = tool_acc
        .into_values()
        .filter_map(|(id, name, args)| {
            let name = name?;
            Some(ToolCall {
                id: id.unwrap_or_else(|| agent_protocol::models::new_id("call")),
                kind: "function".to_string(),
                name,
                arguments: if args.is_empty() {
                    "{}".to_string()
                } else {
                    args
                },
            })
        })
        .collect();
    ProviderResponse {
        text,
        reasoning: if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        },
        tool_calls,
        finish_reason,
        usage: Usage::default(),
        provider: provider.to_string(),
        model: model.to_string(),
        degraded: false,
    }
}

pub(crate) fn parse_usage(usage: Option<&Value>) -> Usage {
    Usage {
        prompt_tokens: usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as u32,
        completion_tokens: usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as u32,
        total_tokens: usage
            .and_then(|u| u.get("total_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as u32,
        cache_read_tokens: usage
            .and_then(|u| u.pointer("/prompt_tokens_details/cached_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as u32,
    }
}

fn parse_full_response(v: &Value, provider: &str, model: &str) -> ProviderResponse {
    let choice = v.get("choices").and_then(|c| c.get(0));
    let message = choice.and_then(|c| c.get("message"));
    let text = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let reasoning = message
        .and_then(|m| m.get("reasoning_content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    let mut tool_calls = Vec::new();
    if let Some(tcs) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for tc in tcs {
            let id = tc
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            let f = tc.get("function");
            let name = f
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = f
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("{}")
                .to_string();
            if !name.is_empty() {
                tool_calls.push(ToolCall {
                    id,
                    kind: "function".to_string(),
                    name,
                    arguments: args,
                });
            }
        }
    }
    let finish_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .unwrap_or("stop")
        .to_string();
    let usage = parse_usage(v.get("usage"));
    ProviderResponse {
        text,
        reasoning,
        tool_calls,
        finish_reason,
        usage,
        provider: provider.to_string(),
        model: model.to_string(),
        degraded: false,
    }
}

#[cfg(test)]
mod http_tests {
    //! HTTP-boundary tests against a local mock server (JSON + SSE paths).

    use super::*;
    use std::sync::{Arc as StdArc, Mutex};

    async fn serve(
        response: axum::response::Response<axum::body::Body>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::routing::post;
        let resp = StdArc::new(Mutex::new(Some(response)));
        let app = axum::Router::new().route(
            "/chat/completions",
            post(move || {
                let resp = resp.clone();
                async move { resp.lock().unwrap().take().expect("single-shot mock") }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), handle)
    }

    fn req(stream: bool) -> ProviderRequest {
        ProviderRequest {
            model: "test-model".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            temperature: None,
            stream,
            max_tokens: None,
        }
    }

    #[tokio::test]
    async fn non_stream_parses_text_and_cached_usage() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "回答" }, "finish_reason": "stop" }],
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": 8,
                "total_tokens": 128,
                "prompt_tokens_details": { "cached_tokens": 100 }
            }
        });
        let resp = axum::response::Response::builder()
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let (base, server) = serve(resp).await;

        let provider = OpenAiCompatProvider::new("test", base, "k", 10);
        let sink = crate::types::noop_sink();
        let out = provider.chat(&req(false), &sink).await.unwrap();
        assert_eq!(out.text, "回答");
        assert_eq!(out.usage.prompt_tokens, 120);
        assert_eq!(out.usage.total_tokens, 128);
        assert_eq!(out.usage.cache_read_tokens, 100);
        server.abort();
    }

    #[tokio::test]
    async fn stream_parses_sse_and_emits_tool_arg_deltas() {
        let sse = "\
data: {\"choices\":[{\"delta\":{\"content\":\"思\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"grep\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"x\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n\
data: [DONE]\n\n";
        let resp = axum::response::Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from(sse))
            .unwrap();
        let (base, server) = serve(resp).await;

        let provider = OpenAiCompatProvider::new("test", base, "k", 10);
        let events: StdArc<Mutex<Vec<StreamEvent>>> = StdArc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = move |ev: StreamEvent| captured.lock().unwrap().push(ev);
        let out = provider.chat(&req(true), &sink).await.unwrap();

        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name, "grep");
        assert_eq!(out.tool_calls[0].arguments, r#"{"q":"x"}"#);
        assert_eq!(out.usage.total_tokens, 15);
        let events = events.lock().unwrap();
        // Tool-call argument deltas streamed incrementally to the sink.
        let tool_deltas = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::ToolCall { .. }))
            .count();
        assert!(tool_deltas >= 2, "got {tool_deltas} tool deltas");
        server.abort();
    }

    #[tokio::test]
    async fn http_error_status_is_a_provider_error() {
        let resp = axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::from("kaboom"))
            .unwrap();
        let (base, server) = serve(resp).await;
        let provider = OpenAiCompatProvider::new("test", base, "k", 10);
        let sink = crate::types::noop_sink();
        let err = provider.chat(&req(false), &sink).await.unwrap_err();
        assert_eq!(err.code, "PROVIDER_HTTP_ERROR");
        assert!(err.message.contains("500"));
        server.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::noop_sink;

    #[test]
    fn accumulates_text_and_merges_tool_call_deltas() {
        let mut acc = SseAccumulator::new();
        let sink = noop_sink();
        let chunks = [
            r#"{"choices":[{"delta":{"content":"你好"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.txt\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"list_dir","arguments":""}}]}},{"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        for c in &chunks {
            assert!(!acc.ingest(c, &sink));
        }
        assert!(acc.ingest("[DONE]", &sink));
        let resp = acc.finish("test", "m");
        assert_eq!(resp.text, "你好");
        assert_eq!(resp.tool_calls.len(), 2);
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.tool_calls[0].arguments, r#"{"path":"a.txt"}"#);
        assert_eq!(resp.tool_calls[1].name, "list_dir");
        assert_eq!(resp.tool_calls[1].arguments, "{}");
        assert_eq!(resp.finish_reason, "tool_calls");
    }

    #[test]
    fn ignores_malformed_chunks() {
        let mut acc = SseAccumulator::new();
        let sink = noop_sink();
        assert!(!acc.ingest("not json at all", &sink));
        assert!(!acc.ingest(r#"{"choices":[{"delta":{"content":"ok"}}]}"#, &sink));
        let resp = acc.finish("test", "m");
        assert_eq!(resp.text, "ok");
    }
}
