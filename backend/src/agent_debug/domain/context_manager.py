from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List

from src.agent_debug.infra.utils import make_id, utc_now_iso


@dataclass
class SessionContext:
    session_id: str
    active_context_ref: str
    compacted_context_refs: List[str] = field(default_factory=list)
    raw_context_refs: List[str] = field(default_factory=list)
    checkpoints: List[str] = field(default_factory=list)
    updated_at: str = ""


class SessionContextManager:
    def __init__(self) -> None:
        self.contexts: Dict[str, SessionContext] = {}

    def ensure(self, session_id: str) -> SessionContext:
        if session_id not in self.contexts:
            self.contexts[session_id] = SessionContext(
                session_id=session_id,
                active_context_ref=make_id("ctx"),
                updated_at=utc_now_iso(),
            )
        return self.contexts[session_id]

    def checkpoint(self, session_id: str, ref: str) -> SessionContext:
        context = self.ensure(session_id)
        context.checkpoints.append(ref)
        context.updated_at = utc_now_iso()
        return context

    def rollback_with_summary(self, session_id: str, raw_ref: str, summary_ref: str) -> SessionContext:
        context = self.ensure(session_id)
        context.raw_context_refs.append(raw_ref)
        context.compacted_context_refs.append(summary_ref)
        context.active_context_ref = summary_ref
        context.updated_at = utc_now_iso()
        return context
