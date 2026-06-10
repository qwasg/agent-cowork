"""会话级带工具的 ReAct 流式对话（run_composer_chat）与工具事件关联。"""

from __future__ import annotations

import asyncio
from typing import Any, AsyncIterator, Dict

from src.agent_debug.domain import runtime as runtime_module
from src.agent_debug.domain.context_compactor import ContextCompactor
from src.agent_debug.domain.context_manager import SessionContextManager
from src.agent_debug.domain.runtime import AgentRuntimeService
from src.agent_debug.domain.subagent_orchestrator import SubagentOrchestrator
from src.agent_debug.domain.summary_manager import SummaryManager
from src.agent_debug.domain.todo_engine import TodoEngine
from src.agent_debug.domain.tools.base import (
    FunctionTool,
    ToolExecutionError,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.provider.base import (
    ModelRequestContext,
    ProviderRegistry,
    ProviderResponse,
    ToolCall,
)
from src.agent_debug.provider.service import ProviderExecutionService, build_provider_registry


def _make_runtime(provider_service: ProviderExecutionService, event_bus: EventBus, tool_registry=None):
    return AgentRuntimeService(
        todo_engine=TodoEngine(),
        subagent_orchestrator=SubagentOrchestrator(),
        summary_manager=SummaryManager(provider_service),
        event_bus=event_bus,
        context_manager=SessionContextManager(),
        provider_service=provider_service,
        tool_registry=tool_registry,
    )


def _types(events):
    return [e["type"] for e in events]


def test_run_composer_chat_emits_lifecycle_and_returns_text():
    event_bus = EventBus()
    provider_service = ProviderExecutionService(build_provider_registry(), event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    out = asyncio.run(
        runtime.run_composer_chat(
            session_id="sess_chat",
            user_message="你好",
            system_message="system",
        )
    )

    assert out["run"]["id"]
    assert isinstance(out["message"]["text"], str) and out["message"]["text"].strip()
    types = _types(event_bus.snapshot("sess_chat"))
    assert "agent.started" in types
    assert "agent.completed" in types


class _ToolThenTextProvider:
    """First step requests a tool call; second step returns final text."""

    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[ToolCall(id="call_1", name="echo_tool", arguments={"text": "hi"})],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "完成。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        # No unified-protocol events → execution service falls back to chat().
        return
        yield {}  # pragma: no cover - marks this as an async generator

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _ToolThenEmptyProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[ToolCall(id="call_empty", name="echo_tool", arguments={"text": "hi"})],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": ""},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _NamedToolThenTextProvider:
    def __init__(self, tool_name: str, arguments: Dict[str, Any] | None = None) -> None:
        self.tool_name = tool_name
        self.arguments = arguments or {}
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id=f"call_{self.tool_name}",
                        name=self.tool_name,
                        arguments=self.arguments,
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "最终完成。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _TodoThenTextProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id="call_todos_auto_finish",
                        name="write_todos",
                        arguments={
                            "todos": [
                                {
                                    "id": "t1",
                                    "content": "完成最终检查",
                                    "status": "in_progress",
                                }
                            ]
                        },
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "全部处理完成。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _TodoThenEmptyProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id="call_todos_fail",
                        name="write_todos",
                        arguments={
                            "todos": [
                                {
                                    "id": "t1",
                                    "content": "保留待继续任务",
                                    "status": "in_progress",
                                }
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

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


def test_run_composer_chat_tool_events_carry_tool_call_id_and_output():
    event_bus = EventBus()
    registry = ProviderRegistry()
    registry.register("package-agent", _ToolThenTextProvider())
    provider_service = ProviderExecutionService(registry, event_bus)

    tool_registry = WorkspaceToolRegistry()

    async def _echo(args: Dict[str, Any], ctx: Any) -> ToolResult:
        return ToolResult(output={"echo": args.get("text")}, text=f"echo:{args.get('text')}")

    tool_registry.register(
        FunctionTool(
            name="echo_tool",
            description="echo back",
            parameters={"type": "object", "properties": {"text": {"type": "string"}}},
            fn=_echo,
        )
    )
    runtime = _make_runtime(provider_service, event_bus, tool_registry=tool_registry)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_tool", user_message="echo hi")
    )

    assert "完成" in out["message"]["text"]
    events = event_bus.snapshot("sess_tool")
    invoked = [e for e in events if e["type"] == "agent.tool.invoked"]
    completed = [e for e in events if e["type"] == "agent.tool.completed"]
    assert invoked and invoked[0]["payload"]["toolCallId"] == "call_1"
    assert invoked[0]["payload"]["name"] == "echo_tool"
    assert completed and completed[0]["payload"]["toolCallId"] == "call_1"
    assert completed[0]["payload"]["output"] == "echo:hi"


def test_run_composer_chat_records_tool_failure_in_work_log_summary():
    event_bus = EventBus()
    registry = ProviderRegistry()
    registry.register("package-agent", _NamedToolThenTextProvider("broken_tool", {"path": "missing.txt"}))
    provider_service = ProviderExecutionService(registry, event_bus)

    tool_registry = WorkspaceToolRegistry()

    async def _broken(args: Dict[str, Any], ctx: Any) -> ToolResult:
        raise ToolExecutionError("BROKEN_TOOL", "工具执行失败")

    tool_registry.register(
        FunctionTool(
            name="broken_tool",
            description="always fails",
            parameters={"type": "object", "properties": {"path": {"type": "string"}}},
            fn=_broken,
        )
    )
    runtime = _make_runtime(provider_service, event_bus, tool_registry=tool_registry)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_tool_failure_log", user_message="run broken")
    )
    run_id = out["run"]["id"]

    completed = next(e for e in event_bus.snapshot("sess_tool_failure_log") if e["type"] == "agent.completed")
    summary = completed["payload"]["workLogSummary"]
    assert summary["toolFailures"][0]["name"] == "broken_tool"
    assert summary["toolFailures"][0]["code"] == "BROKEN_TOOL"
    logs = runtime.get_run_logs(run_id)
    assert any(entry.get("role") == "tool" and entry.get("status") == "failed" for entry in logs)
    assert any(entry.get("role") == "work_log_summary" for entry in logs)


def test_run_composer_chat_records_command_result_in_work_log_summary():
    event_bus = EventBus()
    registry = ProviderRegistry()
    registry.register(
        "package-agent",
        _NamedToolThenTextProvider(
            "run_command",
            {"command": "exit 7", "shell": "powershell", "blocking": True},
        ),
    )
    provider_service = ProviderExecutionService(registry, event_bus)

    tool_registry = WorkspaceToolRegistry()

    async def _run_command(args: Dict[str, Any], ctx: Any) -> ToolResult:
        return ToolResult(
            output={
                "commandId": "cmd_1",
                "status": "failed",
                "exitCode": 7,
                "stdout": "",
                "stderr": "boom",
                "shell": args.get("shell"),
                "cwd": ".",
            },
            text="command cmd_1 finished with status failed exit_code=7",
        )

    tool_registry.register(
        FunctionTool(
            name="run_command",
            description="fake command runner",
            parameters={"type": "object", "properties": {"command": {"type": "string"}}},
            fn=_run_command,
        )
    )
    runtime = _make_runtime(provider_service, event_bus, tool_registry=tool_registry)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_command_log", user_message="run command")
    )

    completed = next(e for e in event_bus.snapshot("sess_command_log") if e["type"] == "agent.completed")
    commands = completed["payload"]["workLogSummary"]["commands"]
    assert commands[0]["name"] == "run_command"
    assert commands[0]["command"] == "exit 7"
    assert commands[0]["status"] == "failed"
    assert commands[0]["exitCode"] == 7
    assert runtime.get_run_log_summary(out["run"]["id"])["commands"][0]["stderr"] == "boom"


def test_run_composer_chat_keeps_unfinished_agent_todos_visible_when_finishing():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _TodoThenTextProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_auto_finish", user_message="处理任务")
    )

    assert out["message"]["text"] == "全部处理完成。"
    assert provider.calls == 2
    events = event_bus.snapshot("sess_auto_finish")
    completed_todos = [event for event in events if event["type"] == "todo.completed"]
    assert completed_todos == []
    todos = runtime.todo_engine.list_by_session("sess_auto_finish")
    assert len(todos) == 1
    assert todos[0].status == "running"
    assert todos[0].archived_at is None
    assert runtime.todo_engine.list_default_visible_by_session("sess_auto_finish")
    assert "agent.completed" in _types(events)


