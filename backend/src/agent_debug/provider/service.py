from __future__ import annotations

import asyncio
import json
import logging
import os
from dataclasses import dataclass
from typing import Any, Callable, Generic, List, Literal, TypeVar

from src.agent_debug.domain.models import DebugEvent
from src.agent_debug.infra.circuit_breaker import CircuitBreaker
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.infra.retry import (
    RetryConfig,
    classify_error,
    compute_backoff_seconds,
    is_retryable_error,
)
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.base import (
    ModelRequestContext,
    ProviderRegistry,
    ProviderResponse,
    ToolCall,
)
from src.agent_debug.provider.fake_provider import FakeProvider
from src.agent_debug.provider.mock_provider import MockProvider
from src.agent_debug.provider.package_agent_provider import PackageAgentProvider


ProviderOperation = Literal[
    "plan_generation",
    "subagent_execution",
    "summary_generation",
    "composer_chat",
    "code_edit_proposal",
]
ProviderErrorCode = Literal["timeout", "rate_limited", "invalid_response", "unavailable"]

T = TypeVar("T")


@dataclass
class ProviderOperationResult(Generic[T]):
    value: T
    response: ProviderResponse
    operation: ProviderOperation
    attempt: int
    fallback: bool
    fallback_from: str | None = None
    fallback_to: str | None = None


class ProviderExecutionError(RuntimeError):
    def __init__(self, error_code: ProviderErrorCode, message: str) -> None:
        super().__init__(message)
        self.error_code = error_code


