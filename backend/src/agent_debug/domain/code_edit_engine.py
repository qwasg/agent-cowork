"""Code-edit proposal engine.

Two execution paths:

1. ``maybe_generate(...)`` — pure heuristic (no LLM). Kept for backwards
   compatibility with the existing ``test_rest_gateway`` test which asserts
   on the deterministic ``TS2322`` repair output.
2. ``propose_with_llm(...)`` — async path that asks the configured LLM to
   produce a structured ``{summary, changes:[{path, original, proposed,
   description}]}`` JSON. Falls back to ``maybe_generate`` when the LLM
   yields nothing usable, so plan execution still produces a proposal in
   offline / mock mode.

The :class:`ProposalRegistry` persists every emitted proposal in-memory so
the new ``apply`` / ``discard`` REST routes can flip status and write to
disk via :class:`WorkspaceTreeService`.
"""

from __future__ import annotations

import re
from typing import Any, Callable, Dict, Iterable, List, Optional

from src.agent_debug.domain.models import CodeEditProposal
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.base import ModelRequestContext, ProviderResponse
from src.agent_debug.provider.service import (
    ProviderExecutionError,
    ProviderExecutionService,
    extract_json_object,
)


SCHEMA_VERSION = "ai-edit-proposal/v1"


class CodeEditEngine:
    """Builds normalized AI edit proposals from session context."""

    SCHEMA_VERSION = SCHEMA_VERSION

    def maybe_generate(
        self,
        session_id: str,
        user_input: str,
        context_window: Dict[str, Any] | None,
    ) -> Dict[str, Any] | None:
        if not isinstance(context_window, dict):
            return None

        active_file = context_window.get("activeFile")
        if not isinstance(active_file, dict):
            return None

        path = str(active_file.get("path") or "").strip()
        language = str(active_file.get("language") or "plaintext").strip() or "plaintext"
        original_content = str(active_file.get("content") or "")
        if not path or not original_content:
            return None

        selection = context_window.get("selection")
        terminal_output = str(context_window.get("terminalRecentOutput") or "")
        proposed_content = self._propose_content(
            language=language,
            original_content=original_content,
            terminal_output=terminal_output,
            selection=selection,
            user_input=user_input,
        )
        if proposed_content == original_content:
            return None

        return {
            "schemaVersion": self.SCHEMA_VERSION,
            "proposalId": make_id("edit"),
            "sessionId": session_id,
            "source": "agent-debug",
            "summary": self._build_summary(path, terminal_output, user_input),
            "createdAt": utc_now_iso(),
            "changes": [
                {
                    "changeId": make_id("change"),
                    "path": path,
                    "language": language,
                    "description": self._build_change_description(terminal_output, user_input),
                    "strategy": "replace-file",
                    "originalContent": original_content,
                    "proposedContent": proposed_content,
                    "selection": self._normalize_selection(selection),
                }
            ],
        }

    def _propose_content(
        self,
        *,
        language: str,
        original_content: str,
        terminal_output: str,
        selection: Any,
        user_input: str,
    ) -> str:
        normalized_language = language.lower()
        lowered_output = terminal_output.lower()
        lowered_input = user_input.lower()

        if normalized_language in {"typescript", "javascript", "tsx", "jsx"}:
            replaced = re.sub(
                r"(:\s*number\s*=\s*)'(\d+)'",
                r"\g<1>\g<2>",
                original_content,
                count=1,
            )
            if replaced != original_content and (
                "ts2322" in lowered_output
                or "type 'string' is not assignable to type 'number'" in lowered_output
                or "number" in lowered_input
            ):
                return replaced

        del selection
        return original_content

    def _build_summary(self, path: str, terminal_output: str, user_input: str) -> str:
        if "TS2322" in terminal_output or "type 'string' is not assignable to type 'number'" in terminal_output:
            return f"根据 TypeScript 类型报错，为 `{path}` 生成了一个候选修复。"
        if user_input.strip():
            return f"根据当前会话请求，为 `{path}` 生成了一个候选修改。"
        return f"为 `{path}` 生成了一个候选修改。"

    def _build_change_description(self, terminal_output: str, user_input: str) -> str:
        if "TS2322" in terminal_output or "type 'string' is not assignable to type 'number'" in terminal_output:
            return "将错误的字符串数字字面量改为 number，以匹配现有类型声明。"
        if user_input.strip():
            return f"结合当前请求“{user_input.strip()}”生成的修改建议。"
        return "根据当前上下文生成的修改建议。"

    def _normalize_selection(self, value: Any) -> Dict[str, Any] | None:
        if not isinstance(value, dict):
            return None
        try:
            return {
                "start": int(value.get("start") or 0),
                "end": int(value.get("end") or 0),
                "selectedText": str(value.get("selectedText") or ""),
                "startLine": int(value.get("startLine") or 0),
                "endLine": int(value.get("endLine") or 0),
            }
        except (TypeError, ValueError):
            return None

    # ------------------------------------------------------------------ LLM path

    _SYSTEM_DIFF_PROMPT = (
        "You are a senior code-edit reviewer. Given the user objective and the "
        "current file content, return STRICT JSON with this shape:\n"
        '{"summary": str, "changes": [{"path": str, "language": str, '
        '"description": str, "originalContent": str, "proposedContent": str}]}\n'
        "Rules:\n"
        "- ``proposedContent`` must be the FULL replacement file content, "
        "preserving unrelated lines exactly.\n"
        "- Make the smallest correct change that satisfies the objective.\n"
        "- If you cannot suggest a safe change, return {\"changes\": []}."
    )

    async def propose_with_llm(
        self,
        provider_service: ProviderExecutionService,
        *,
        session_id: str,
        user_input: str,
        context_window: Dict[str, Any] | None,
        model_resolver: Callable[[str], str],
    ) -> Optional[Dict[str, Any]]:
        if not isinstance(context_window, dict):
            return None
        active_file = context_window.get("activeFile")
        if not isinstance(active_file, dict):
            return None
        path = str(active_file.get("path") or "").strip()
        language = str(active_file.get("language") or "plaintext").strip() or "plaintext"
        original_content = str(active_file.get("content") or "")
        if not path or not original_content:
            return None

        request_context = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=model_resolver(session_id),
            timeout_ms=20_000,
            session_id=session_id,
            metadata={"operation": "code_edit_proposal"},
        )
        user_msg = (
            f"Objective: {user_input.strip() or '修复当前文件中的明显问题'}\n"
            f"File path: {path}\n"
            f"Language: {language}\n"
            f"Terminal output: {str(context_window.get('terminalRecentOutput') or '')[:1500]}\n"
            "----\n"
            "Current file content:\n"
            f"```\n{original_content}\n```"
        )
        request = {
            "messages": [
                {"role": "system", "content": self._SYSTEM_DIFF_PROMPT},
                {"role": "user", "content": user_msg},
            ],
            "temperature": 0.1,
        }

        try:
            result = await provider_service.execute_chat_operation(
                request=request,
                ctx=request_context,
                session_id=session_id,
                operation="composer_chat",
                parser=self._parse_llm_proposal,
            )
            payload = result.value
        except ProviderExecutionError:
            return self.maybe_generate(session_id, user_input, context_window)

        changes = payload.get("changes") or []
        if not isinstance(changes, list) or not changes:
            return self.maybe_generate(session_id, user_input, context_window)

        normalised: List[Dict[str, Any]] = []
        for change in changes:
            if not isinstance(change, dict):
                continue
            change_path = str(change.get("path") or path)
            proposed = str(change.get("proposedContent") or "")
            original = str(change.get("originalContent") or original_content)
            if proposed == original:
                continue
            normalised.append(
                {
                    "changeId": make_id("change"),
                    "path": change_path,
                    "language": str(change.get("language") or language),
                    "description": str(change.get("description") or ""),
                    "strategy": "replace-file",
                    "originalContent": original,
                    "proposedContent": proposed,
                    "selection": None,
                }
            )
        if not normalised:
            return self.maybe_generate(session_id, user_input, context_window)
        return {
            "schemaVersion": self.SCHEMA_VERSION,
            "proposalId": make_id("edit"),
            "sessionId": session_id,
            "source": "agent-debug-llm",
            "summary": str(payload.get("summary") or "Agent 提议的代码修改"),
            "createdAt": utc_now_iso(),
            "changes": normalised,
        }

    def _parse_llm_proposal(self, response: ProviderResponse) -> Dict[str, Any]:
        return extract_json_object(response.output)