def test_run_composer_chat_fails_when_final_output_has_no_text():
    event_bus = EventBus()
    registry = ProviderRegistry()
    registry.register("package-agent", _ToolThenEmptyProvider())
    provider_service = ProviderExecutionService(registry, event_bus)

    tool_registry = WorkspaceToolRegistry()

    async def _echo(args: Dict[str, Any], ctx: Any) -> ToolResult:
        return ToolResult(output={"echo": args.get("text")}, text=f"echo:{args.get('text')}")

    tool_registry.register(
        FunctionTool(
            name="echo_tool",
            description="echo back",
            parameters={"type": "object", "properties": {"text": {"type": "string"}}},
            fn=_echo,
        )
    )
    runtime = _make_runtime(provider_service, event_bus, tool_registry=tool_registry)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_empty", user_message="echo hi")
    )

    assert "未生成可展示文本" in out["message"]["text"]
    events = event_bus.snapshot("sess_empty")
    assert "agent.failed" in _types(events)
    assert not any(event["type"] == "agent.completed" for event in events)
    failed = next(event for event in events if event["type"] == "agent.failed")
    assert failed["payload"]["errorCode"] == "EMPTY_ASSISTANT_OUTPUT"


def test_run_composer_chat_keeps_agent_todos_visible_when_failed():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _TodoThenEmptyProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_failed_todo", user_message="处理任务")
    )

    assert "未生成可展示文本" in out["message"]["text"]
    todos = runtime.todo_engine.list_by_session("sess_failed_todo")
    assert len(todos) == 1
    assert todos[0].status == "running"
    assert todos[0].archived_at is None
    visible = runtime.todo_engine.list_default_visible_by_session("sess_failed_todo")
    assert len(visible) == 1
    assert visible[0].title == "保留待继续任务"


