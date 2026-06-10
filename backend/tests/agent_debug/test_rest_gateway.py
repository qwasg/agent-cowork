import asyncio

from src.agent_debug.api.rest_gateway import AgentDebugRestGateway
from src.agent_debug.domain.session_service import SessionService
from src.agent_debug.provider.base import ModelRequestContext, ProviderRegistry, ProviderResponse, ToolCall


class _GatewayTodoThenTextProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request, ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls % 2 == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id="call_gateway_todo",
                        name="write_todos",
                        arguments={
                            "todos": [
                                {"id": "a", "content": "同名任务", "status": "in_progress"},
                            ]
                        },
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "完成"},
            finish_reason="stop",
        )

    async def responses(self, request, ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request, ctx: ModelRequestContext):
        return
        yield {}

    async def healthcheck(self):
        return {"ok": True}


class _GatewayTodoThenEmptyProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request, ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id="call_gateway_todo_fail",
                        name="write_todos",
                        arguments={
                            "todos": [
                                {"id": "a", "content": "待继续任务", "status": "in_progress"},
                            ]
                        },
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": ""},
            finish_reason="stop",
        )

    async def responses(self, request, ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request, ctx: ModelRequestContext):
        return
        yield {}

    async def healthcheck(self):
        return {"ok": True}


class _GatewayUnknownToolThenTextProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request, ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id="call_missing_tool",
                        name="missing_tool",
                        arguments={"value": "x"},
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "已记录工具问题。"},
            finish_reason="stop",
        )

    async def responses(self, request, ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request, ctx: ModelRequestContext):
        return
        yield {}

    async def healthcheck(self):
        return {"ok": True}


def test_rest_gateway_executes_plan_and_emits_replay_events():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("demo")["session"]
    plan = asyncio.run(
        gateway.generate_plan(
            session["id"],
            "/plan build runtime",
            context_window={
                "activeFile": {
                    "path": "frontend/src/main.ts",
                    "language": "typescript",
                    "content": "const answer: number = '42';\nconsole.log(answer);\n",
                },
                "selection": {
                    "start": 0,
                    "end": 26,
                    "selectedText": "const answer: number = '42';",
                    "startLine": 1,
                    "endLine": 1,
                },
                "terminalRecentOutput": "error TS2322: Type 'string' is not assignable to type 'number'.",
            },
        )
    )["plan"]

    result = asyncio.run(gateway.execute_plan(plan["id"]))
    replay = gateway.get_replay(session["id"])

    assert result["run"]["id"]
    assert replay["events"]
    assert any(event["type"] == "subagent.summary.generated" for event in replay["events"])
    proposal = next(event for event in replay["events"] if event["type"] == "agent.code_edit.proposed")
    assert proposal["payload"]["schemaVersion"] == "ai-edit-proposal/v1"
    assert proposal["payload"]["changes"][0]["proposedContent"].startswith("const answer: number = 42;")


def test_rest_gateway_can_pause_resume_cancel_run():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("demo")["session"]
    run_obj = asyncio.run(gateway.runtime.start(session["id"], "plan_test", "long running"))
    gateway.sessions.update_active_run(session["id"], run_obj.id, status="running")
    run = {"id": run_obj.id}

    assert asyncio.run(gateway.pause_run(run["id"]))["accepted"] is True
    assert asyncio.run(gateway.resume_run(run["id"]))["accepted"] is True
    assert asyncio.run(gateway.cancel_run(run["id"]))["accepted"] is True

    types = [event["type"] for event in gateway.get_replay(session["id"])["events"]]
    assert "agent.paused" in types
    assert "agent.resumed" in types
    assert asyncio.run(gateway.pause_run(run["id"]))["accepted"] is False


