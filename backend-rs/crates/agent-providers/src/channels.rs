//! Multi-channel provider catalog (OpenAI + Chinese LLM vendors), mirroring
//! `provider/channels.py`. Each channel carries a wire protocol and default
//! base URL; secrets are stored encrypted via `CryptoStore`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    OpenAI,
    Anthropic,
    Google,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTypeInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub protocol: Protocol,
    pub default_base_url: &'static str,
}

pub const PROVIDER_TYPES: &[ProviderTypeInfo] = &[
    ProviderTypeInfo {
        id: "openai",
        label: "OpenAI",
        protocol: Protocol::OpenAI,
        default_base_url: "https://api.openai.com/v1",
    },
    ProviderTypeInfo {
        id: "anthropic",
        label: "Anthropic",
        protocol: Protocol::Anthropic,
        default_base_url: "https://api.anthropic.com",
    },
    ProviderTypeInfo {
        id: "deepseek",
        label: "DeepSeek",
        protocol: Protocol::OpenAI,
        default_base_url: "https://api.deepseek.com/v1",
    },
    ProviderTypeInfo {
        id: "google",
        label: "Google Gemini",
        protocol: Protocol::Google,
        default_base_url: "https://generativelanguage.googleapis.com",
    },
    ProviderTypeInfo {
        id: "kimi",
        label: "Kimi API",
        protocol: Protocol::Anthropic,
        default_base_url: "https://api.moonshot.cn/anthropic",
    },
    ProviderTypeInfo {
        id: "kimi_coding",
        label: "Kimi Coding",
        protocol: Protocol::Anthropic,
        default_base_url: "https://api.kimi.com/coding/v1",
    },
    ProviderTypeInfo {
        id: "zhipu",
        label: "智谱 GLM",
        protocol: Protocol::OpenAI,
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
    },
    ProviderTypeInfo {
        id: "minimax",
        label: "MiniMax",
        protocol: Protocol::Anthropic,
        default_base_url: "https://api.minimaxi.com/anthropic",
    },
    ProviderTypeInfo {
        id: "doubao",
        label: "豆包 (火山方舟)",
        protocol: Protocol::OpenAI,
        default_base_url: "https://ark.cn-beijing.volces.com/api/v3",
    },
    ProviderTypeInfo {
        id: "qwen",
        label: "通义千问",
        protocol: Protocol::OpenAI,
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    },
    ProviderTypeInfo {
        id: "openai_compatible",
        label: "OpenAI 兼容 (自定义)",
        protocol: Protocol::OpenAI,
        default_base_url: "",
    },
];

pub fn provider_type(id: &str) -> Option<&'static ProviderTypeInfo> {
    PROVIDER_TYPES.iter().find(|p| p.id == id)
}

/// A user-configured channel (one vendor account). API key persisted encrypted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub provider_type: String,
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    /// Stored as `enc:...` ciphertext. Public API responses must be projected
    /// through the gateway so this encrypted value is never sent to clients.
    #[serde(default)]
    pub api_key_enc: String,
    #[serde(default)]
    pub models: Vec<ChannelModel>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelModel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub supports_reasoning: bool,
}

impl Channel {
    /// Whether the channel has a stored credential (without revealing it).
    pub fn has_key(&self) -> bool {
        !self.api_key_enc.is_empty()
    }
}