class _RecordingProvider:
    """Records the ``messages`` array received on each chat call."""

    def __init__(self) -> None:
        self.seen_messages: list[list[Dict[str, Any]]] = []
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        self.seen_messages.append(list(request.get("messages") or []))
        return ProviderResponse(
            provider="rec",
            model=ctx.model,
            output={"role": "assistant", "content": f"回复{self.calls}"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="rec", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}  # pragma: no cover - marks this as an async generator

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


def test_run_composer_chat_persists_history_across_turns():
    """同一会话连续两轮：第二轮发给 provider 的 messages 应携带第一轮 user/assistant。"""
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _RecordingProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    out1 = asyncio.run(
        runtime.run_composer_chat(session_id="sess_mem", user_message="我叫小明")
    )
    out2 = asyncio.run(
        runtime.run_composer_chat(session_id="sess_mem", user_message="我叫什么")
    )

    assert out1["message"]["text"] == "回复1"
    assert out2["message"]["text"] == "回复2"

    # 第一轮只看到当前 user。
    first_turn = provider.seen_messages[0]
    assert [m["content"] for m in first_turn if m["role"] == "user"] == ["我叫小明"]

    # 第二轮应包含第一轮的 user + assistant，再加当前 user。
    second_turn = provider.seen_messages[-1]
    contents = [(m["role"], m["content"]) for m in second_turn]
    assert ("user", "我叫小明") in contents
    assert ("assistant", "回复1") in contents
    assert ("user", "我叫什么") in contents
    assert contents[-1] == ("user", "我叫什么")


def test_run_composer_chat_rebuilds_history_from_events():
    """内存丢失（模拟重启）时，应能从事件流重建会话历史。"""
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _RecordingProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    asyncio.run(runtime.run_composer_chat(session_id="sess_restart", user_message="第一句"))

    # 模拟进程重启：清空内存历史，但事件总线快照仍在。
    runtime._conversation_history.clear()

    asyncio.run(runtime.run_composer_chat(session_id="sess_restart", user_message="第二句"))

    second_turn = provider.seen_messages[-1]
    contents = [(m["role"], m["content"]) for m in second_turn]
    assert ("user", "第一句") in contents
    assert ("assistant", "回复1") in contents
    assert contents[-1] == ("user", "第二句")


class _SixthStepTodoThenTextProvider:
    """Consumes six tool-loop steps before returning final text."""

    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls < 6:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id=f"call_echo_{self.calls}",
                        name="echo_tool",
                        arguments={"text": f"step-{self.calls}"},
                    )
                ],
                finish_reason="tool_calls",
            )
        if self.calls == 6:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": "先列 todo。"},
                tool_calls=[
                    ToolCall(
                        id="call_todos",
                        name="write_todos",
                        arguments={
                            "todos": [
                                {
                                    "id": "t1",
                                    "content": "查看 backend 结构",
                                    "status": "in_progress",
                                },
                                {
                                    "id": "t2",
                                    "content": "总结发现",
                                    "status": "pending",
                                },
                            ]
                        },
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "继续执行并完成。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _AlwaysToolProvider:
    def __init__(self) -> None:
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": ""},
            tool_calls=[
                ToolCall(
                    id=f"call_todos_{self.calls}",
                    name="write_todos",
                    arguments={
                        "todos": [
                            {
                                "id": "t1",
                                "content": "持续请求工具",
                                "status": "in_progress",
                            }
                        ]
                    },
                )
            ],
            finish_reason="tool_calls",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _ManyToolsThenTextProvider:
    def __init__(self, tool_steps: int) -> None:
        self.tool_steps = tool_steps
        self.calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        self.calls += 1
        if self.calls <= self.tool_steps:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id=f"call_echo_{self.calls}",
                        name="echo_tool",
                        arguments={"text": f"step-{self.calls}"},
                    )
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": f"完成 {self.tool_steps} 步。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _MultiToolThenValidatingTextProvider:
    def __init__(self) -> None:
        self.calls = 0
        self.seen_messages: list[list[Dict[str, Any]]] = []

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        if not request.get("tools"):
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": "摘要"},
                finish_reason="stop",
            )
        self.calls += 1
        messages = list(request.get("messages") or [])
        self.seen_messages.append(messages)
        non_system = [message for message in messages if message.get("role") != "system"]
        assert ContextCompactor._is_valid_tool_sequence(non_system)
        if self.calls == 1:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[
                    ToolCall(
                        id=f"call_{idx}",
                        name="echo_tool",
                        arguments={"text": f"step-{idx}"},
                    )
                    for idx in range(3)
                ],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "压缩后继续完成。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