class ProviderExecutionService:
    def __init__(
        self,
        registry: ProviderRegistry,
        event_bus: EventBus,
        *,
        trace_collector: Any = None,
    ) -> None:
        self.registry = registry
        self.event_bus = event_bus
        # 每个供应商一个熔断器，连续失败达到阈值后短路并触发链路回退。
        self._breakers: dict[str, CircuitBreaker] = {}
        self._trace_collector = trace_collector
        # 按所选模型精确路由：给定 model_id 返回应优先使用的 provider 名称
        # （通常为 ``f"channel:{id}"``）。由上层在 registry 重建时刷新。
        self.model_channel_resolver: Callable[[str], str | None] | None = None

    def _breaker(self, provider_name: str) -> CircuitBreaker:
        breaker = self._breakers.get(provider_name)
        if breaker is None:
            breaker = CircuitBreaker()
            self._breakers[provider_name] = breaker
        return breaker

    async def _invoke_with_resilience(
        self,
        *,
        provider: Any,
        provider_name: str,
        request: dict[str, Any],
        ctx: ModelRequestContext,
        session_id: str,
        operation: ProviderOperation,
        correlation_id: str | None,
        chain_attempt: int,
    ) -> ProviderResponse:
        """对单个供应商做「熔断检查 + 超时 + 退避重试」的弹性调用。"""
        breaker = self._breaker(provider_name)
        breaker.ensure_available()

        policy = ctx.retry_policy
        config = RetryConfig(
            max_attempts=max(1, policy.max_attempts),
            base_delay_seconds=max(0.0, policy.base_delay_ms / 1000.0),
            max_delay_seconds=max(0.05, policy.max_delay_ms / 1000.0),
        )
        timeout_seconds = ctx.timeout_ms / 1000.0 if ctx.timeout_ms and ctx.timeout_ms > 0 else None

        span = None
        if self._trace_collector is not None:
            span = self._trace_collector.start_span(
                ctx.trace_id,
                f"provider.{provider_name}.chat",
                {"operation": operation, "model": ctx.model},
            )

        async def _call() -> ProviderResponse:
            coro = provider.chat(request, ctx)
            if timeout_seconds is not None:
                return await asyncio.wait_for(coro, timeout=timeout_seconds)
            return await coro

        async def _on_retry(attempt: int, exc: Exception, delay: float) -> None:
            await self._publish_event(
                session_id=session_id,
                event_type="provider.request.retry",
                request_id=ctx.request_id,
                correlation_id=correlation_id,
                payload={
                    "requestId": ctx.request_id,
                    "provider": provider_name,
                    "model": ctx.model,
                    "operation": operation,
                    "chainAttempt": chain_attempt,
                    "retryAttempt": attempt,
                    "delaySeconds": round(delay, 3),
                    "errorCode": classify_error(exc),
                    "error": str(exc),
                },
            )

        last_error: Exception | None = None
        try:
            for attempt in range(1, config.max_attempts + 1):
                try:
                    response = await _call()
                    breaker.on_success()
                    if span is not None:
                        self._trace_collector.finish_span(span)
                    return response
                except Exception as exc:  # noqa: BLE001 - 由分类器决定是否重试
                    last_error = exc
                    if attempt >= config.max_attempts or not is_retryable_error(
                        exc, set(config.retryable_codes)
                    ):
                        break
                    delay = compute_backoff_seconds(attempt, config)
                    await _on_retry(attempt, exc, delay)
                    await asyncio.sleep(delay)
            breaker.on_failure()
            if span is not None:
                self._trace_collector.finish_span(span)
            assert last_error is not None
            raise last_error
        except Exception:
            if span is not None and span.ended_at is None:
                self._trace_collector.finish_span(span)
            raise

    async def execute_chat_operation(
        self,
        *,
        request: dict[str, Any],
        ctx: ModelRequestContext,
        session_id: str,
        operation: ProviderOperation,
        parser: Callable[[ProviderResponse], T],
        correlation_id: str | None = None,
        fallback_chain: list[str] | None = None,
    ) -> ProviderOperationResult[T]:
        chain = self._resolve_chain(fallback_chain or ctx.fallback_chain, operation, ctx=ctx)
        last_error_code: ProviderErrorCode = "unavailable"
        last_error_message = "Provider operation failed"

        for attempt, provider_name in enumerate(chain, start=1):
            provider = self.registry.get(provider_name)
            fallback = attempt > 1
            fallback_from = chain[attempt - 2] if fallback else None
            fallback_to = provider_name if fallback else None

            await self._publish_event(
                session_id=session_id,
                event_type="provider.request.started",
                request_id=ctx.request_id,
                correlation_id=correlation_id,
                payload={
                    "requestId": ctx.request_id,
                    "provider": provider_name,
                    "model": ctx.model,
                    "operation": operation,
                    "attempt": attempt,
                    "fallback": fallback,
                    "fallbackFrom": fallback_from,
                    "fallbackTo": fallback_to,
                },
            )

            try:
                response = await self._invoke_with_resilience(
                    provider=provider,
                    provider_name=provider_name,
                    request=request,
                    ctx=ctx,
                    session_id=session_id,
                    operation=operation,
                    correlation_id=correlation_id,
                    chain_attempt=attempt,
                )
            except Exception as exc:
                last_error_code = normalize_provider_error(exc)
                last_error_message = str(exc)
                await self._publish_event(
                    session_id=session_id,
                    event_type="provider.request.failed",
                    request_id=ctx.request_id,
                    correlation_id=correlation_id,
                    payload={
                        "requestId": ctx.request_id,
                        "provider": provider_name,
                        "model": ctx.model,
                        "operation": operation,
                        "attempt": attempt,
                        "fallback": fallback,
                        "fallbackFrom": fallback_from,
                        "fallbackTo": fallback_to,
                        "error": str(exc),
                        "errorCode": last_error_code,
                    },
                )
                continue

            try:
                value = parser(response)
            except Exception as exc:
                last_error_code = "invalid_response"
                last_error_message = str(exc)
                await self._publish_event(
                    session_id=session_id,
                    event_type="provider.request.failed",
                    request_id=ctx.request_id,
                    correlation_id=correlation_id,
                    payload={
                        "requestId": ctx.request_id,
                        "provider": response.provider,
                        "model": response.model,
                        "operation": operation,
                        "attempt": attempt,
                        "fallback": fallback,
                        "fallbackFrom": fallback_from,
                        "fallbackTo": fallback_to,
                        "error": str(exc),
                        "errorCode": "invalid_response",
                    },
                )
                continue

            await self._publish_event(
                session_id=session_id,
                event_type="provider.request.completed",
                request_id=ctx.request_id,
                correlation_id=correlation_id,
                payload={
                    "requestId": ctx.request_id,
                    "provider": response.provider,
                    "model": response.model,
                    "operation": operation,
                    "attempt": attempt,
                    "fallback": fallback,
                    "fallbackFrom": fallback_from,
                    "fallbackTo": fallback_to,
                    "tokenUsage": response.token_usage,
                    "finishReason": response.finish_reason,
                },
            )
            return ProviderOperationResult(
                value=value,
                response=response,
                operation=operation,
                attempt=attempt,
                fallback=fallback,
                fallback_from=fallback_from,
                fallback_to=fallback_to,
            )

        raise ProviderExecutionError(last_error_code, last_error_message)

    async def stream_chat_operation(
        self,
        *,
        request: dict[str, Any],
        ctx: ModelRequestContext,
        session_id: str,
        operation: ProviderOperation,
        parser: Callable[[ProviderResponse], T],
        correlation_id: str | None = None,
        fallback_chain: list[str] | None = None,
        on_delta: Callable[[dict[str, Any]], Any] | None = None,
    ) -> ProviderOperationResult[T]:
        """流式版 ``execute_chat_operation``。

        逐 provider 尝试真流式；若该 provider 未产出任何流事件（如 mock/fake），
        回退到其非流式 ``chat``。整体仍遵循链路回退。``on_delta`` 接收统一协议
        的增量事件（text / reasoning / tool_call / done）。
        """
        chain = self._resolve_chain(fallback_chain or ctx.fallback_chain, operation, ctx=ctx)
        last_error_code: ProviderErrorCode = "unavailable"
        last_error_message = "Provider streaming failed"

        for attempt, provider_name in enumerate(chain, start=1):
            provider = self.registry.get(provider_name)
            fallback = attempt > 1
            fallback_from = chain[attempt - 2] if fallback else None
            fallback_to = provider_name if fallback else None

            await self._publish_event(
                session_id=session_id,
                event_type="provider.request.started",
                request_id=ctx.request_id,
                correlation_id=correlation_id,
                payload={
                    "requestId": ctx.request_id,
                    "provider": provider_name,
                    "model": ctx.model,
                    "operation": operation,
                    "attempt": attempt,
                    "fallback": fallback,
                    "streaming": True,
                },
            )

            try:
                response = await self._stream_collect(
                    provider=provider,
                    provider_name=provider_name,
                    request=request,
                    ctx=ctx,
                    on_delta=on_delta,
                )
                if response is None:
                    # 该 provider 未真正流式，回退到非流式调用。
                    response = await self._invoke_with_resilience(
                        provider=provider,
                        provider_name=provider_name,
                        request=request,
                        ctx=ctx,
                        session_id=session_id,
                        operation=operation,
                        correlation_id=correlation_id,
                        chain_attempt=attempt,
                    )
            except Exception as exc:
                last_error_code = normalize_provider_error(exc)
                last_error_message = str(exc)
                await self._publish_event(
                    session_id=session_id,
                    event_type="provider.request.failed",
                    request_id=ctx.request_id,
                    correlation_id=correlation_id,
                    payload={
                        "requestId": ctx.request_id,
                        "provider": provider_name,
                        "operation": operation,
                        "attempt": attempt,
                        "error": str(exc),
                        "errorCode": last_error_code,
                    },
                )
                continue

            try:
                value = parser(response)
            except Exception as exc:
                last_error_code = "invalid_response"
                last_error_message = str(exc)
                await self._publish_event(
                    session_id=session_id,
                    event_type="provider.request.failed",
                    request_id=ctx.request_id,
                    correlation_id=correlation_id,
                    payload={
                        "requestId": ctx.request_id,
                        "provider": response.provider,
                        "operation": operation,
                        "attempt": attempt,
                        "error": str(exc),
                        "errorCode": "invalid_response",
                    },
                )
                continue

            await self._publish_event(
                session_id=session_id,
                event_type="provider.request.completed",
                request_id=ctx.request_id,
                correlation_id=correlation_id,
                payload={
                    "requestId": ctx.request_id,
                    "provider": response.provider,
                    "model": response.model,
                    "operation": operation,
                    "attempt": attempt,
                    "fallback": fallback,
                    "tokenUsage": response.token_usage,
                    "finishReason": response.finish_reason,
                    "streaming": True,
                },
            )
            return ProviderOperationResult(
                value=value,
                response=response,
                operation=operation,
                attempt=attempt,
                fallback=fallback,
                fallback_from=fallback_from,
                fallback_to=fallback_to,
            )

        raise ProviderExecutionError(last_error_code, last_error_message)

    async def _stream_collect(
        self,
        *,
        provider: Any,
        provider_name: str,
        request: dict[str, Any],
        ctx: ModelRequestContext,
        on_delta: Callable[[dict[str, Any]], Any] | None,
    ) -> ProviderResponse | None:
        """消费 provider.stream() 并归并为一个 ProviderResponse。

        返回 None 表示 provider 未产出任何流事件（应回退到非流式）。
        """
        breaker = self._breaker(provider_name)
        breaker.ensure_available()

        text_parts: list[str] = []
        reasoning_parts: list[str] = []
        tool_acc: dict[int, dict[str, Any]] = {}
        finish_reason: str | None = None
        saw_any = False

        timeout_seconds = ctx.timeout_ms / 1000.0 if ctx.timeout_ms and ctx.timeout_ms > 0 else None

        async def _consume() -> None:
            nonlocal finish_reason, saw_any
            async for event in provider.stream(request, ctx):
                etype = event.get("type")
                # 仅统一协议事件视为「真流式」；旧协议/未知事件忽略，触发非流式回退。
                if etype == "text":
                    saw_any = True
                    chunk = event.get("text") or ""
                    text_parts.append(chunk)
                    if on_delta is not None and chunk:
                        await _maybe_await(on_delta({"type": "text", "text": chunk}))
                elif etype == "reasoning":
                    saw_any = True
                    chunk = event.get("text") or ""
                    reasoning_parts.append(chunk)
                    if on_delta is not None and chunk:
                        await _maybe_await(on_delta({"type": "reasoning", "text": chunk}))
                elif etype == "tool_call":
                    saw_any = True
                    idx = int(event.get("index", 0) or 0)
                    slot = tool_acc.setdefault(idx, {"id": None, "name": None, "args": ""})
                    if event.get("id"):
                        slot["id"] = event["id"]
                    if event.get("name"):
                        slot["name"] = event["name"]
                    slot["args"] += event.get("arguments_delta") or ""
                elif etype == "done":
                    saw_any = True
                    finish_reason = event.get("finish_reason") or finish_reason

        try:
            if timeout_seconds is not None:
                await asyncio.wait_for(_consume(), timeout=timeout_seconds)
            else:
                await _consume()
        except Exception:
            breaker.on_failure()
            raise

        if not saw_any:
            return None

        breaker.on_success()
        tool_calls: list[ToolCall] = []
        for idx in sorted(tool_acc.keys()):
            slot = tool_acc[idx]
            name = (slot.get("name") or "").strip()
            if not name:
                continue
            raw_args = slot.get("args") or ""
            try:
                parsed = json.loads(raw_args) if raw_args.strip() else {}
            except json.JSONDecodeError:
                parsed = {"_raw": raw_args}
            tool_calls.append(
                ToolCall(
                    id=str(slot.get("id") or name),
                    name=name,
                    arguments=parsed if isinstance(parsed, dict) else {"_value": parsed},
                )
            )

        return ProviderResponse(
            provider=getattr(provider, "_provider_name", provider_name),
            model=getattr(provider, "_default_model", None) or ctx.model,
            output="".join(text_parts),
            token_usage={},
            finish_reason=finish_reason,
            tool_calls=tool_calls,
            reasoning="".join(reasoning_parts) or None,
            provider_type=getattr(provider, "_provider_type", None),
            channel_id=getattr(provider, "_channel_id", None),
        )

    def _resolve_chain(
        self,
        requested_chain: list[str] | None,
        operation: ProviderOperation,
        *,
        ctx: ModelRequestContext | None = None,
    ) -> list[str]:
        # 按所选模型精确路由：若该模型归属某个已启用渠道，则将对应
        # ``channel:{id}`` 置于链首，其余仍按既有逻辑追加（去重）以保留回退能力。
        preferred: list[str] = []
        if ctx is not None and self.model_channel_resolver is not None:
            try:
                channel_provider = self.model_channel_resolver(ctx.model)
            except Exception:  # noqa: BLE001 - 解析失败时退回默认链路
                channel_provider = None
            if channel_provider and self.registry.has(channel_provider):
                preferred.append(channel_provider)

        def _prepend_preferred(chain: list[str]) -> list[str]:
            merged = list(preferred)
            for name in chain:
                if name not in merged:
                    merged.append(name)
            return merged

        configured_chain = requested_chain or _read_provider_chain_from_env()
        resolved: list[str] = []
        for name in configured_chain:
            if self.registry.has(name) and name not in resolved:
                resolved.append(name)
        if resolved:
            return _prepend_preferred(resolved)

        # 默认优先级：本地 package-agent > 已配置的中国大模型渠道 > OpenAI 兼容 >
        # mock/fake 兜底。registry.has 会自动过滤未注册项。
        priority = [
            "package-agent",
            "deepseek",
            "qwen",
            "zhipu",
            "kimi-api",
            "kimi-coding",
            "doubao",
            "minimax",
            "anthropic",
            "openai",
        ]
        default_chain: list[str] = [name for name in priority if self.registry.has(name)]
        if operation in {
            "plan_generation",
            "summary_generation",
            "subagent_execution",
            "composer_chat",
            "code_edit_proposal",
        } and self.registry.has("mock"):
            default_chain.append("mock")
        if self.registry.has("fake"):
            default_chain.append("fake")
        if default_chain:
            return _prepend_preferred(default_chain)

        if preferred:
            return preferred

        raise ProviderExecutionError("unavailable", "No provider chain available")

    async def _publish_event(
        self,
        *,
        session_id: str,
        event_type: str,
        request_id: str,
        payload: dict[str, Any],
        correlation_id: str | None,
    ) -> None:
        seq = self.event_bus.next_seq(session_id)
        event = DebugEvent(
            id=make_id("evt"),
            session_id=session_id,
            seq=seq,
            type=event_type,
            ts=utc_now_iso(),
            source={"domain": "provider", "id": request_id},
            payload=payload,
            correlation_id=correlation_id,
        )
        await self.event_bus.publish(event)


