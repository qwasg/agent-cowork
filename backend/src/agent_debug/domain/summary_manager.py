from __future__ import annotations

import os
from typing import Any, Callable

from src.agent_debug.domain.models import SubagentRun, SubagentSummary
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.base import ModelRequestContext, ProviderResponse
from src.agent_debug.provider.service import (
    ProviderExecutionError,
    ProviderExecutionService,
    extract_json_object,
)


def _to_string_list(value: Any) -> list[str]:
    return [str(item) for item in value] if isinstance(value, list) else []


class SummaryManager:
    def __init__(
        self,
        provider_service: ProviderExecutionService,
        model_resolver: Callable[[str], str] | None = None,
    ) -> None:
        self.provider_service = provider_service
        self.model_resolver = model_resolver or (lambda _session_id: os.getenv("OPENAI_MODEL", "mock-model"))
        self.summaries = InMemoryTable[SubagentSummary]()

    async def summarize_subagent(self, subagent: SubagentRun, objective: str, session_id: str) -> SubagentSummary:
        fallback = self._fallback_summary(subagent, objective)
        request_context = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=self.model_resolver(session_id),
            timeout_ms=8000,
            session_id=session_id,
            run_id=subagent.parent_run_id,
            metadata={
                "operation": "summary_generation",
                "contextRef": subagent.context_ref,
            },
        )

        try:
            result = await self.provider_service.execute_chat_operation(
                request=self._build_request(subagent, objective),
                ctx=request_context,
                session_id=session_id,
                operation="summary_generation",
                parser=self._parse_provider_summary,
                correlation_id=subagent.parent_run_id,
            )
            summary = self._merge_summary(subagent, objective, result.response, result.value)
        except ProviderExecutionError:
            summary = fallback

        self.summaries.save(summary.id, summary)
        return summary

    def _build_request(self, subagent: SubagentRun, objective: str) -> dict[str, Any]:
        return {
            "messages": [
                {
                    "role": "system",
                    "content": (
                        "You summarize agent work for a debug console. "
                        "Return JSON only with keys: actions, keyFindings, artifacts, errors, risks, decisions, "
                        "nextActions, unresolvedQuestions, contextDelta, lineage, tokenUsage, compressionLevel."
                    ),
                },
                {
                    "role": "user",
                    "content": (
                        f"Objective: {objective}\n"
                        f"Subagent objective: {subagent.objective}\n"
                        f"Raw context ref: {subagent.context_ref}\n"
                        "Produce a concise structured rollback summary."
                    ),
                },
            ]
        }

    def _parse_provider_summary(self, response: ProviderResponse) -> dict[str, Any]:
        payload = extract_json_object(response.output)
        if "summary" in payload and isinstance(payload["summary"], dict):
            payload = payload["summary"]
        if not isinstance(payload.get("actions"), list):
            raise ValueError("Summary response must include actions")
        if not isinstance(payload.get("keyFindings"), list):
            raise ValueError("Summary response must include keyFindings")
        return payload

    def _merge_summary(
        self,
        subagent: SubagentRun,
        objective: str,
        response: ProviderResponse,
        payload: dict[str, Any],
    ) -> SubagentSummary:
        token_usage = payload.get("tokenUsage") if isinstance(payload.get("tokenUsage"), dict) else None
        normalized_token_usage = {
            "inputTokens": int((token_usage or {}).get("inputTokens") or response.token_usage.get("input", 200)),
            "outputTokens": int((token_usage or {}).get("outputTokens") or response.token_usage.get("output", 120)),
            "summaryTokens": int((token_usage or {}).get("summaryTokens") or response.token_usage.get("output", 120)),
        }
        return SubagentSummary(
            id=make_id("summary"),
            task_id=subagent.plan_node_ids[0] if subagent.plan_node_ids else subagent.id,
            subagent_run_id=subagent.id,
            objective=objective,
            input_goal=objective,
            actions=_to_string_list(payload.get("actions")),
            key_findings=_to_string_list(payload.get("keyFindings") or payload.get("key_findings")),
            artifacts=payload.get("artifacts") if isinstance(payload.get("artifacts"), list) else [],
            errors=payload.get("errors") if isinstance(payload.get("errors"), list) else [],
            risks=_to_string_list(payload.get("risks")),
            decisions=_to_string_list(payload.get("decisions")),
            next_actions=_to_string_list(payload.get("nextActions") or payload.get("next_actions")),
            unresolved_questions=_to_string_list(payload.get("unresolvedQuestions") or payload.get("unresolved_questions")),
            context_delta=self._normalize_context_delta(payload.get("contextDelta") or payload.get("context_delta")),
            lineage=self._normalize_lineage(payload.get("lineage"), subagent.context_ref),
            token_usage=normalized_token_usage,
            compression_level=str(payload.get("compressionLevel") or payload.get("compression_level") or "structured"),
            created_at=utc_now_iso(),
        )

    def _fallback_summary(self, subagent: SubagentRun, objective: str) -> SubagentSummary:
        return SubagentSummary(
            id=make_id("summary"),
            task_id=subagent.plan_node_ids[0] if subagent.plan_node_ids else subagent.id,
            subagent_run_id=subagent.id,
            objective=objective,
            input_goal=objective,
            actions=["collect context", "execute subagent task", "produce summary"],
            key_findings=["当前为骨架实现，已生成可追踪的结构化 summary"],
            artifacts=[],
            errors=[],
            risks=[],
            decisions=["采用结构化 summary + lineage 记录"],
            next_actions=["将 summary 回写主 Agent 上下文"],
            unresolved_questions=[],
            context_delta={
                "factsAdded": ["summary 结果已注入 compacted context"],
                "factsRemoved": [],
                "assumptionsUpdated": [],
            },
            lineage={
                "replacedContextRefs": [subagent.context_ref],
                "sourceEventIds": [],
                "canExpand": True,
            },
            token_usage={"inputTokens": 200, "outputTokens": 120, "summaryTokens": 80},
            compression_level="structured",
            created_at=utc_now_iso(),
        )

    def _normalize_context_delta(self, value: Any) -> dict[str, Any]:
        if isinstance(value, dict):
            return {
                "factsAdded": _to_string_list(value.get("factsAdded") or value.get("facts_added")),
                "factsRemoved": _to_string_list(value.get("factsRemoved") or value.get("facts_removed")),
                "assumptionsUpdated": _to_string_list(
                    value.get("assumptionsUpdated") or value.get("assumptions_updated")
                ),
            }
        return {"factsAdded": [], "factsRemoved": [], "assumptionsUpdated": []}

    def _normalize_lineage(self, value: Any, raw_context_ref: str) -> dict[str, Any]:
        if isinstance(value, dict):
            return {
                "replacedContextRefs": _to_string_list(
                    value.get("replacedContextRefs") or value.get("replaced_context_refs")
                )
                or [raw_context_ref],
                "sourceEventIds": _to_string_list(value.get("sourceEventIds") or value.get("source_event_ids")),
                "canExpand": bool(value.get("canExpand", value.get("can_expand", True))),
            }
        return {
            "replacedContextRefs": [raw_context_ref],
            "sourceEventIds": [],
            "canExpand": True,
        }