def _echo_tool_registry() -> WorkspaceToolRegistry:
    tool_registry = WorkspaceToolRegistry()

    async def _echo(args: Dict[str, Any], ctx: Any) -> ToolResult:
        return ToolResult(output={"echo": args.get("text")}, text=f"echo:{args.get('text')}")

    tool_registry.register(
        FunctionTool(
            name="echo_tool",
            description="echo back",
            parameters={"type": "object", "properties": {"text": {"type": "string"}}},
            fn=_echo,
        )
    )
    return tool_registry


def _long_echo_tool_registry() -> WorkspaceToolRegistry:
    tool_registry = WorkspaceToolRegistry()

    async def _echo(args: Dict[str, Any], ctx: Any) -> ToolResult:
        return ToolResult(
            output={"echo": args.get("text")},
            text=f"echo:{args.get('text')} " + ("长工具输出 " * 80),
        )

    tool_registry.register(
        FunctionTool(
            name="echo_tool",
            description="echo back with long content",
            parameters={"type": "object", "properties": {"text": {"type": "string"}}},
            fn=_echo,
        )
    )
    return tool_registry


def test_run_composer_chat_continues_after_todo_written_on_sixth_step():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _SixthStepTodoThenTextProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus, tool_registry=_echo_tool_registry())

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_sixth_todo", user_message="探索项目")
    )

    assert out["message"]["text"] == "继续执行并完成。"
    assert provider.calls == 7
    events = event_bus.snapshot("sess_sixth_todo")
    assert any(event["type"] == "todo.created" for event in events)
    write_completed = [
        event
        for event in events
        if event["type"] == "agent.tool.completed"
        and event["payload"]["name"] == "write_todos"
    ]
    assert write_completed
    assert "继续执行当前任务：查看 backend 结构" in write_completed[-1]["payload"]["output"]
    assert "agent.completed" in _types(events)


