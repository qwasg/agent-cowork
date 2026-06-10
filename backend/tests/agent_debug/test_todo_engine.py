import asyncio

from src.agent_debug.domain.plan_engine import PlanEngine
from src.agent_debug.domain.todo_engine import TodoEngine
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.provider.service import ProviderExecutionService, build_provider_registry


def test_todo_engine_materializes_from_plan_tasks():
    plan_engine = PlanEngine(ProviderExecutionService(build_provider_registry(), EventBus()))
    todo_engine = TodoEngine()
    result = asyncio.run(plan_engine.generate("sess_1", "/plan test todo"))

    todos = todo_engine.materialize_from_tasks("sess_1", result["tasks"])

    assert len(todos) == 1
    assert todos[0].session_id == "sess_1"
    assert todos[0].related_plan_node_ids == [result["tasks"][0].id]


def test_sync_agent_todos_creates_with_status_mapping():
    engine = TodoEngine()
    changes = engine.sync_agent_todos(
        "sess_1",
        [
            {"id": "a", "content": "Read the runtime", "status": "in_progress"},
            {"id": "b", "content": "Add the tool", "status": "pending"},
        ],
        run_id="run_1",
    )

    assert [kind for _, kind in changes] == ["created", "created"]
    todos = {t.title: t for t, _ in changes}
    assert todos["Read the runtime"].status == "running"
    assert todos["Read the runtime"].source == "agent"
    assert todos["Read the runtime"].related_agent_run_id == "run_1"
    assert todos["Add the tool"].status == "ready"
    # Full list is persisted on the session.
    assert len(engine.list_by_session("sess_1")) == 2


def test_sync_agent_todos_updates_same_items_by_id():
    engine = TodoEngine()
    first = engine.sync_agent_todos(
        "sess_1",
        [
            {"id": "a", "content": "Step one", "status": "in_progress"},
            {"id": "b", "content": "Step two", "status": "pending"},
        ],
    )
    created_id = next(t.id for t, _ in first if t.title == "Step one")

    second = engine.sync_agent_todos(
        "sess_1",
        [
            {"id": "a", "content": "Step one", "status": "completed"},
            {"id": "b", "content": "Step two", "status": "in_progress"},
        ],
    )

    by_title = {t.title: (t, kind) for t, kind in second}
    # Same internal ids are reused (no duplicate todos).
    assert by_title["Step one"][0].id == created_id
    assert by_title["Step one"][1] == "completed"
    assert by_title["Step one"][0].status == "completed"
    assert by_title["Step two"][1] == "running"
    assert len(engine.list_by_session("sess_1")) == 2


def test_sync_agent_todos_matches_by_title_without_id():
    engine = TodoEngine()
    engine.sync_agent_todos("sess_1", [{"content": "Refactor module", "status": "pending"}])
    again = engine.sync_agent_todos(
        "sess_1", [{"content": "refactor   module", "status": "completed"}]
    )

    assert len(engine.list_by_session("sess_1")) == 1
    todo, kind = again[0]
    assert kind == "completed"
    assert todo.status == "completed"


def test_sync_agent_todos_rebinds_reused_items_to_latest_run():
    engine = TodoEngine()
    engine.sync_agent_todos(
        "sess_1",
        [{"id": "a", "content": "Inspect runtime", "status": "in_progress"}],
        run_id="run_1",
    )

    again = engine.sync_agent_todos(
        "sess_1",
        [{"id": "a", "content": "Inspect runtime", "status": "completed"}],
        run_id="run_2",
    )

    todo, kind = again[0]
    assert kind == "completed"
    assert todo.related_agent_run_id == "run_2"


def test_complete_agent_todos_for_run_only_reports_already_completed_items():
    engine = TodoEngine()
    engine.sync_agent_todos(
        "sess_1",
        [
            {"id": "a", "content": "Current step", "status": "completed"},
            {"id": "b", "content": "Queued step", "status": "in_progress"},
        ],
        run_id="run_1",
    )
    manual = engine.add_manual("sess_1", "Manual follow-up")
    engine.update_status(manual.id, "running")

    completed = engine.complete_agent_todos_for_run("run_1")

    assert {todo.title for todo in completed} == {"Current step"}
    by_title = {todo.title: todo for todo in engine.list_by_session("sess_1")}
    assert by_title["Current step"].status == "completed"
    assert by_title["Queued step"].status == "running"
    assert by_title["Manual follow-up"].status == "running"


def test_sync_agent_todos_archives_items_missing_from_complete_list():
    engine = TodoEngine()
    first = engine.sync_agent_todos(
        "sess_1",
        [
            {"id": "a", "content": "Keep", "status": "pending"},
            {"id": "b", "content": "Remove", "status": "pending"},
        ],
    )

    second = engine.sync_agent_todos(
        "sess_1",
        [{"id": "a", "content": "Keep", "status": "completed"}],
    )

    by_title = {todo.title: todo for todo in engine.list_by_session("sess_1")}
    assert by_title["Keep"].status == "completed"
    assert by_title["Remove"].status == "cancelled"
    assert by_title["Remove"].archived_at
    assert any(todo.title == "Remove" for todo, _ in second)


def test_sync_agent_todos_keeps_duplicate_titles_as_distinct_without_ids():
    engine = TodoEngine()

    engine.sync_agent_todos(
        "sess_1",
        [
            {"content": "Fix bug", "status": "pending"},
            {"content": "fix   bug", "status": "in_progress"},
        ],
    )

    todos = engine.list_by_session("sess_1")
    assert len(todos) == 2
    assert len({todo.id for todo in todos}) == 2


def test_archived_agent_group_is_hidden_from_default_views_and_not_reused():
    engine = TodoEngine()
    first = engine.sync_agent_todos(
        "sess_1",
        [{"id": "a", "content": "Same title", "status": "completed"}],
        run_id="run_1",
    )
    archived_group_id = first[0][0].agent_todo_group_id
    assert archived_group_id is not None

    archived = engine.archive_agent_group("sess_1", archived_group_id)
    assert len(archived) == 1
    assert engine.list_default_visible_by_session("sess_1") == []

    second = engine.sync_agent_todos(
        "sess_1",
        [{"id": "a", "content": "Same title", "status": "in_progress"}],
        run_id="run_2",
    )
    assert second[0][0].id != first[0][0].id
    assert second[0][0].agent_todo_group_id != archived_group_id
    visible = engine.list_default_visible_by_session("sess_1")
    assert len(visible) == 1
    assert visible[0].id == second[0][0].id


def test_default_visible_agent_group_prefers_unarchived_resumable_group():
    engine = TodoEngine()
    first = engine.sync_agent_todos(
        "sess_1",
        [{"id": "a", "content": "First run", "status": "completed"}],
        run_id="run_1",
    )
    engine.archive_agent_group("sess_1", first[0][0].agent_todo_group_id or "")

    second = engine.sync_agent_todos(
        "sess_1",
        [{"id": "b", "content": "Resumable run", "status": "in_progress"}],
        run_id="run_2",
    )

    visible = engine.list_default_visible_by_session("sess_1")
    assert len(visible) == 1
    assert visible[0].title == "Resumable run"
    assert visible[0].archived_at is None
    assert visible[0].agent_todo_group_id == second[0][0].agent_todo_group_id