def test_rest_gateway_run_logs_include_final_summary_for_tool_failures():
    registry = ProviderRegistry()
    registry.register("package-agent", _GatewayUnknownToolThenTextProvider())
    gateway = AgentDebugRestGateway(provider_registry=registry)
    session = gateway.create_session("demo")["session"]

    out = asyncio.run(
        gateway.ask_execute(
            session["id"],
            "触发缺失工具",
            composer_mode="build",
        )
    )
    logs = gateway.get_run_logs(out["run"]["id"])

    assert logs["summary"]["toolFailures"][0]["name"] == "missing_tool"
    assert logs["summary"]["toolFailures"][0]["code"] == "TOOL_NOT_AVAILABLE"
    assert any(entry.get("role") == "work_log_summary" for entry in logs["entries"])


def test_rest_gateway_auto_titles_default_session_from_first_prompt():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("新的 Agent Debug 会话")["session"]

    out = asyncio.run(
        gateway.ask_execute(
            session["id"],
            "/debug 修复 Agent 输出保留与自动命名。请检查前后端状态同步。",
            composer_mode="build",
        )
    )
    updated = gateway.get_session(session["id"])["session"]

    assert out["run"]["id"]
    assert updated["title"] == "修复 Agent 输出保留与自动命名"


def test_rest_gateway_does_not_auto_title_manual_session():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("手动标题")["session"]

    asyncio.run(
        gateway.ask_execute(
            session["id"],
            "这条消息不应该覆盖标题",
            composer_mode="build",
        )
    )
    updated = gateway.get_session(session["id"])["session"]

    assert updated["title"] == "手动标题"


def test_rest_gateway_does_not_auto_title_manual_default_string():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("demo")["session"]
    gateway.patch_session(session["id"], {"title": "新的 Agent Debug 会话"})

    asyncio.run(
        gateway.ask_execute(
            session["id"],
            "这条消息不应该覆盖用户手动设置的默认文案",
            composer_mode="build",
        )
    )
    updated = gateway.get_session(session["id"])["session"]

    assert updated["title"] == "新的 Agent Debug 会话"


def test_rest_gateway_generate_plan_auto_titles_default_session():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("Agent Debug Session")["session"]

    asyncio.run(gateway.generate_plan(session["id"], "/plan 构建文档编译流程。"))
    updated = gateway.get_session(session["id"])["session"]

    assert updated["title"] == "构建文档编译流程"


def test_rest_gateway_plan_execution_finishes_session_and_plan_status():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("demo")["session"]
    plan = asyncio.run(gateway.generate_plan(session["id"], "/plan build runtime"))["plan"]

    run = asyncio.run(gateway.execute_plan(plan["id"]))["run"]
    snapshot = gateway.get_design_snapshot(session["id"])
    task_statuses = [task["status"] for task in snapshot["planBundle"]["tasks"]]
    types = [event["type"] for event in gateway.get_replay(session["id"])["events"]]

    assert run["status"] == "completed"
    assert snapshot["activeSession"]["status"] == "idle"
    assert snapshot["activeSession"]["activeRunId"] is None
    assert snapshot["planBundle"]["plan"]["status"] == "completed"
    assert task_statuses == ["completed"]
    assert "agent.completed" in types


def test_rest_gateway_rejects_control_for_completed_run():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("demo")["session"]
    plan = asyncio.run(gateway.generate_plan(session["id"], "/plan build runtime"))["plan"]
    run = asyncio.run(gateway.execute_plan(plan["id"]))["run"]

    assert asyncio.run(gateway.pause_run(run["id"]))["accepted"] is False
    assert asyncio.run(gateway.cancel_run(run["id"]))["accepted"] is False


def test_rest_gateway_pin_does_not_change_session_updated_at():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("demo")["session"]
    before = session["updatedAt"]

    updated = gateway.patch_session(session["id"], {"pinned": True})["session"]

    assert updated["pinned"] is True
    assert updated["updatedAt"] == before


def test_rest_gateway_seed_swarm_demo_returns_nodes_and_shards():
    gateway = AgentDebugRestGateway()
    result = asyncio.run(gateway.seed_swarm_demo())

    assert result["nodes"]
    assert result["shards"]


