//! LLM channel (provider credential) management.

use serde_json::{json, Value};

use agent_protocol::models::{new_id, now_ts};
use agent_protocol::{ApiError, ApiResult};
use agent_providers::channels::{self, Channel, ChannelModel};
use agent_store::store::T_CHANNELS;

use crate::api::AppServices;

impl AppServices {
    pub fn list_provider_types(&self) -> Value {
        json!({ "providerTypes": channels::PROVIDER_TYPES })
    }

    pub fn list_channels(&self) -> Value {
        let list = self.store.list::<Channel>(T_CHANNELS).unwrap_or_default();
        let channels: Vec<Value> = list.iter().map(public_channel).collect();
        json!({ "channels": channels })
    }

    pub async fn upsert_channel(&self, payload: &Value) -> ApiResult<Value> {
        let provider_type = payload
            .get("providerType")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::new("AUTH_INVALID_INPUT", "providerType required"))?;
        let info = channels::provider_type(provider_type)
            .ok_or_else(|| ApiError::new("AUTH_INVALID_INPUT", "unknown providerType"))?;
        let id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| new_id("ch"));
        let existing = self.store.get::<Channel>(T_CHANNELS, &id).ok().flatten();
        let api_key_enc = match payload.get("apiKey").and_then(|v| v.as_str()) {
            Some(k) if !k.is_empty() => self.crypto.encrypt(k),
            _ => existing
                .as_ref()
                .map(|c| c.api_key_enc.clone())
                .unwrap_or_default(),
        };
        let models: Vec<ChannelModel> = payload
            .get("models")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .or_else(|| existing.as_ref().map(|c| c.models.clone()))
            .unwrap_or_default();
        let channel = Channel {
            id: id.clone(),
            provider_type: provider_type.to_string(),
            label: payload
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(info.label)
                .to_string(),
            base_url: payload
                .get("baseUrl")
                .and_then(|v| v.as_str())
                .unwrap_or(info.default_base_url)
                .to_string(),
            api_key_enc,
            models,
            enabled: payload
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            created_at: existing.map(|c| c.created_at).unwrap_or_else(now_ts),
        };
        self.astore
            .put(T_CHANNELS, &id, &channel)
            .await
            .map_err(|e| ApiError::store(format!("failed to persist channel: {e}")))?;
        self.providers.reload(&self.cfg, &self.store, &self.crypto);
        Ok(json!({ "channel": public_channel(&channel) }))
    }

    pub async fn delete_channel(&self, id: &str) -> ApiResult<Value> {
        let existed = self.astore.delete(T_CHANNELS, id).await.unwrap_or(false);
        if !existed {
            return Err(ApiError::new("MODEL_NOT_FOUND", "channel not found"));
        }
        self.providers.reload(&self.cfg, &self.store, &self.crypto);
        Ok(json!({ "ok": true }))
    }

    pub async fn fetch_channel_models(&self, payload: &Value) -> ApiResult<Value> {
        // Best-effort: query the OpenAI-compatible /models endpoint.
        let base = payload
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let key = payload.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
        if base.is_empty() {
            return Ok(json!({ "models": [] }));
        }
        let url = format!("{}/models", base.trim_end_matches('/'));
        let resp = agent_store::http::shared_client()
            .get(url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(key)
            .send()
            .await;
        match resp {
            Ok(r) => {
                let body: Value = r.json().await.unwrap_or(json!({}));
                let models: Vec<Value> = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                            .map(|id| json!({ "id": id, "label": id }))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(json!({ "models": models }))
            }
            Err(e) => Err(ApiError::new("PROVIDER_HTTP_ERROR", e.to_string())),
        }
    }
}

fn public_channel(channel: &Channel) -> Value {
    json!({
        "id": channel.id,
        "providerType": channel.provider_type,
        "label": channel.label,
        "baseUrl": channel.base_url,
        "apiKeySet": channel.has_key(),
        "models": channel.models,
        "enabled": channel.enabled,
        "createdAt": channel.created_at,
    })
}
