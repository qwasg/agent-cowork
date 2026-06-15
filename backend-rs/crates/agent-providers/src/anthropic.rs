//! Native Anthropic Messages API adapter (port of `anthropic_adapter.py`).
//! Covers Anthropic itself plus vendors exposing the Anthropic wire protocol
//! (Kimi, MiniMax, …). Supports streaming + tool use.

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::openai::assemble;
use crate::types::{DeltaSink, ProviderRequest, ProviderResponse, StreamEvent, Usage};
use crate::LLMProvider;
use agent_protocol::models::ToolCall;
use agent_protocol::{ApiError, ApiResult};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    name: String,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
    ) -> Self {
        AnthropicProvider {
            name: name.into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            client: reqwest::Client::builder()
                .pool_max_idle_per_host(8)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(timeout_secs.max(10)))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        }
    }

    fn build_body(&self, req: &ProviderRequest) -> Value {
        let mut system = String::new();
        let mut messages: Vec<Value> = Vec::new();
        for m in &req.messages {
            match m.role.as_str() {
                "system" => {
                    if !system.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&m.content);
                }
                "tool" => {
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                            "content": m.content,
                        }]
                    }));
                }
                "assistant" if !m.tool_calls.is_empty() => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        blocks.push(json!({ "type": "text", "text": m.content }));
                    }
                    for c in &m.tool_calls {
                        let input: Value =
                            serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": input,
                        }));
                    }
                    messages.push(json!({ "role": "assistant", "content": blocks }));
                }
                role => {
                    let role = if role == "assistant" {
                        "assistant"
                    } else {
                        "user"
                    };
                    messages.push(json!({ "role": role, "content": m.content }));
                }
            }
        }
        let mut body = json!({
            "model": req.model,
            "messages": messages,
            "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "stream": req.stream,
        });
        if !system.is_empty() {
            // Block form with cache_control: the (prefix-stable) system
            // prompt becomes a prompt-cache anchor across turns.
            body["system"] = json!([{
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" },
            }]);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(
                req.tools
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "input_schema": t.parameters,
                        })
                    })
                    .collect(),
            );
        }
        body
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
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
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
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

impl AnthropicProvider {
    async fn consume_stream(
        &self,
        resp: reqwest::Response,
        model: &str,
        sink: DeltaSink<'_>,
    ) -> ApiResult<ProviderResponse> {
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_acc: BTreeMap<usize, (Option<String>, Option<String>, String)> =
            BTreeMap::new();
        let mut finish_reason = String::from("stop");
        let mut usage = Usage::default();

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::new("PROVIDER_STREAM_ERROR", e.to_string()))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim().to_string();
                buf.drain(..=nl);
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
                    continue;
                };
                match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "content_block_start" => {
                        let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        if let Some(block) = v.get("content_block") {
                            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                let id = block
                                    .get("id")
                                    .and_then(|i| i.as_str())
                                    .map(|s| s.to_string());
                                let name = block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .map(|s| s.to_string());
                                tool_acc.insert(idx, (id, name, String::new()));
                            }
                        }
                    }
                    "content_block_delta" => {
                        let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        if let Some(delta) = v.get("delta") {
                            match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                                "text_delta" => {
                                    if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                                        text.push_str(t);
                                        sink(StreamEvent::Text(t.to_string()));
                                    }
                                }
                                "thinking_delta" => {
                                    if let Some(t) = delta.get("thinking").and_then(|t| t.as_str())
                                    {
                                        reasoning.push_str(t);
                                        sink(StreamEvent::Reasoning(t.to_string()));
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(pj) =
                                        delta.get("partial_json").and_then(|p| p.as_str())
                                    {
                                        let entry = tool_acc.entry(idx).or_insert((
                                            None,
                                            None,
                                            String::new(),
                                        ));
                                        entry.2.push_str(pj);
                                        sink(StreamEvent::ToolCall {
                                            index: idx,
                                            id: entry.0.clone(),
                                            name: entry.1.clone(),
                                            arguments: pj.to_string(),
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(sr) = v
                            .get("delta")
                            .and_then(|d| d.get("stop_reason"))
                            .and_then(|s| s.as_str())
                        {
                            finish_reason = map_stop_reason(sr);
                        }
                        if let Some(u) = v.get("usage") {
                            usage.completion_tokens = u
                                .get("output_tokens")
                                .and_then(|n| n.as_u64())
                                .unwrap_or(usage.completion_tokens as u64)
                                as u32;
                        }
                    }
                    "message_start" => {
                        if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                            usage.prompt_tokens =
                                u.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0) as u32;
                            usage.cache_read_tokens =
                                u.get("cache_read_input_tokens")
                                    .and_then(|n| n.as_u64())
                                    .unwrap_or(0) as u32;
                        }
                    }
                    "message_stop" => {
                        sink(StreamEvent::Done {
                            finish_reason: finish_reason.clone(),
                        });
                        usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
                        let mut resp =
                            assemble(text, reasoning, tool_acc, finish_reason, &self.name, model);
                        resp.usage = usage;
                        return Ok(resp);
                    }
                    _ => {}
                }
            }
        }
        sink(StreamEvent::Done {
            finish_reason: finish_reason.clone(),
        });
        usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
        let mut out = assemble(text, reasoning, tool_acc, finish_reason, &self.name, model);
        out.usage = usage;
        Ok(out)
    }
}

fn map_stop_reason(sr: &str) -> String {
    match sr {
        "end_turn" | "stop_sequence" => "stop",
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        other => other,
    }
    .to_string()
}

fn parse_full_response(v: &Value, provider: &str, model: &str) -> ProviderResponse {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    if let Some(blocks) = v.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
                "thinking" => {
                    if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                        reasoning.push_str(t);
                    }
                }
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        tool_calls.push(ToolCall {
                            id: block
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string(),
                            kind: "function".to_string(),
                            name,
                            arguments: block
                                .get("input")
                                .map(|i| i.to_string())
                                .unwrap_or_else(|| "{}".to_string()),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    let finish_reason = v
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .map(map_stop_reason)
        .unwrap_or_else(|| "stop".to_string());
    let usage = v.get("usage");
    let prompt = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0) as u32;
    let completion = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0) as u32;
    ProviderResponse {
        text,
        reasoning: if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        },
        tool_calls,
        finish_reason,
        usage: Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cache_read_tokens: usage
                .and_then(|u| u.get("cache_read_input_tokens"))
                .and_then(|n| n.as_u64())
                .unwrap_or(0) as u32,
        },
        provider: provider.to_string(),
        model: model.to_string(),
        degraded: false,
    }
}