async def _maybe_await(value: Any) -> None:
    """允许 on_delta 回调既可同步也可异步。"""
    if hasattr(value, "__await__"):
        await value


def normalize_provider_error(exc: Exception) -> ProviderErrorCode:
    message = str(exc).lower()
    status_code = getattr(exc, "status_code", None)

    if isinstance(exc, TimeoutError) or "timeout" in message:
        return "timeout"
    if status_code == 429 or "429" in message or "rate limit" in message:
        return "rate_limited"
    return "unavailable"


def _try_text_from_content_blocks(items: list[Any]) -> str | None:
    """Parse OpenAI/Anthropic-style content block lists."""
    parts: list[str] = []
    for item in items:
        if isinstance(item, str) and item.strip():
            parts.append(item)
            continue
        if not isinstance(item, dict):
            continue
        text = item.get("text")
        if isinstance(text, str) and text.strip():
            parts.append(text)
            continue
        if isinstance(text, dict) and isinstance(text.get("value"), str) and str(text.get("value", "")).strip():
            parts.append(text["value"])
            continue
        if isinstance(item.get("content"), str) and item["content"].strip():
            parts.append(item["content"])
    if parts:
        return "\n".join(parts)
    return None


def _try_extract_text_output(output: Any) -> str | None:
    """Best-effort text extraction; returns None if nothing usable."""
    if output is None:
        return None
    if isinstance(output, list):
        return _try_text_from_content_blocks(output)
    if isinstance(output, str):
        return output
    if not isinstance(output, dict):
        return None
    for key in ("refusal", "reasoning", "reasoning_content", "output_text", "text"):
        val = output.get(key)
        if isinstance(val, str) and val.strip():
            return val
    choices = output.get("choices")
    if isinstance(choices, list) and choices:
        c0 = choices[0]
        if isinstance(c0, dict) and c0.get("message") is not None:
            nested = _try_extract_text_output(c0.get("message"))
            if nested is not None:
                return nested
    msg = output.get("message")
    if isinstance(msg, str) and msg.strip():
        return msg
    if isinstance(msg, dict):
        nested = _try_extract_text_output(msg)
        if nested is not None:
            return nested
    content = output.get("content")
    if isinstance(content, str) and content.strip():
        return content
    if isinstance(content, list):
        nested = _try_text_from_content_blocks(content)
        if nested is not None:
            return nested
    return None


