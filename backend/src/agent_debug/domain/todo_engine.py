"""Todo store with dependency awareness.

The previous implementation only owned ``materialize/list/update_status`` and
ignored every dependency edge expressed in ``PlanTask.depends_on``. The new
shape:

- ``materialize_from_tasks`` carries ``depends_on`` over to ``TodoItem.dependencies``
  (mapped from the originating ``PlanTask`` ids to the freshly created
  ``TodoItem`` ids).
- ``ready_todos`` returns todos whose dependencies are all completed.
- ``mark_status`` is the single status mutator (``mark_status(id, status, error=None)``).
- ``unblock_dependents`` flips any todo waiting on a freshly completed todo to
  ``ready`` so a serial / parallel executor can pick it up immediately.
- ``add_manual`` / ``update`` / ``batch_rerun`` back the user-facing routes.
- The contract is intentionally synchronous: persistence still lives in
  in-memory tables; the API layer is responsible for emitting events.
"""

from __future__ import annotations

from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

from src.agent_debug.domain.models import PlanTask, TodoItem, TodoStatus
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso


_TERMINAL_STATUSES = {"completed", "rolled_up", "failed", "cancelled", "skipped"}

# Map the model-facing ``write_todos`` statuses onto the internal lifecycle.
_AGENT_STATUS_MAP: Dict[str, TodoStatus] = {
    "pending": "ready",
    "in_progress": "running",
    "completed": "completed",
}