def test_run_composer_chat_default_loop_can_exceed_old_step_cap():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _ManyToolsThenTextProvider(tool_steps=25)
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus, tool_registry=_echo_tool_registry())

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_many_steps", user_message="多步执行")
    )

    assert out["message"]["text"] == "完成 25 步。"
    assert provider.calls == 26
    events = event_bus.snapshot("sess_many_steps")
    assert "agent.completed" in _types(events)
    assert not any(event["type"] == "agent.failed" for event in events)


def test_run_composer_chat_compaction_preserves_tool_pairs():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _MultiToolThenValidatingTextProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(
        provider_service,
        event_bus,
        tool_registry=_long_echo_tool_registry(),
    )
    runtime.context_compactor = ContextCompactor(
        provider_service,
        context_budget=100,
        keep_recent=2,
    )

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_compact_pairs", user_message="多工具压缩")
    )

    assert out["message"]["text"] == "压缩后继续完成。"
    assert provider.calls == 2
    assert len(provider.seen_messages) == 2
    assert len(provider.seen_messages[-1]) > 4
    events = event_bus.snapshot("sess_compact_pairs")
    assert "agent.context.compacted" in _types(events)
    assert "agent.completed" in _types(events)
    assert not any(event["type"] == "agent.failed" for event in events)


def test_run_composer_chat_reports_configured_tool_loop_exhaustion(monkeypatch):
    monkeypatch.setattr(runtime_module, "_TOOL_LOOP_MAX_STEPS", 2)
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _AlwaysToolProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_exhausted", user_message="一直用工具")
    )

    assert "已达到用户配置的工具循环上限" in out["message"]["text"]
    assert provider.calls == 2
    events = event_bus.snapshot("sess_exhausted")
    assert "agent.failed" in _types(events)
    assert not any(event["type"] == "agent.completed" for event in events)
    failed = next(event for event in events if event["type"] == "agent.failed")
    assert failed["payload"]["errorCode"] == "TOOL_LOOP_EXHAUSTED"


def test_run_composer_chat_stops_repeated_identical_tool_calls(monkeypatch):
    monkeypatch.setattr(runtime_module, "_TOOL_LOOP_MAX_STEPS", 0)
    monkeypatch.setattr(runtime_module, "_REPEATED_TOOL_LIMIT", 2)
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _AlwaysToolProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus)

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_repeated", user_message="重复工具")
    )

    assert "连续重复请求相同工具调用" in out["message"]["text"]
    assert provider.calls == 3
    events = event_bus.snapshot("sess_repeated")
    assert "agent.failed" in _types(events)
    assert not any(event["type"] == "agent.completed" for event in events)
    failed = next(event for event in events if event["type"] == "agent.failed")
    assert failed["payload"]["errorCode"] == "TOOL_LOOP_REPEATED"


def test_mcp_demo_tool_registers_when_available():
    """McpDemoTool 通过 _dispatch_tool 产出带 mcp__ 前缀的工具事件。"""
    from src.agent_debug.domain.tools.mcp_tools import McpDemoTool

    tool = McpDemoTool(remote_name="add", description="add two numbers")
    assert tool.name == "mcp__demo__add"
    assert tool.name.startswith("mcp__")
