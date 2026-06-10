"""Subagent lifecycle store.

Adds explicit ``cancel``/``fail``/``retry`` transitions and a small bookkeeping
surface so :class:`AgentRuntimeService` can keep arbitrary numbers of
subagents in flight without losing track of which ones still need
summarisation.
"""

from __future__ import annotations

from typing import List, Optional

from src.agent_debug.domain.models import SubagentRun
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso


class SubagentOrchestrator:
    def __init__(self) -> None:
        self.subagents = InMemoryTable[SubagentRun]()

    def create(
        self,
        parent_run_id: str,
        plan_node_ids: List[str],
        todo_ids: List[str],
        objective: str,
        *,
        mode: str = "serial",
    ) -> SubagentRun:
        subagent = SubagentRun(
            id=make_id("sub"),
            parent_run_id=parent_run_id,
            plan_node_ids=plan_node_ids,
            todo_ids=todo_ids,
            mode=mode,
            status="running",
            objective=objective,
            context_ref=make_id("ctx"),
            retry_count=0,
            started_at=utc_now_iso(),
        )
        self.subagents.save(subagent.id, subagent)
        return subagent

    def get(self, subagent_id: str) -> Optional[SubagentRun]:
        return self.subagents.get(subagent_id)

    def list_for_run(self, parent_run_id: str) -> List[SubagentRun]:
        return self.subagents.list_by("parent_run_id", parent_run_id)

    def complete(self, subagent_id: str) -> Optional[SubagentRun]:
        return self._transition(subagent_id, "completed")

    def cancel(self, subagent_id: str) -> Optional[SubagentRun]:
        return self._transition(subagent_id, "cancelled")

    def fail(self, subagent_id: str, *, error: str | None = None) -> Optional[SubagentRun]:
        del error  # carried alongside in events; subagent record only stores status/timestamps
        return self._transition(subagent_id, "failed")

    def retry(self, subagent_id: str) -> Optional[SubagentRun]:
        sub = self.subagents.get(subagent_id)
        if not sub:
            return None
        sub.status = "running"
        sub.retry_count = int(sub.retry_count or 0) + 1
        sub.started_at = utc_now_iso()
        sub.ended_at = None
        self.subagents.save(sub.id, sub)
        return sub

    def _transition(self, subagent_id: str, status: str) -> Optional[SubagentRun]:
        sub = self.subagents.get(subagent_id)
        if not sub:
            return None
        sub.status = status
        sub.ended_at = utc_now_iso()
        self.subagents.save(sub.id, sub)
        return sub
