"""OpenAI 兼容协议适配器（含国内厂商特化）。

服务于走 ``/chat/completions`` 的供应商：OpenAI、DeepSeek (v1)、通义千问、
智谱 GLM、豆包、MiniMax 以及自定义 OpenAI 兼容端点。

相对旧 ``openai_provider.py`` 的增强：

- 渠道化：携带 ``provider_type`` / ``channel_id`` / ``base_url`` / 默认模型。
- 思考能力：按 ``thinking_capability`` 注入 ``enable_thinking`` / ``thinking`` /
  ``output_config.effort`` 等字段，并回收 ``reasoning_content``。
- 真流式：``stream()`` 产出归一化 token / reasoning / tool-call delta。
- 超时：尊重 ``ctx.timeout_ms``（由上层 retry 层统一 ``wait_for`` 包裹）。
"""

from __future__ import annotations

import json
import os
from typing import Any, AsyncIterator, Dict, List, Optional

from src.agent_debug.provider.base import (
    LLMProvider,
    ModelRequestContext,
    ProviderResponse,
    ToolCall,
)
from src.agent_debug.provider.channels import ProviderType, default_base_url
from src.agent_debug.provider.thinking_capability import (
    apply_thinking_to_openai_request,
    detect_thinking_capability,
)

try:
    from openai import AsyncOpenAI
except ImportError:  # pragma: no cover - optional dependency
    AsyncOpenAI = None  # type: ignore[assignment]


def _parse_tool_call_arguments(raw: Any) -> Dict[str, Any]:
    if isinstance(raw, dict):
        return raw
    if not isinstance(raw, str) or not raw.strip():
        return {}
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return {"_raw": raw}
    return parsed if isinstance(parsed, dict) else {"_value": parsed}


def _normalise_tool_calls(raw_calls: Any) -> List[ToolCall]:
    out: List[ToolCall] = []
    if not isinstance(raw_calls, list):
        return out
    for call in raw_calls:
        if not isinstance(call, dict):
            continue
        function = call.get("function") or {}
        if not isinstance(function, dict):
            continue
        name = str(function.get("name") or "").strip()
        if not name:
            continue
        out.append(
            ToolCall(
                id=str(call.get("id") or name),
                name=name,
                arguments=_parse_tool_call_arguments(function.get("arguments")),
            )
        )
    return out


# 部分厂商不接受 ``temperature`` 之外的某些标准参数，这里集中维护需要剥离的字段。
_UNSUPPORTED_PARAMS: Dict[str, set[str]] = {
    # 通义千问 enable_thinking 模式下不支持 temperature 自定义部分场景，保守不剥离。
}


