"""上下文压缩（compaction）。

参考 Proma 的 ``/compact`` + 历史回填策略：当对话消息的估算 token 数接近
上下文预算时，把较早的消息交给 LLM 压缩成一段结构化摘要，仅保留最近 N 条
原始消息，从而控制窗口占用。

- token 估算走 CJK 友好的 ``cjk_token_estimator``。
- 摘要走 ``ProviderExecutionService`` 的 ``summary_generation`` 操作；失败时
  退化为「保留 system + 最近 N 条」的硬截断，保证不抛错。
"""

from __future__ import annotations

import os
from typing import Any, Callable, Dict, List, Optional, Tuple

from src.agent_debug.infra.utils import make_id
from src.agent_debug.provider.base import ModelRequestContext
from src.agent_debug.provider.cjk_token_estimator import estimate_messages_tokens
from src.agent_debug.provider.service import (
    ProviderExecutionService,
    extract_text_output,
)


def _default_budget() -> int:
    raw = (os.getenv("AGENT_DEBUG_CONTEXT_BUDGET_TOKENS") or "").strip()
    try:
        return max(2000, int(raw)) if raw else 24000
    except ValueError:
        return 24000


def _reserved_context_tokens() -> int:
    raw = (os.getenv("AGENT_DEBUG_CONTEXT_RESERVE_TOKENS") or "").strip()
    try:
        return max(0, int(raw)) if raw else 75_000
    except ValueError:
        return 75_000


class ContextCompactor:
    def __init__(
        self,
        provider_service: ProviderExecutionService,
        *,
        model_resolver: Optional[Callable[[str], str]] = None,
        context_budget_resolver: Optional[Callable[[str], Optional[int]]] = None,
        context_budget: Optional[int] = None,
        keep_recent: int = 8,
    ) -> None:
        self.provider_service = provider_service
        self.model_resolver = model_resolver or (lambda _sid: os.getenv("OPENAI_MODEL", "mock-model"))
        self.context_budget_resolver = context_budget_resolver
        self.context_budget = context_budget or _default_budget()
        self.reserve_tokens = _reserved_context_tokens()
        self.keep_recent = max(2, keep_recent)

    def estimate(self, messages: List[Dict[str, Any]]) -> int:
        return estimate_messages_tokens(messages)

    def needs_compaction(self, messages: List[Dict[str, Any]]) -> bool:
        return self.estimate(messages) > self.context_budget

    def _budget_for_session(self, session_id: str) -> int:
        raw_budget: int | None = None
        if self.context_budget_resolver is not None:
            try:
                resolved = self.context_budget_resolver(session_id)
            except Exception:
                resolved = None
            if resolved is not None:
                try:
                    raw_budget = max(2000, int(resolved))
                except (TypeError, ValueError):
                    pass
        if raw_budget is None:
            raw_budget = self.context_budget
        if raw_budget > self.reserve_tokens:
            return max(2000, raw_budget - self.reserve_tokens)
        return raw_budget

    async def compact(
        self, messages: List[Dict[str, Any]], session_id: str
    ) -> Tuple[List[Dict[str, Any]], bool]:
        if self.estimate(messages) <= self._budget_for_session(session_id):
            return messages, False

        # 拆分：system 消息保留在头部；中段压缩；尾部最近 keep_recent 条原样保留。
        system_msgs = [m for m in messages if m.get("role") == "system"]
        non_system = [m for m in messages if m.get("role") != "system"]
        if len(non_system) <= self.keep_recent:
            return messages, False

        to_summarize, recent = self._split_preserving_tool_pairs(non_system)
        if not to_summarize or not recent or not self._is_valid_tool_sequence(recent):
            return messages, False

        summary_text = await self._summarize(to_summarize, session_id)
        compact_msg = {
            "role": "system",
            "content": f"[历史对话压缩摘要]\n{summary_text}",
        }
        new_messages = [*system_msgs, compact_msg, *recent]
        return new_messages, True

    def _split_preserving_tool_pairs(
        self, messages: List[Dict[str, Any]]
    ) -> Tuple[List[Dict[str, Any]], List[Dict[str, Any]]]:
        """Split messages without leaving orphan ``tool`` messages in the suffix.

        OpenAI-compatible providers require every ``tool`` message to answer a
        preceding assistant message with matching ``tool_calls``. A plain
        ``messages[-keep_recent:]`` can cut through that pair and create invalid
        requests, so we expand the suffix leftward until it is protocol-valid.
        """
        start = max(0, len(messages) - self.keep_recent)
        while start > 0:
            recent = messages[start:]
            if self._is_valid_tool_sequence(recent):
                return messages[:start], recent
            start -= 1
        return [], messages

    @staticmethod
    def _is_valid_tool_sequence(messages: List[Dict[str, Any]]) -> bool:
        pending_tool_call_ids: List[str] = []
        for message in messages:
            role = message.get("role")
            if role == "assistant":
                if pending_tool_call_ids:
                    return False
                pending_tool_call_ids = [
                    str(call.get("id") or "")
                    for call in (message.get("tool_calls") or [])
                    if isinstance(call, dict) and call.get("id")
                ]
            elif role == "tool":
                tool_call_id = str(message.get("tool_call_id") or "")
                if not pending_tool_call_ids or tool_call_id not in pending_tool_call_ids:
                    return False
                pending_tool_call_ids.remove(tool_call_id)
            else:
                if pending_tool_call_ids:
                    return False
        return not pending_tool_call_ids

    async def _summarize(self, messages: List[Dict[str, Any]], session_id: str) -> str:
        transcript = self._render_transcript(messages)
        prompt = (
            "请把下面的多轮对话/工具调用历史压缩成简洁的中文要点摘要，保留：关键事实、"
            "已做的决定、产出物、未解决的问题与下一步。只输出摘要正文：\n\n" + transcript
        )
        ctx = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=self.model_resolver(session_id),
            timeout_ms=60000,
        )
        try:
            result = await self.provider_service.execute_chat_operation(
                request={"messages": [{"role": "user", "content": prompt}]},
                ctx=ctx,
                session_id=session_id,
                operation="summary_generation",
                parser=lambda response: extract_text_output(response.output),
            )
            return result.value
        except Exception:
            # 退化：直接拼接被压缩消息的截断文本。
            return self._render_transcript(messages)[:2000]

    @staticmethod
    def _render_transcript(messages: List[Dict[str, Any]]) -> str:
        lines: List[str] = []
        for m in messages:
            role = m.get("role", "?")
            content = m.get("content")
            if isinstance(content, str):
                text = content
            else:
                text = str(content)
            if m.get("tool_calls"):
                text = (text + " " if text else "") + f"[tool_calls: {m['tool_calls']}]"
            lines.append(f"{role}: {text}")
        return "\n".join(lines)