def test_rest_gateway_local_file_roundtrip_and_validation(tmp_path):
    gateway = AgentDebugRestGateway()
    target = tmp_path / "notes.txt"

    write_result = gateway.write_local_file(str(target), "hello")
    assert write_result["path"] == str(target.resolve())
    assert target.read_text(encoding="utf-8") == "hello"

    read_result = gateway.read_local_file(str(target))
    assert read_result["content"] == "hello"

    assert gateway.read_local_file("notes.txt")["error"]["code"] == "INVALID_PATH"
    assert gateway.read_local_file(str(tmp_path))["error"]["code"] == "PATH_IS_DIRECTORY"

    big_file = tmp_path / "big.txt"
    big_file.write_bytes(b"x" * (5 * 1024 * 1024 + 1))
    assert gateway.read_local_file(str(big_file))["error"]["code"] == "INVALID_PATH"


def test_rest_gateway_keeps_unfinished_agent_todos_visible_after_success():
    registry = ProviderRegistry()
    registry.register("package-agent", _GatewayTodoThenTextProvider())
    gateway = AgentDebugRestGateway(provider_registry=registry)
    session = gateway.create_session("demo")["session"]

    out1 = asyncio.run(
        gateway.ask_execute(session["id"], "第一次任务", composer_mode="build")
    )
    snapshot = gateway.get_design_snapshot(session["id"])
    out2 = asyncio.run(
        gateway.ask_execute(session["id"], "第二次任务", composer_mode="build")
    )

    assert out1["run"]["id"]
    assert out2["run"]["id"]
    assert len(snapshot["todos"]) == 1
    assert snapshot["todos"][0]["title"] == "同名任务"
    assert gateway.get_todos(session["id"])["items"]
    all_todos = gateway.todo_engine.list_by_session(session["id"])
    assert len(all_todos) == 1
    assert not any(todo.archived_at for todo in all_todos)
    assert all_todos[0].title == "同名任务"


def test_rest_gateway_keeps_failed_agent_todos_visible_and_session_resumable():
    registry = ProviderRegistry()
    registry.register("package-agent", _GatewayTodoThenEmptyProvider())
    gateway = AgentDebugRestGateway(provider_registry=registry)
    session = gateway.create_session("demo")["session"]

    out = asyncio.run(
        gateway.ask_execute(session["id"], "失败任务", composer_mode="build")
    )
    snapshot = gateway.get_design_snapshot(session["id"])
    stored = gateway.sessions.get(session["id"])

    assert "未生成可展示文本" in out["message"]["text"]
    assert stored is not None
    assert stored.active_run_id == out["run"]["id"]
    assert stored.status == "failed"
    assert len(snapshot["todos"]) == 1
    assert snapshot["todos"][0]["title"] == "待继续任务"
    assert snapshot["activeSession"]["activeRunId"] == out["run"]["id"]


def test_delete_session_persists_and_cascades_state():
    gateway = AgentDebugRestGateway()
    session = gateway.create_session("待删除")["session"]
    session_id = session["id"]

    gateway.add_todo({"sessionId": session_id, "title": "todo-1"})
    plan = asyncio.run(gateway.generate_plan(session_id, "/plan build runtime"))["plan"]
    assert gateway.todo_engine.list_by_session(session_id)
    assert gateway.plan_engine.plans.list_by("session_id", session_id)

    result = gateway.delete_session(session_id)
    assert result == {"deleted": True, "id": session_id}

    # 会话索引层面已删除，且文件已持久化（重新加载同一锚定文件不应复活）。
    assert gateway.sessions.get(session_id) is None
    reloaded = SessionService(data_file=gateway.sessions.data_file)
    assert reloaded.get(session_id) is None

    # 级联清理：todo / plan / 事件缓冲均被移除。
    assert not gateway.todo_engine.list_by_session(session_id)
    assert not gateway.plan_engine.plans.list_by("session_id", session_id)
    assert not gateway.plan_engine.get(plan["id"])
    assert gateway.event_bus.snapshot(session_id) == []
    assert session_id not in gateway.session_context_windows


def test_delete_session_unknown_returns_error():
    gateway = AgentDebugRestGateway()
    result = gateway.delete_session("sess_does_not_exist")
    assert result["error"]["code"] == "SESSION_NOT_FOUND"
