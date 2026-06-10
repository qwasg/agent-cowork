"""Anthropic Messages 协议适配器（``/v1/messages``）。

服务于走 Anthropic 协议的供应商：Anthropic 原生、Kimi API / Coding、
MiniMax（Anthropic 兼容端点）等。

核心职责是把后端内部统一的「OpenAI 风格」消息与工具结构，双向翻译为
Anthropic 的 content block 结构：

- 入：``messages``（role + content / tool_calls / tool role）→ Anthropic blocks，
  ``system`` 抽到顶层，``tools`` 转成 ``{name, description, input_schema}``。
- 出：Anthropic ``content``（text / tool_use / thinking）→ 文本 + ToolCall + reasoning。

国内 Anthropic 渠道（Kimi / MiniMax）默认不发 thinking 字段，详见
``thinking_capability``。
"""

from __future__ import annotations

import json
import os
from typing import Any, AsyncIterator, Dict, List, Optional, Tuple

from src.agent_debug.provider.base import (
    LLMProvider,
    ModelRequestContext,
    ProviderResponse,
    ToolCall,
)
from src.agent_debug.provider.channels import ProviderType, default_base_url
from src.agent_debug.provider.thinking_capability import detect_thinking_capability

try:
    from anthropic import AsyncAnthropic
except ImportError:  # pragma: no cover - optional dependency
    AsyncAnthropic = None  # type: ignore[assignment]

# thinking 回传开关：默认关闭（缺少 signature 时部分端点会报错）；
# 对要求「thinking must be passed back」的供应商可置 1 开启。
_THINKING_PASSTHROUGH = os.getenv("AGENT_DEBUG_THINKING_PASSTHROUGH", "0").strip().lower() in (
    "1",
    "true",
    "yes",
    "on",
)


def _convert_tools(openai_tools: Any) -> List[Dict[str, Any]]:
    """OpenAI function tools → Anthropic tools。"""
    out: List[Dict[str, Any]] = []
    if not isinstance(openai_tools, list):
        return out
    for tool in openai_tools:
        if not isinstance(tool, dict):
            continue
        fn = tool.get("function") if "function" in tool else tool
        if not isinstance(fn, dict):
            continue
        name = fn.get("name")
        if not name:
            continue
        out.append(
            {
                "name": name,
                "description": fn.get("description", ""),
                "input_schema": fn.get("parameters") or {"type": "object", "properties": {}},
            }
        )
    return out


def _convert_messages(messages: List[Dict[str, Any]]) -> Tuple[Optional[str], List[Dict[str, Any]]]:
    """OpenAI messages → (system, anthropic_messages)。"""
    system_parts: List[str] = []
    converted: List[Dict[str, Any]] = []

    for msg in messages:
        role = msg.get("role")
        content = msg.get("content")

        if role == "system":
            if isinstance(content, str) and content.strip():
                system_parts.append(content)
            continue

        if role == "tool":
            # tool 结果 → user 角色下的 tool_result block。
            block = {
                "type": "tool_result",
                "tool_use_id": msg.get("tool_call_id") or msg.get("name") or "",
                "content": content if isinstance(content, str) else json.dumps(content, ensure_ascii=False),
            }
            converted.append({"role": "user", "content": [block]})
            continue

        if role == "assistant":
            blocks: List[Dict[str, Any]] = []
            # thinking 回传（部分供应商如 DeepSeek 要求多轮把 thinking 带回）。
            if _THINKING_PASSTHROUGH and isinstance(msg.get("reasoning"), str) and msg["reasoning"].strip():
                blocks.append({"type": "thinking", "thinking": msg["reasoning"]})
            if isinstance(content, str) and content.strip():
                blocks.append({"type": "text", "text": content})
            for call in msg.get("tool_calls") or []:
                fn = call.get("function") or {}
                args = fn.get("arguments")
                if isinstance(args, str):
                    try:
                        args = json.loads(args) if args.strip() else {}
                    except json.JSONDecodeError:
                        args = {"_raw": args}
                blocks.append(
                    {
                        "type": "tool_use",
                        "id": call.get("id") or fn.get("name") or "",
                        "name": fn.get("name") or "",
                        "input": args or {},
                    }
                )
            if not blocks:
                blocks.append({"type": "text", "text": ""})
            converted.append({"role": "assistant", "content": blocks})
            continue

        # 默认按 user 处理。
        if isinstance(content, str):
            converted.append({"role": "user", "content": content})
        else:
            converted.append({"role": "user", "content": content or ""})

    system = "\n\n".join(system_parts) if system_parts else None
    return system, converted


def _parse_response_content(blocks: Any) -> Tuple[str, List[ToolCall], Optional[str]]:
    text_parts: List[str] = []
    reasoning_parts: List[str] = []
    tool_calls: List[ToolCall] = []
    if not isinstance(blocks, list):
        return "", [], None
    for block in blocks:
        if not isinstance(block, dict):
            continue
        btype = block.get("type")
        if btype == "text" and isinstance(block.get("text"), str):
            text_parts.append(block["text"])
        elif btype in ("thinking", "redacted_thinking"):
            thinking = block.get("thinking") or block.get("text")
            if isinstance(thinking, str):
                reasoning_parts.append(thinking)
        elif btype == "tool_use":
            tool_calls.append(
                ToolCall(
                    id=str(block.get("id") or block.get("name") or ""),
                    name=str(block.get("name") or ""),
                    arguments=block.get("input") if isinstance(block.get("input"), dict) else {},
                )
            )
    return "\n".join(text_parts), tool_calls, ("\n".join(reasoning_parts) or None)