class OpenAICompatibleProvider(LLMProvider):
    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        provider_type: ProviderType = "custom",
        channel_id: str | None = None,
        default_model: str | None = None,
        thinking_enabled: bool = True,
        extra_headers: Optional[Dict[str, str]] = None,
        timeout_seconds: float | None = None,
    ) -> None:
        resolved_api_key = api_key or os.getenv("OPENAI_API_KEY")
        resolved_base_url = base_url or os.getenv("OPENAI_BASE_URL") or default_base_url(provider_type) or None
        self._provider_type: ProviderType = provider_type
        self._provider_name = provider_type if provider_type != "custom" else "openai-compatible"
        self._channel_id = channel_id
        self._base_url = resolved_base_url
        self._default_model = default_model
        self._thinking_enabled = thinking_enabled
        self._extra_headers = extra_headers or {}
        self._timeout_seconds = timeout_seconds
        self._client = (
            AsyncOpenAI(
                api_key=resolved_api_key,
                base_url=resolved_base_url,
                default_headers=self._extra_headers or None,
            )
            if AsyncOpenAI and resolved_api_key
            else None
        )

    # ----------------------------------------------------------------- helpers
    def _resolve_model(self, ctx: ModelRequestContext) -> str:
        return ctx.model or self._default_model or "gpt-4o-mini"

    def _build_kwargs(self, request: Dict[str, Any], ctx: ModelRequestContext) -> Dict[str, Any]:
        model = self._resolve_model(ctx)
        kwargs: Dict[str, Any] = {
            "model": model,
            "messages": request.get("messages", []),
        }
        tools = request.get("tools")
        if isinstance(tools, list) and tools:
            kwargs["tools"] = tools
            tool_choice = request.get("tool_choice", "auto")
            if tool_choice is not None:
                kwargs["tool_choice"] = tool_choice
        if request.get("temperature") is not None:
            kwargs["temperature"] = request["temperature"]
        if request.get("max_tokens") is not None:
            kwargs["max_tokens"] = request["max_tokens"]

        # 思考能力特化：写入 enable_thinking / thinking / output_config 等。
        capability = detect_thinking_capability(self._provider_type, model)
        enabled = self._thinking_enabled and request.get("thinking_enabled", True)
        if capability.supports_thinking or capability.mode == "effort-based-max":
            applied = apply_thinking_to_openai_request({}, capability, enabled=bool(enabled))
            for key, value in applied.items():
                if key == "extra_body":
                    kwargs.setdefault("extra_body", {}).update(value)
                else:
                    kwargs[key] = value
        return kwargs

    @staticmethod
    def _extract_reasoning(message: Dict[str, Any]) -> Optional[str]:
        for key in ("reasoning_content", "reasoning"):
            val = message.get(key)
            if isinstance(val, str) and val.strip():
                return val
        return None

    # -------------------------------------------------------------------- chat
    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        if self._client is None:
            raise RuntimeError(f"{self._provider_name} provider is not configured")

        kwargs = self._build_kwargs(request, ctx)
        kwargs["stream"] = False
        completion = await self._client.chat.completions.create(**kwargs)
        choice = completion.choices[0]
        raw = choice.message.model_dump() if hasattr(choice.message, "model_dump") else choice.message
        from src.agent_debug.provider.service import _try_extract_text_output  # 避免循环导入

        tool_calls = _normalise_tool_calls(raw.get("tool_calls") if isinstance(raw, dict) else None)
        reasoning = self._extract_reasoning(raw) if isinstance(raw, dict) else None

        text = _try_extract_text_output(raw) if isinstance(raw, dict) else _try_extract_text_output(str(raw))
        normalized_out: Any = text.strip() if (text and text.strip()) else raw
        return ProviderResponse(
            provider=self._provider_name,
            model=completion.model,
            output=normalized_out,
            token_usage={
                "input": getattr(completion.usage, "prompt_tokens", 0) if completion.usage else 0,
                "output": getattr(completion.usage, "completion_tokens", 0) if completion.usage else 0,
            },
            finish_reason=choice.finish_reason,
            tool_calls=tool_calls,
            reasoning=reasoning,
            provider_type=self._provider_type,
            channel_id=self._channel_id,
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        if self._client is None:
            raise RuntimeError(f"{self._provider_name} provider is not configured")
        response = await self._client.responses.create(model=self._resolve_model(ctx), **request)
        return ProviderResponse(
            provider=self._provider_name,
            model=self._resolve_model(ctx),
            output=response.model_dump() if hasattr(response, "model_dump") else response,
            token_usage={},
            finish_reason="stop",
            provider_type=self._provider_type,
            channel_id=self._channel_id,
        )

    # ------------------------------------------------------------------ stream
    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        """产出归一化流式事件（统一协议）。

        - ``{"type": "text", "text": str}``
        - ``{"type": "reasoning", "text": str}``
        - ``{"type": "tool_call", "index": int, "id": str|None, "name": str|None, "arguments_delta": str}``
        - ``{"type": "done", "finish_reason": str}``
        """
        if self._client is None:
            raise RuntimeError(f"{self._provider_name} provider is not configured")

        kwargs = self._build_kwargs(request, ctx)
        kwargs["stream"] = True
        kwargs["stream_options"] = {"include_usage": True}

        stream = await self._client.chat.completions.create(**kwargs)
        async for chunk in stream:
            data = chunk.model_dump() if hasattr(chunk, "model_dump") else chunk
            choices = data.get("choices") if isinstance(data, dict) else None
            if not choices:
                continue
            delta = choices[0].get("delta") or {}
            content = delta.get("content")
            if isinstance(content, str) and content:
                yield {"type": "text", "text": content}
            reasoning = self._extract_reasoning(delta)
            if reasoning:
                yield {"type": "reasoning", "text": reasoning}
            for tc in delta.get("tool_calls") or []:
                fn = tc.get("function") or {}
                yield {
                    "type": "tool_call",
                    "index": tc.get("index", 0),
                    "id": tc.get("id"),
                    "name": fn.get("name"),
                    "arguments_delta": fn.get("arguments") or "",
                }
            finish = choices[0].get("finish_reason")
            if finish:
                yield {"type": "done", "finish_reason": finish}

    async def healthcheck(self) -> Dict[str, Any]:
        if self._client is None:
            return {"ok": False, "latencyMs": 0, "reason": "missing api key or openai package"}
        try:
            models = await self._client.models.list()
        except Exception as exc:  # pragma: no cover - provider-specific
            return {
                "ok": True,
                "latencyMs": 0,
                "baseUrl": self._base_url,
                "provider": self._provider_name,
                "listModels": "unavailable",
                "reason": str(exc),
            }
        return {"ok": bool(models.data), "latencyMs": 0, "baseUrl": self._base_url, "provider": self._provider_name}
