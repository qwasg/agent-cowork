//! Model catalog, provider status, search config and model preferences.

use serde_json::{json, Value};

use agent_protocol::models::AgentModelOption;
use agent_protocol::{ApiError, ApiResult};
use agent_providers::channels::Channel;
use agent_store::store::T_CHANNELS;

use crate::api::AppServices;

impl AppServices {
    pub fn list_models(&self) -> Value {
        let mut models: Vec<AgentModelOption> = Vec::new();
        // Env OpenAI-compatible default.
        if self.cfg.openai_api_key.is_some() {
            models.push(AgentModelOption {
                id: self.cfg.openai_model.clone(),
                label: self.cfg.openai_model.clone(),
                provider: "openai".to_string(),
                source: "env".to_string(),
                tier: "balanced".to_string(),
                supports_reasoning: false,
                context_window_tokens: Some(128_000),
                availability: "available".to_string(),
                is_default: true,
            });
        }
        // Channel models.
        if let Ok(list) = self.store.list::<Channel>(T_CHANNELS) {
            for ch in list.into_iter().filter(|c| c.enabled) {
                for m in &ch.models {
                    models.push(AgentModelOption {
                        id: format!("channel:{}:{}", ch.id, m.id),
                        label: format!("{} · {}", ch.label, m.label),
                        provider: ch.provider_type.clone(),
                        source: "channel".to_string(),
                        tier: "balanced".to_string(),
                        supports_reasoning: m.supports_reasoning,
                        context_window_tokens: Some(128_000),
                        availability: if ch.has_key() {
                            "available"
                        } else {
                            "needs-key"
                        }
                        .to_string(),
                        is_default: false,
                    });
                }
            }
        }
        if models.is_empty() {
            models.push(AgentModelOption {
                id: "mock".to_string(),
                label: "离线模拟模型 (mock)".to_string(),
                provider: "mock".to_string(),
                source: "builtin".to_string(),
                tier: "fast".to_string(),
                supports_reasoning: false,
                context_window_tokens: Some(32_000),
                availability: "available".to_string(),
                is_default: true,
            });
        }
        let default_id = models.iter().find(|m| m.is_default).map(|m| m.id.clone());
        json!({ "models": models, "defaultModelId": default_id })
    }

    pub fn provider_status(&self) -> Value {
        json!({
            "providers": self.providers.provider_names(),
            "hasRealProvider": self.providers.has_real_provider(),
        })
    }

    pub fn get_search_config(&self) -> Value {
        json!({ "config": self.search_config.public_view() })
    }

    pub fn set_search_config(&self, payload: &Value) -> Value {
        let _ = self.search_config.save_patch(payload);
        json!({ "config": self.search_config.public_view() })
    }

    pub fn get_model_preferences(&self) -> Value {
        json!({ "preferences": { "globalDefaultModelId": self.store.kv_get("default_model") } })
    }

    pub async fn set_model_preferences(&self, model_id: &str) -> ApiResult<Value> {
        if model_id.trim().is_empty() {
            return Err(ApiError::new("MODEL_NOT_FOUND", "modelId required"));
        }
        self.astore
            .kv_put("default_model", model_id)
            .await
            .map_err(|e| ApiError::store(format!("failed to persist model preference: {e}")))?;
        Ok(json!({ "ok": true, "globalDefaultModelId": model_id }))
    }
}
