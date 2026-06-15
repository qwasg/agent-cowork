//! Deterministic offline provider used as the final fallback and in tests
//! (mirrors `mock_provider.py` / `fake_provider.py`).

use async_trait::async_trait;

use crate::types::{DeltaSink, ProviderRequest, ProviderResponse, StreamEvent};
use crate::LLMProvider;
use agent_protocol::ApiResult;

pub struct MockProvider {
    name: String,
}

impl MockProvider {
    pub fn new() -> Self {
        MockProvider {
            name: "mock".to_string(),
        }
    }
}

#[async_trait]
impl LLMProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat(
        &self,
        req: &ProviderRequest,
        sink: DeltaSink<'_>,
    ) -> ApiResult<ProviderResponse> {
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let text = if last_user.trim().is_empty() {
            "（mock）你好，我是离线模拟模型，未配置任何真实 LLM 渠道。".to_string()
        } else {
            format!(
                "（mock 回复）已收到你的请求：{}。当前为离线模拟模型，请在「渠道」中配置真实模型以获得完整能力。",
                last_user.chars().take(200).collect::<String>()
            )
        };
        // Emit the text as a single streaming delta so the UI still animates.
        sink(StreamEvent::Text(text.clone()));
        sink(StreamEvent::Done {
            finish_reason: "stop".to_string(),
        });
        let mut resp = ProviderResponse::empty(&self.name, &req.model);
        resp.text = text;
        Ok(resp)
    }
}
