//! Provider-facing request/response types and the streaming delta protocol.
//! The unified stream protocol mirrors the Python adapter:
//! `text` / `reasoning` / `tool_call` / `done`.

use serde_json::Value;

use crate::contracts::models::{ChatMessage, ToolCall};

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolSpec {
    /// OpenAI `tools=[{type:function, function:{...}}]` JSON shape.
    pub fn to_openai_json(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub stream: bool,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Usage,
    pub provider: String,
    pub model: String,
    /// True when this response came from the mock fallback *after* one or more
    /// real providers failed — callers should surface this to the user.
    pub degraded: bool,
}

impl ProviderResponse {
    pub fn empty(provider: &str, model: &str) -> Self {
        ProviderResponse {
            text: String::new(),
            reasoning: None,
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: Usage::default(),
            provider: provider.to_string(),
            model: model.to_string(),
            degraded: false,
        }
    }
}

/// Streaming delta events emitted while a response is being produced.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    Reasoning(String),
    ToolCall {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    },
    Done {
        finish_reason: String,
    },
    /// A previous (partially streamed) attempt failed and is being retried;
    /// consumers should discard any deltas received so far for this request.
    Reset,
}

/// Callback the runtime supplies so streaming deltas can be turned into bus events.
pub type DeltaSink<'a> = &'a (dyn Fn(StreamEvent) + Send + Sync);

/// No-op sink for non-streaming calls.
pub fn noop_sink() -> impl Fn(StreamEvent) + Send + Sync {
    |_e: StreamEvent| {}
}