class AnthropicProvider(LLMProvider):
    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        provider_type: ProviderType = "anthropic",
        channel_id: str | None = None,
        default_model: str | None = None,
        auth_token: str | None = None,
        extra_headers: Optional[Dict[str, str]] = None,
        max_tokens: int = 4096,
    ) -> None:
        resolved_api_key = api_key or os.getenv("ANTHROPIC_API_KEY")
        resolved_base_url = base_url or os.getenv("ANTHROPIC_BASE_URL") or default_base_url(provider_type) or None
        self._provider_type: ProviderType = provider_type
        self._provider_name = provider_type
        self._channel_id = channel_id
        self._base_url = resolved_base_url
        self._default_model = default_model
        self._max_tokens = max_tokens
        self._extra_headers = extra_headers or {}

        client_kwargs: Dict[str, Any] = {}
        if resolved_base_url:
            client_kwargs["base_url"] = resolved_base_url
        if self._extra_headers:
            client_kwargs["default_headers"] = self._extra_headers
        # kimi-coding 等使用 auth_token 而非 api_key。
        if auth_token:
            client_kwargs["auth_token"] = auth_token
        else:
            client_kwargs["api_key"] = resolved_api_key

        self._client = (
            AsyncAnthropic(**client_kwargs)
            if AsyncAnthropic and (resolved_api_key or auth_token)
            else None
        )

    def _resolve_model(self, ctx: ModelRequestContext) -> str:
        return ctx.model or self._default_model or "claude-sonnet-4-5"

    def _build_kwargs(self, request: Dict[str, Any], ctx: ModelRequestContext) -> Dict[str, Any]:
        system, messages = _convert_messages(request.get("messages", []))
        kwargs: Dict[str, Any] = {
            "model": self._resolve_model(ctx),
            "messages": messages,
            "max_tokens": request.get("max_tokens") or self._max_tokens,
        }
        if system:
            kwargs["system"] = system
        if request.get("temperature") is not None:
            kwargs["temperature"] = request["temperature"]
        tools = _convert_tools(request.get("tools"))
        if tools:
            kwargs["tools"] = tools

        capability = detect_thinking_capability(self._provider_type, kwargs["model"])
        if capability.mode == "none" and capability.disable_strategy == "omit-field":
            pass  # 国内 Anthropic 渠道（Kimi/MiniMax）：不发 thinking。
        return kwargs

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        if self._client is None:
            raise RuntimeError(f"{self._provider_name} provider is not configured")
        kwargs = self._build_kwargs(request, ctx)
        message = await self._client.messages.create(**kwargs)
        data = message.model_dump() if hasattr(message, "model_dump") else message
        text, tool_calls, reasoning = _parse_response_content(data.get("content"))
        usage = data.get("usage") or {}
        return ProviderResponse(
            provider=self._provider_name,
            model=data.get("model") or kwargs["model"],
            output=text or data,
            token_usage={
                "input": usage.get("input_tokens", 0),
                "output": usage.get("output_tokens", 0),
            },
            finish_reason=data.get("stop_reason"),
            tool_calls=tool_calls,
            reasoning=reasoning,
            provider_type=self._provider_type,
            channel_id=self._channel_id,
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return await self.chat(request, ctx)

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        """统一协议流式事件（与 OpenAI 适配器一致）。"""
        if self._client is None:
            raise RuntimeError(f"{self._provider_name} provider is not configured")
        kwargs = self._build_kwargs(request, ctx)
        async with self._client.messages.stream(**kwargs) as stream:
            async for event in stream:
                etype = getattr(event, "type", None)
                if etype == "content_block_start":
                    block = getattr(event, "content_block", None)
                    if getattr(block, "type", None) == "tool_use":
                        yield {
                            "type": "tool_call",
                            "index": getattr(event, "index", 0),
                            "id": getattr(block, "id", None),
                            "name": getattr(block, "name", None),
                            "arguments_delta": "",
                        }
                elif etype == "content_block_delta":
                    delta = getattr(event, "delta", None)
                    dtype = getattr(delta, "type", None)
                    if dtype == "text_delta":
                        yield {"type": "text", "text": getattr(delta, "text", "")}
                    elif dtype == "thinking_delta":
                        yield {"type": "reasoning", "text": getattr(delta, "thinking", "")}
                    elif dtype == "input_json_delta":
                        yield {
                            "type": "tool_call",
                            "index": getattr(event, "index", 0),
                            "id": None,
                            "name": None,
                            "arguments_delta": getattr(delta, "partial_json", "") or "",
                        }
                elif etype == "message_stop":
                    yield {"type": "done", "finish_reason": "stop"}

    async def healthcheck(self) -> Dict[str, Any]:
        if self._client is None:
            return {"ok": False, "latencyMs": 0, "reason": "missing anthropic api key or package"}
        return {"ok": True, "latencyMs": 0, "baseUrl": self._base_url, "provider": self._provider_name}