class TodoEngine:
    def __init__(self) -> None:
        self.todos = InMemoryTable[TodoItem]()
        # Per-session map of model-provided client ids → internal todo ids, so a
        # follow-up ``write_todos`` call can update the same items by id.
        self._agent_todo_index: Dict[str, Dict[str, str]] = {}

    def delete_session_todos(self, session_id: str) -> int:
        """删除某会话的全部 todo 及其 agent 索引（会话删除级联清理）。"""
        removed = 0
        for todo in self.list_by_session(session_id):
            if self.todos.delete(todo.id):
                removed += 1
        self._agent_todo_index.pop(session_id, None)
        return removed

    def materialize_from_tasks(
        self, session_id: str, tasks: Sequence[PlanTask]
    ) -> List[TodoItem]:
        """Create one ``TodoItem`` per ``PlanTask`` and rewrite ``depends_on``
        so the resulting graph references todo ids (not plan task ids).
        """
        now = utc_now_iso()
        task_id_to_todo_id: Dict[str, str] = {task.id: make_id("todo") for task in tasks}
        items: List[TodoItem] = []
        for order, task in enumerate(tasks, start=1):
            todo_id = task_id_to_todo_id[task.id]
            dep_ids = [
                task_id_to_todo_id[dep_task_id]
                for dep_task_id in (task.depends_on or [])
                if dep_task_id in task_id_to_todo_id
            ]
            initial_status: TodoStatus = "queued" if dep_ids else "ready"
            todo = TodoItem(
                id=todo_id,
                session_id=session_id,
                title=task.title,
                description=task.description,
                source="plan",
                owner={"type": task.owner_type or "main-agent", "id": "main"},
                priority=100 - order,
                status=initial_status,
                dependencies=dep_ids,
                related_plan_node_ids=[task.id],
                related_subagent_run_ids=[],
                related_swarm_shard_ids=[],
                retry_count=0,
                due_hint="now",
                artifacts=[],
                created_at=now,
                updated_at=now,
            )
            self.todos.save(todo.id, todo)
            items.append(todo)
        return items

    def list_by_session(self, session_id: str) -> List[TodoItem]:
        return self.todos.list_by("session_id", session_id)

    def list_default_visible_by_session(self, session_id: str) -> List[TodoItem]:
        visible_group_id = self.current_agent_group_id(session_id)
        visible: List[TodoItem] = []
        for todo in self.list_by_session(session_id):
            if todo.archived_at:
                continue
            if todo.source != "agent":
                visible.append(todo)
                continue
            if visible_group_id is not None and todo.agent_todo_group_id != visible_group_id:
                continue
            visible.append(todo)
        return visible

    def archive_plan_todos_except(
        self,
        session_id: str,
        active_plan_node_ids: Iterable[str],
    ) -> List[TodoItem]:
        active = set(active_plan_node_ids)
        now = utc_now_iso()
        archived: List[TodoItem] = []
        for todo in self.list_by_session(session_id):
            if todo.source != "plan" or todo.archived_at:
                continue
            if any(node_id in active for node_id in (todo.related_plan_node_ids or [])):
                continue
            todo.status = "cancelled"  # type: ignore[assignment]
            todo.archived_at = now
            todo.updated_at = now
            self.todos.save(todo.id, todo)
            archived.append(todo)
        return archived

    def reset_plan_todos(self, todos: Iterable[TodoItem]) -> List[TodoItem]:
        reset: List[TodoItem] = []
        for todo in todos:
            if todo.source != "plan":
                continue
            todo.status = "queued" if todo.dependencies else "ready"  # type: ignore[assignment]
            todo.last_error = None
            todo.updated_at = utc_now_iso()
            self.todos.save(todo.id, todo)
            reset.append(todo)
        return reset

    def get(self, todo_id: str) -> Optional[TodoItem]:
        return self.todos.get(todo_id)

    def ready_todos(self, session_id: str) -> List[TodoItem]:
        out: List[TodoItem] = []
        for todo in self.list_by_session(session_id):
            if todo.status in _TERMINAL_STATUSES or todo.status == "running":
                continue
            if self.pending_dependencies(todo.id):
                continue
            out.append(todo)
        out.sort(key=lambda t: -int(t.priority))
        return out

    def pending_dependencies(self, todo_id: str) -> List[str]:
        todo = self.todos.get(todo_id)
        if not todo:
            return []
        out: List[str] = []
        for dep_id in todo.dependencies:
            dep = self.todos.get(dep_id)
            if dep is None:
                continue
            if dep.status not in {"completed", "rolled_up"}:
                out.append(dep_id)
        return out

    def mark_status(
        self,
        todo_id: str,
        status: TodoStatus,
        *,
        error: Optional[str] = None,
        bump_retry: bool = False,
    ) -> Optional[TodoItem]:
        todo = self.todos.get(todo_id)
        if not todo:
            return None
        todo.status = status  # type: ignore[assignment]
        todo.updated_at = utc_now_iso()
        if error is not None:
            todo.last_error = error
        if bump_retry:
            todo.retry_count = int(todo.retry_count or 0) + 1
        self.todos.save(todo.id, todo)
        return todo

    def update_status(self, todo_id: str, status: str) -> Optional[TodoItem]:
        return self.mark_status(todo_id, status)  # type: ignore[arg-type]

    def unblock_dependents(self, completed_id: str) -> List[TodoItem]:
        promoted: List[TodoItem] = []
        for todo in self.todos.list_all():
            if completed_id not in todo.dependencies:
                continue
            if todo.status not in {"queued", "blocked"}:
                continue
            if self.pending_dependencies(todo.id):
                continue
            todo.status = "ready"  # type: ignore[assignment]
            todo.updated_at = utc_now_iso()
            self.todos.save(todo.id, todo)
            promoted.append(todo)
        return promoted

    def complete_agent_todos_for_run(self, run_id: str) -> List[TodoItem]:
        completed: List[TodoItem] = []
        if not run_id:
            return completed
        for todo in self.todos.list_all():
            if todo.source != "agent":
                continue
            if todo.related_agent_run_id != run_id:
                continue
            if todo.status in {"completed", "rolled_up"}:
                completed.append(todo)
                continue
        return completed

    def has_open_agent_todos(self, session_id: str, group_id: str) -> bool:
        for todo in self.list_by_session(session_id):
            if todo.source != "agent" or todo.agent_todo_group_id != group_id:
                continue
            if todo.archived_at:
                continue
            if todo.status not in _TERMINAL_STATUSES:
                return True
        return False

    def archive_agent_group(self, session_id: str, group_id: str) -> List[TodoItem]:
        archived: List[TodoItem] = []
        if not session_id or not group_id:
            return archived
        now = utc_now_iso()
        for todo in self.list_by_session(session_id):
            if todo.source != "agent":
                continue
            if todo.agent_todo_group_id != group_id:
                continue
            if todo.archived_at:
                continue
            todo.archived_at = now
            todo.updated_at = now
            self.todos.save(todo.id, todo)
            archived.append(todo)
        if archived:
            archived_ids = {todo.id for todo in archived}
            session_index = self._agent_todo_index.get(session_id)
            if session_index:
                for client_id, todo_id in list(session_index.items()):
                    if todo_id in archived_ids:
                        del session_index[client_id]
        return archived

    def current_agent_group_id(self, session_id: str) -> Optional[str]:
        candidate_group_id: Optional[str] = None
        candidate_updated_at = ""
        for todo in self.list_by_session(session_id):
            if todo.source != "agent":
                continue
            if todo.archived_at:
                continue
            group_id = todo.agent_todo_group_id
            if not group_id:
                continue
            if todo.updated_at >= candidate_updated_at:
                candidate_group_id = group_id
                candidate_updated_at = todo.updated_at
        return candidate_group_id

    def add_manual(
        self,
        session_id: str,
        title: str,
        *,
        description: str = "",
        dependencies: Optional[Iterable[str]] = None,
        priority: int = 50,
        owner_type: str = "user",
    ) -> TodoItem:
        now = utc_now_iso()
        deps = list(dependencies or [])
        initial_status: TodoStatus = "queued" if deps else "ready"
        todo = TodoItem(
            id=make_id("todo"),
            session_id=session_id,
            title=title,
            description=description,
            source="user",
            owner={"type": owner_type, "id": owner_type},
            priority=int(priority),
            status=initial_status,
            dependencies=deps,
            related_plan_node_ids=[],
            related_subagent_run_ids=[],
            related_swarm_shard_ids=[],
            retry_count=0,
            due_hint="now",
            artifacts=[],
            created_at=now,
            updated_at=now,
        )
        self.todos.save(todo.id, todo)
        return todo

    def clone_session_todos(
        self,
        source_session_id: str,
        target_session_id: str,
        *,
        plan_node_id_map: Optional[Dict[str, str]] = None,
    ) -> List[TodoItem]:
        now = utc_now_iso()
        plan_node_id_map = plan_node_id_map or {}
        source_items = [todo for todo in self.list_by_session(source_session_id) if not todo.archived_at]
        id_map: Dict[str, str] = {todo.id: make_id("todo") for todo in source_items}
        cloned: List[TodoItem] = []
        for todo in source_items:
            clone = TodoItem(
                id=id_map[todo.id],
                session_id=target_session_id,
                title=todo.title,
                description=todo.description,
                source=todo.source,
                owner=dict(todo.owner or {}),
                priority=todo.priority,
                status=todo.status,
                dependencies=[id_map[d] for d in todo.dependencies if d in id_map],
                related_plan_node_ids=[
                    plan_node_id_map.get(node_id, node_id)
                    for node_id in (todo.related_plan_node_ids or [])
                ],
                related_agent_run_id=None,
                agent_todo_group_id=todo.agent_todo_group_id,
                related_subagent_run_ids=list(todo.related_subagent_run_ids or []),
                related_swarm_shard_ids=list(todo.related_swarm_shard_ids or []),
                retry_count=todo.retry_count,
                due_hint=todo.due_hint,
                artifacts=list(todo.artifacts or []),
                summary=todo.summary,
                last_error=todo.last_error,
                archived_at=None,
                created_at=now,
                updated_at=now,
            )
            self.todos.save(clone.id, clone)
            cloned.append(clone)
        return cloned

    def sync_agent_todos(
        self,
        session_id: str,
        items: Sequence[Dict[str, Any]],
        *,
        run_id: Optional[str] = None,
    ) -> List[Tuple[TodoItem, str]]:
        """Reconcile a full agent-authored todo list (``write_todos`` semantics).

        ``items`` is the COMPLETE list the model sent this turn. Each item is
        ``{id?, content/title, status, note?/description?}`` where ``status`` is
        one of ``pending|in_progress|completed``. Items are matched to existing
        todos by the model-provided client ``id`` (preferred) or, failing that,
        by a normalised title. Unmatched items are created.

        Returns ``[(todo, kind)]`` where ``kind`` is one of
        ``created|updated|running|completed`` so the caller can publish the
        matching ``todo.*`` event.
        """
        now = utc_now_iso()
        index = self._agent_todo_index.setdefault(session_id, {})
        group_id = self.current_agent_group_id(session_id) or make_id("todo_group")
        # Title → internal id fallback for items without a stable client id.
        title_index: Dict[str, str] = {}
        duplicate_titles: set[str] = set()
        for todo in self.list_by_session(session_id):
            if self._is_active_agent_todo(todo, group_id):
                key = self._normalise_title(todo.title)
                if key in title_index:
                    duplicate_titles.add(key)
                else:
                    title_index[key] = todo.id
        for key in duplicate_titles:
            title_index.pop(key, None)

        changes: List[Tuple[TodoItem, str]] = []
        present_ids: set[str] = set()
        seen_payload_titles: set[str] = set()
        for order, raw in enumerate(items, start=1):
            if not isinstance(raw, dict):
                continue
            title = str(raw.get("content") or raw.get("title") or "").strip()
            if not title:
                continue
            status = _AGENT_STATUS_MAP.get(str(raw.get("status") or "pending"), "ready")
            note = raw.get("note")
            description = str(note if note is not None else raw.get("description") or "")
            client_id = str(raw.get("id") or "").strip()
            normalized_title = self._normalise_title(title)

            existing_id: Optional[str] = None
            if client_id and client_id in index:
                indexed_id = index[client_id]
                indexed_todo = self.todos.get(indexed_id)
                if self._is_active_agent_todo(indexed_todo, group_id):
                    existing_id = indexed_id
            if existing_id is None:
                candidate_id = title_index.get(normalized_title)
                if candidate_id and normalized_title not in seen_payload_titles:
                    candidate = self.todos.get(candidate_id)
                    if self._is_active_agent_todo(candidate, group_id):
                        existing_id = candidate_id

            todo = self.todos.get(existing_id) if existing_id else None
            if todo is not None and not self._is_active_agent_todo(todo, group_id):
                todo = None
            if todo is None:
                todo = TodoItem(
                    id=make_id("todo"),
                    session_id=session_id,
                    title=title,
                    description=description,
                    source="agent",
                    owner={"type": "main-agent", "id": "main"},
                    priority=100 - order,
                    status=status,
                    dependencies=[],
                    related_plan_node_ids=[],
                    related_agent_run_id=run_id,
                    agent_todo_group_id=group_id,
                    related_subagent_run_ids=[],
                    related_swarm_shard_ids=[],
                    retry_count=0,
                    due_hint="now",
                    artifacts=[],
                    summary=description or None,
                    created_at=now,
                    updated_at=now,
                )
                self.todos.save(todo.id, todo)
                if client_id:
                    index[client_id] = todo.id
                title_index[normalized_title] = todo.id
                seen_payload_titles.add(normalized_title)
                present_ids.add(todo.id)
                changes.append((todo, "created"))
                continue

            prev_status = todo.status
            todo.title = title
            if description:
                todo.description = description
                todo.summary = description
            todo.priority = 100 - order
            todo.status = status  # type: ignore[assignment]
            if run_id:
                todo.related_agent_run_id = run_id
            todo.agent_todo_group_id = group_id
            todo.updated_at = now
            self.todos.save(todo.id, todo)
            if client_id:
                index[client_id] = todo.id
            title_index[normalized_title] = todo.id
            seen_payload_titles.add(normalized_title)
            present_ids.add(todo.id)

            if status != prev_status and status == "running":
                kind = "running"
            elif status != prev_status and status == "completed":
                kind = "completed"
            else:
                kind = "updated"
            changes.append((todo, kind))

        for todo in self.list_by_session(session_id):
            if not self._is_active_agent_todo(todo, group_id):
                continue
            if todo.id in present_ids:
                continue
            if todo.status not in _TERMINAL_STATUSES:
                todo.status = "cancelled"  # type: ignore[assignment]
            todo.archived_at = todo.archived_at or now
            todo.updated_at = now
            self.todos.save(todo.id, todo)
            changes.append((todo, "updated"))

        return changes

    @staticmethod
    def _normalise_title(title: str) -> str:
        return " ".join(str(title or "").split()).lower()

    @staticmethod
    def _is_active_agent_todo(todo: Optional[TodoItem], group_id: str) -> bool:
        """True when *todo* is a non-archived agent item in the active *group_id*."""
        if todo is None:
            return False
        return (
            todo.source == "agent"
            and not todo.archived_at
            and todo.agent_todo_group_id == group_id
        )

    def update(
        self,
        todo_id: str,
        patch: Dict[str, Any],
    ) -> Optional[TodoItem]:
        todo = self.todos.get(todo_id)
        if todo is None:
            return None
        if "title" in patch and isinstance(patch["title"], str):
            t = patch["title"].strip()
            if t:
                todo.title = t
        if "description" in patch and isinstance(patch["description"], str):
            todo.description = patch["description"]
        if "priority" in patch:
            try:
                todo.priority = int(patch["priority"])
            except (TypeError, ValueError):
                pass
        if "status" in patch and isinstance(patch["status"], str):
            todo.status = patch["status"]  # type: ignore[assignment]
        if "dependencies" in patch and isinstance(patch["dependencies"], list):
            todo.dependencies = [str(x) for x in patch["dependencies"]]
        todo.updated_at = utc_now_iso()
        self.todos.save(todo.id, todo)
        return todo

    def batch_rerun(self, todo_ids: Iterable[str]) -> List[TodoItem]:
        affected: List[TodoItem] = []
        for tid in todo_ids:
            todo = self.todos.get(tid)
            if todo is None:
                continue
            todo.status = "queued"  # type: ignore[assignment]
            todo.last_error = None
            todo.retry_count = int(todo.retry_count or 0) + 1
            todo.updated_at = utc_now_iso()
            if not self.pending_dependencies(tid):
                todo.status = "ready"  # type: ignore[assignment]
            self.todos.save(todo.id, todo)
            affected.append(todo)
        return affected