class ProposalRegistry:
    """Stores :class:`CodeEditProposal` records for apply/discard."""

    def __init__(self) -> None:
        self._table = InMemoryTable[CodeEditProposal]()

    def record(self, payload: Dict[str, Any]) -> CodeEditProposal:
        record = CodeEditProposal(
            id=str(payload.get("proposalId") or make_id("edit")),
            session_id=str(payload.get("sessionId") or ""),
            schema_version=str(payload.get("schemaVersion") or SCHEMA_VERSION),
            summary=str(payload.get("summary") or ""),
            source=str(payload.get("source") or "agent-debug"),
            changes=list(payload.get("changes") or []),
            status="pending",
            created_at=str(payload.get("createdAt") or utc_now_iso()),
        )
        self._table.save(record.id, record)
        return record

    def get(self, proposal_id: str) -> Optional[CodeEditProposal]:
        return self._table.get(proposal_id)

    def list_for_session(self, session_id: str) -> List[CodeEditProposal]:
        return self._table.list_by("session_id", session_id)

    def all(self) -> List[CodeEditProposal]:
        return self._table.list_all()

    def mark_applied(self, proposal_id: str, applied_files: Iterable[str]) -> Optional[CodeEditProposal]:
        record = self._table.get(proposal_id)
        if record is None:
            return None
        record.status = "applied"
        record.applied_at = utc_now_iso()
        record.applied_files = list(applied_files)
        record.apply_error = None
        self._table.save(record.id, record)
        return record

    def mark_apply_failed(self, proposal_id: str, error: str) -> Optional[CodeEditProposal]:
        record = self._table.get(proposal_id)
        if record is None:
            return None
        record.apply_error = error
        self._table.save(record.id, record)
        return record

    def mark_discarded(self, proposal_id: str) -> Optional[CodeEditProposal]:
        record = self._table.get(proposal_id)
        if record is None:
            return None
        record.status = "discarded"
        record.discarded_at = utc_now_iso()
        self._table.save(record.id, record)
        return record


def proposal_to_dict(record: CodeEditProposal) -> Dict[str, Any]:
    return {
        "id": record.id,
        "sessionId": record.session_id,
        "schemaVersion": record.schema_version,
        "summary": record.summary,
        "source": record.source,
        "status": record.status,
        "appliedAt": record.applied_at,
        "discardedAt": record.discarded_at,
        "appliedFiles": list(record.applied_files),
        "applyError": record.apply_error,
        "changes": list(record.changes),
        "createdAt": record.created_at,
    }
