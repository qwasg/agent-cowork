//! Thinking / reasoning capability detection (port of
//! `provider/thinking_capability.py`, itself ported from Proma's
//! `thinking-capability.ts`).
//!
//! Different vendors require very different "thinking mode" request fields,
//! especially Chinese providers:
//! - DeepSeek v4: `output_config.effort = "max"` enables thinking; disabling
//!   requires an explicit `thinking: {type: "disabled"}`.
//! - DeepSeek v3 / reasoner: legacy manual protocol (`reasoning_content`).
//! - Kimi / MiniMax: omit the thinking field entirely.
//! - Zhipu GLM: `thinking: {"type": "enabled"|"disabled"}`.
//! - Qwen: boolean `enable_thinking` switch.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    AdaptiveOnly,
    AdaptivePreferred,
    ManualOnly,
    EffortBasedMax,
    QwenEnableFlag,
    GlmThinkingFlag,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisableStrategy {
    ExplicitDisabled,
    OmitField,
}

#[derive(Debug, Clone, Copy)]
pub struct ThinkingCapability {
    pub mode: ThinkingMode,
    pub disable_strategy: DisableStrategy,
}

impl ThinkingCapability {
    pub fn supports_thinking(&self) -> bool {
        self.mode != ThinkingMode::None
    }
}

fn starts_with(model_id: &str, prefix: &str) -> bool {
    let mid = model_id.to_lowercase();
    mid == prefix || mid.starts_with(&format!("{prefix}-"))
}

fn contains(model_id: &str, needle: &str) -> bool {
    model_id.to_lowercase().contains(needle)
}

/// Infer the thinking protocol from provider type + model id. Model-id
/// patterns win over the provider type (channel configs are often imprecise).
pub fn detect_thinking_capability(provider: &str, model_id: &str) -> ThinkingCapability {
    // DeepSeek v4: effort-based; disabling must be explicit.
    if starts_with(model_id, "deepseek-v4") || contains(model_id, "deepseek-v4") {
        return ThinkingCapability {
            mode: ThinkingMode::EffortBasedMax,
            disable_strategy: DisableStrategy::ExplicitDisabled,
        };
    }
    // DeepSeek reasoner / v3: manual reasoning protocol.
    if provider == "deepseek" || contains(model_id, "deepseek-reasoner") {
        return ThinkingCapability {
            mode: ThinkingMode::ManualOnly,
            disable_strategy: DisableStrategy::ExplicitDisabled,
        };
    }
    // Qwen: enable_thinking switch.
    if provider == "qwen" || starts_with(model_id, "qwen") || starts_with(model_id, "qwq") {
        return ThinkingCapability {
            mode: ThinkingMode::QwenEnableFlag,
            disable_strategy: DisableStrategy::OmitField,
        };
    }
    // Zhipu GLM: thinking={"type": ...}
    if provider == "zhipu" || starts_with(model_id, "glm") {
        return ThinkingCapability {
            mode: ThinkingMode::GlmThinkingFlag,
            disable_strategy: DisableStrategy::ExplicitDisabled,
        };
    }
    // Kimi / MiniMax Anthropic channels: omit the field for stability.
    if matches!(
        provider,
        "kimi" | "kimi_coding" | "kimi-api" | "kimi-coding" | "minimax"
    ) {
        return ThinkingCapability {
            mode: ThinkingMode::None,
            disable_strategy: DisableStrategy::OmitField,
        };
    }
    if provider == "anthropic" {
        if starts_with(model_id, "claude-mythos-preview")
            || contains(model_id, "opus-4-7")
            || contains(model_id, "opus-4.7")
        {
            return ThinkingCapability {
                mode: ThinkingMode::AdaptiveOnly,
                disable_strategy: DisableStrategy::OmitField,
            };
        }
        if contains(model_id, "sonnet-4-6")
            || contains(model_id, "opus-4-6")
            || contains(model_id, "sonnet-4.6")
            || contains(model_id, "opus-4.6")
        {
            return ThinkingCapability {
                mode: ThinkingMode::AdaptivePreferred,
                disable_strategy: DisableStrategy::ExplicitDisabled,
            };
        }
        return ThinkingCapability {
            mode: ThinkingMode::ManualOnly,
            disable_strategy: DisableStrategy::ExplicitDisabled,
        };
    }
    // Everything else (doubao / custom / openai): no thinking field.
    ThinkingCapability {
        mode: ThinkingMode::None,
        disable_strategy: DisableStrategy::OmitField,
    }
}

/// Write the thinking configuration into an OpenAI-compatible request body
/// (in place). Unknown modes leave the request untouched.
pub fn apply_thinking_to_openai_request(
    body: &mut Value,
    capability: ThinkingCapability,
    enabled: bool,
) {
    match capability.mode {
        ThinkingMode::QwenEnableFlag => {
            body["enable_thinking"] = json!(enabled);
        }
        ThinkingMode::GlmThinkingFlag => {
            body["thinking"] = json!({ "type": if enabled { "enabled" } else { "disabled" } });
        }
        ThinkingMode::EffortBasedMax => {
            if enabled {
                body["output_config"] = json!({ "effort": "max" });
            } else {
                body["thinking"] = json!({ "type": "disabled" });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vendor_protocols() {
        assert_eq!(
            detect_thinking_capability("openai_compatible", "deepseek-v4").mode,
            ThinkingMode::EffortBasedMax
        );
        assert_eq!(
            detect_thinking_capability("deepseek", "deepseek-chat").mode,
            ThinkingMode::ManualOnly
        );
        assert_eq!(
            detect_thinking_capability("qwen", "qwen-max").mode,
            ThinkingMode::QwenEnableFlag
        );
        assert_eq!(
            detect_thinking_capability("zhipu", "glm-4.7").mode,
            ThinkingMode::GlmThinkingFlag
        );
        assert_eq!(
            detect_thinking_capability("minimax", "minimax-m2").mode,
            ThinkingMode::None
        );
        assert_eq!(
            detect_thinking_capability("anthropic", "claude-sonnet-4-6").mode,
            ThinkingMode::AdaptivePreferred
        );
        assert_eq!(
            detect_thinking_capability("openai", "gpt-4o").mode,
            ThinkingMode::None
        );
    }

    #[test]
    fn applies_request_fields() {
        let cap = detect_thinking_capability("qwen", "qwen-max");
        let mut body = json!({"model": "qwen-max"});
        apply_thinking_to_openai_request(&mut body, cap, true);
        assert_eq!(body["enable_thinking"], json!(true));

        let cap = detect_thinking_capability("zhipu", "glm-4.7");
        let mut body = json!({});
        apply_thinking_to_openai_request(&mut body, cap, false);
        assert_eq!(body["thinking"]["type"], "disabled");

        let cap = detect_thinking_capability("x", "deepseek-v4");
        let mut body = json!({});
        apply_thinking_to_openai_request(&mut body, cap, true);
        assert_eq!(body["output_config"]["effort"], "max");
        let mut body = json!({});
        apply_thinking_to_openai_request(&mut body, cap, false);
        assert_eq!(body["thinking"]["type"], "disabled");

        // OpenAI: untouched.
        let cap = detect_thinking_capability("openai", "gpt-4o");
        let mut body = json!({"model": "gpt-4o"});
        apply_thinking_to_openai_request(&mut body, cap, true);
        assert_eq!(body, json!({"model": "gpt-4o"}));
    }
}