def extract_text_output(output: Any) -> str:
    out = _try_extract_text_output(output)
    if out is None:
        raise ValueError("Provider output does not contain text content")
    text = out.strip()
    if not text:
        raise ValueError("Provider output does not contain text content")
    return text


def extract_json_object(output: Any) -> dict[str, Any]:
    if isinstance(output, dict) and any(key in output for key in ("stages", "actions", "plan", "summary")):
        return output

    raw_text = extract_text_output(output)
    candidate = raw_text.strip()
    if candidate.startswith("```"):
        fence_start = candidate.find("\n")
        fence_end = candidate.rfind("```")
        if fence_start >= 0 and fence_end > fence_start:
            candidate = candidate[fence_start + 1 : fence_end].strip()
    start = candidate.find("{")
    end = candidate.rfind("}")
    if start >= 0 and end > start:
        candidate = candidate[start : end + 1]
    data = json.loads(candidate)
    if not isinstance(data, dict):
        raise ValueError("Provider output is not a JSON object")
    return data


def _read_provider_chain_from_env() -> list[str]:
    raw_value = os.getenv("AGENT_DEBUG_PROVIDER_CHAIN", "")
    return [item.strip() for item in raw_value.split(",") if item.strip()]


def build_provider_registry(
    *,
    channel_store: Any = None,
) -> ProviderRegistry:
    registry = ProviderRegistry()
    registry.register("mock", MockProvider())
    registry.register("fake", FakeProvider())
    if PackageAgentProvider.is_available():
        registry.register("package-agent", PackageAgentProvider())

    # 渠道化的中国大模型 / Anthropic / 自定义供应商。
    _register_channel_providers(registry, channel_store)
    return registry


def _register_channel_providers(registry: ProviderRegistry, channel_store: Any) -> None:
    """把已配置（或环境兜底）的渠道注册到 registry。

    每个渠道注册两个名字：``channel:{id}``（唯一）与其供应商类型名（便于默认链命中）。
    """
    try:
        from src.agent_debug.provider.channel_store import ChannelStore

        store = channel_store or ChannelStore()
        channels = list(store.enabled_channels())
        seen_provider_types: set[str] = set()
        for channel in channels:
            if not (channel.api_key or "").strip():
                continue
            provider = store.build_provider(channel)
            registry.register(f"channel:{channel.id}", provider)
            if channel.provider not in seen_provider_types:
                registry.register(channel.provider, provider)
                seen_provider_types.add(channel.provider)

    except Exception:  # pragma: no cover - 渠道层缺失不应阻断 registry 构建
        logging.getLogger(__name__).debug("channel provider registration skipped", exc_info=True)
