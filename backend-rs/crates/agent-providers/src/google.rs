//! Google Gemini adapter (generateContent API). Previously `Protocol::Google`
//! channels were silently skipped; this wires them into the provider chain.
//! Uses the non-streaming endpoint and emits the result as a single delta.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::types::{DeltaSink, ProviderRequest, ProviderResponse, StreamEvent, Usage};
use crate::LLMProvider;
use agent_protocol::models::{new_id, ToolCall};
use agent_protocol::{ApiError, ApiResult};

pub struct GoogleProvider {
    name: String,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl GoogleProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout_secs: u64,
    ) -> Self {
        GoogleProvider {
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

    fn endpoint(&self, model: &str) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            model
        )
    }

    fn build_body(&self, req: &ProviderRequest) -> Value {
        let mut system = String::new();
        let mut contents: Vec<Value> = Vec::new();
        for m in &req.messages {
            match m.role.as_str() {
                "system" => {
                    if !system.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&m.content);
                }
                "tool" => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": m.name.clone().unwrap_or_default(),
                                "response": { "result": m.content },
                            }
                        }]
                    }));
                }
                "assistant" if !m.tool_calls.is_empty() => {
                    let mut parts: Vec<Value> = Vec::new();
                    if !m.content.is_empty() {
                        parts.push(json!({ "text": m.content }));
                    }
                    for c in &m.tool_calls {
                        let args: Value =
                            serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
                        parts.push(json!({ "functionCall": { "name": c.name, "args": args } }));
                    }
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
                "assistant" => {
                    contents.push(json!({ "role": "model", "parts": [{ "text": m.content }] }));
                }
                _ => {
                    contents.push(json!({ "role": "user", "parts": [{ "text": m.content }] }));
                }
            }
        }
        let mut body = json!({ "contents": contents });
        if !system.is_empty() {
            body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
        }
        let mut gen_cfg = serde_json::Map::new();
        if let Some(t) = req.temperature {
            gen_cfg.insert("temperature".to_string(), json!(t));
        }
        if let Some(mt) = req.max_tokens {
            gen_cfg.insert("maxOutputTokens".to_string(), json!(mt));
        }
        if !gen_cfg.is_empty() {
            body["generationConfig"] = Value::Object(gen_cfg);
        }
        if !req.tools.is_empty() {
            let decls: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            body["tools"] = json!([{ "functionDeclarations": decls }]);
        }
        body
    }
}

#[async_trait]
impl LLMProvider for GoogleProvider {
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
            .post(self.endpoint(&req.model))
            .header("x-goog-api-key", &self.api_key)
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

        let value: Value = resp
            .json()
            .await
            .map_err(|e| ApiError::new("PROVIDER_DECODE_ERROR", e.to_string()))?;
        let out = parse_response(&value, &self.name, &req.model);
        if !out.text.is_empty() {
            sink(StreamEvent::Text(out.text.clone()));
        }
        sink(StreamEvent::Done {
            finish_reason: out.finish_reason.clone(),
        });
        Ok(out)
    }
}

fn parse_response(v: &Value, provider: &str, model: &str) -> ProviderResponse {
    let candidate = v.get("candidates").and_then(|c| c.get(0));
    let parts = candidate
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array());
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    if let Some(parts) = parts {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() {
                    tool_calls.push(ToolCall {
                        id: new_id("call"),
                        kind: "function".to_string(),
                        name,
                        arguments: fc
                            .get("args")
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "{}".to_string()),
                    });
                }
            }
        }
    }
    let finish_reason = if !tool_calls.is_empty() {
        "tool_calls".to_string()
    } else {
        match candidate
            .and_then(|c| c.get("finishReason"))
            .and_then(|f| f.as_str())
            .unwrap_or("STOP")
        {
            "STOP" => "stop",
            "MAX_TOKENS" => "length",
            other => other,
        }
        .to_string()
    };
    let usage_meta = v.get("usageMetadata");
    let prompt = usage_meta
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0) as u32;
    let completion = usage_meta
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0) as u32;
    let total = usage_meta
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(|n| n.as_u64())
        .unwrap_or((prompt + completion) as u64) as u32;
    ProviderResponse {
        text,
        reasoning: None,
        tool_calls,
        finish_reason,
        usage: Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total,
            cache_read_tokens: usage_meta
                .and_then(|u| u.get("cachedContentTokenCount"))
                .and_then(|n| n.as_u64())
                .unwrap_or(0) as u32,
        },
        provider: provider.to_string(),
        model: model.to_string(),
        degraded: false,
    }
}
