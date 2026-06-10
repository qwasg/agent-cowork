"""思考 / 推理（thinking）能力检测。

参考 Proma ``packages/core/src/providers/thinking-capability.ts`` 移植。

不同供应商对「思考模式」的协议要求差异很大，尤其国内模型：

- DeepSeek v4 系列：``output_config.effort = 'max'`` 开启思考，关闭时必须显式
  发送 ``thinking: disabled``，否则报「thinking must be passed back」。
- DeepSeek v3 / reasoner：旧 manual 协议（``reasoning`` / ``reasoning_content``）。
- Kimi / MiniMax：默认不发 thinking 字段（兼容性差异较大）。
- 智谱 GLM：通过 ``thinking={"type": "enabled"|"disabled"}`` 控制。
- 通义千问：通过 ``enable_thinking`` 布尔开关控制。

本模块只做「按模型 ID + 供应商类型推断协议」，具体请求体由各适配器消费。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from src.agent_debug.provider.channels import ProviderType

ThinkingMode = Literal[
    "adaptive-only",
    "adaptive-preferred",
    "manual-only",
    "effort-based-max",
    "qwen-enable-flag",
    "glm-thinking-flag",
    "none",
]

ThinkingDisableStrategy = Literal["explicit-disabled", "omit-field"]


@dataclass(frozen=True)
class ThinkingCapability:
    mode: ThinkingMode
    disable_strategy: ThinkingDisableStrategy

    @property
    def supports_thinking(self) -> bool:
        return self.mode != "none"


def _starts_with(model_id: str, prefix: str) -> bool:
    mid = (model_id or "").lower()
    return mid == prefix or mid.startswith(f"{prefix}-")


def _contains(model_id: str, needle: str) -> bool:
    return needle in (model_id or "").lower()


def detect_thinking_capability(provider: ProviderType, model_id: str) -> ThinkingCapability:
    """根据供应商与模型 ID 推断思考协议能力。

    匹配优先级：先按模型 ID 命中特定系列（避免历史渠道 providerType 配置不准），
    再按 providerType 兜底。
    """
    model_id = model_id or ""

    # DeepSeek v4：effort-based，关闭思考需显式 disabled。
    if _starts_with(model_id, "deepseek-v4") or _contains(model_id, "deepseek-v4"):
        return ThinkingCapability("effort-based-max", "explicit-disabled")

    # DeepSeek reasoner / v3：manual 推理协议。
    if provider == "deepseek" or _contains(model_id, "deepseek-reasoner"):
        return ThinkingCapability("manual-only", "explicit-disabled")

    # 通义千问：enable_thinking 开关（仅部分模型支持，默认按可开启处理）。
    if provider == "qwen" or _starts_with(model_id, "qwen") or _starts_with(model_id, "qwq"):
        return ThinkingCapability("qwen-enable-flag", "omit-field")

    # 智谱 GLM：thinking={"type": ...}
    if provider == "zhipu" or _starts_with(model_id, "glm"):
        return ThinkingCapability("glm-thinking-flag", "explicit-disabled")

    # Kimi / MiniMax 的 Anthropic 渠道：直接省略 thinking 字段保持连接稳定。
    if provider in ("kimi-api", "kimi-coding", "minimax"):
        return ThinkingCapability("none", "omit-field")

    if provider == "anthropic":
        # Claude 系列：4.6+ 支持 adaptive；这里保守地默认 adaptive-preferred。
        if _starts_with(model_id, "claude-mythos-preview"):
            return ThinkingCapability("adaptive-only", "omit-field")
        if _contains(model_id, "opus-4-7") or _contains(model_id, "opus-4.7"):
            return ThinkingCapability("adaptive-only", "omit-field")
        if (
            _contains(model_id, "sonnet-4-6")
            or _contains(model_id, "opus-4-6")
            or _contains(model_id, "sonnet-4.6")
            or _contains(model_id, "opus-4.6")
        ):
            return ThinkingCapability("adaptive-preferred", "explicit-disabled")
        return ThinkingCapability("manual-only", "explicit-disabled")

    # 其它（含 doubao / custom / openai）：默认不发 thinking。
    return ThinkingCapability("none", "omit-field")


def apply_thinking_to_openai_request(
    request: dict,
    capability: ThinkingCapability,
    *,
    enabled: bool,
) -> dict:
    """把思考配置写入 OpenAI 兼容请求体（就地修改并返回）。

    仅处理国内 OpenAI 兼容厂商的开关字段，未知模式保持请求不变。
    """
    if capability.mode == "qwen-enable-flag":
        extra = request.setdefault("extra_body", {})
        extra["enable_thinking"] = bool(enabled)
    elif capability.mode == "glm-thinking-flag":
        request["thinking"] = {"type": "enabled" if enabled else "disabled"}
    elif capability.mode == "effort-based-max":
        # DeepSeek v4：开启 -> effort=max；关闭 -> 显式 disabled。
        if enabled:
            extra = request.setdefault("extra_body", {})
            extra.setdefault("output_config", {})["effort"] = "max"
        else:
            request["thinking"] = {"type": "disabled"}
    return request
