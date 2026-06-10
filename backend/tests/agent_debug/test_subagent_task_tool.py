"""子代理（Task）委派工具：runtime 拦截、受限 allowlist 防递归、并行事件标签、summary 回填。"""

from __future__ import annotations

import asyncio
from typing import Any, AsyncIterator, Dict, List

from src.agent_debug.domain.context_manager import SessionContextManager
from src.agent_debug.domain.runtime import AgentRuntimeService
from src.agent_debug.domain.subagent_orchestrator import SubagentOrchestrator
from src.agent_debug.domain.summary_manager import SummaryManager
from src.agent_debug.domain.todo_engine import TodoEngine
from src.agent_debug.domain.tools.base import (
    FunctionTool,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.tools.subagent_tools import TaskTool
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.provider.base import (
    ModelRequestContext,
    ProviderRegistry,
    ProviderResponse,
    ToolCall,
)
from src.agent_debug.provider.service import ProviderExecutionService


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


def _registry_with_task_and_grep() -> WorkspaceToolRegistry:
    registry = WorkspaceToolRegistry()
    registry.register(TaskTool())

    async def _grep(args: Dict[str, Any], ctx: Any) -> ToolResult:
        return ToolResult(output={"hits": 3}, text=f"grep:{args.get('pattern')}")

    # ``grep`` is part of the explorer profile allowlist, so a sub-agent can use it.
    registry.register(
        FunctionTool(
            name="grep",
            description="search",
            parameters={"type": "object", "properties": {"pattern": {"type": "string"}}},
            fn=_grep,
        )
    )
    return registry


def _tool_names(request: Dict[str, Any]) -> List[str]:
    return [t["function"]["name"] for t in (request.get("tools") or [])]


class _TaskWithNestedToolProvider:
    """Parent delegates one Task (explorer); the sub-agent runs grep then summarizes."""

    def __init__(self) -> None:
        self.parent_calls = 0
        self.sub_calls = 0
        self.sub_tool_names_seen: List[List[str]] = []

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        names = _tool_names(request)
        messages = request.get("messages") or []
        has_tool_result = any(m.get("role") == "tool" for m in messages)
        if "Task" in names:
            self.parent_calls += 1
            if not has_tool_result:
                return ProviderResponse(
                    provider="x",
                    model=ctx.model,
                    output={"role": "assistant", "content": ""},
                    tool_calls=[
                        ToolCall(
                            id="task_1",
                            name="Task",
                            arguments={
                                "description": "探索后端",
                                "prompt": "探索 backend 模块结构",
                                "subagent_type": "explorer",
                            },
                        )
                    ],
                    finish_reason="tool_calls",
                )
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": "父代理综合完成。"},
                finish_reason="stop",
            )
        # Sub-agent turn (no Task tool exposed).
        self.sub_calls += 1
        self.sub_tool_names_seen.append(names)
        if not has_tool_result:
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": ""},
                tool_calls=[ToolCall(id="sub_grep_1", name="grep", arguments={"pattern": "foo"})],
                finish_reason="tool_calls",
            )
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": "子代理探索摘要：找到 3 个文件。"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}  # pragma: no cover - marks this as an async generator

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


def test_task_tool_spawns_subagent_with_restricted_allowlist_and_summary():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _TaskWithNestedToolProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus, tool_registry=_registry_with_task_and_grep())

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_task", user_message="请委派子代理探索后端")
    )

    assert out["message"]["text"] == "父代理综合完成。"

    # 子代理永远看不到 Task（防递归），但能看到其画像允许的 grep。
    assert provider.sub_tool_names_seen, "sub-agent should have been invoked"
    for names in provider.sub_tool_names_seen:
        assert "Task" not in names
        assert "grep" in names

    events = event_bus.snapshot("sess_task")
    types = [e["type"] for e in events]
    assert "subagent.created" in types
    assert "subagent.completed" in types

    created = next(e for e in events if e["type"] == "subagent.created")
    assert created["payload"]["parentToolCallId"] == "task_1"
    assert created["payload"]["subagentType"] == "explorer"

    # Task 工具卡的完成事件携带子代理摘要。
    task_completed = [
        e
        for e in events
        if e["type"] == "agent.tool.completed" and e["payload"].get("name") == "Task"
    ]
    assert task_completed
    assert "子代理探索摘要" in task_completed[-1]["payload"]["output"]
    assert task_completed[-1]["payload"]["toolCallId"] == "task_1"

    # 子代理内部的 grep 事件带 parentToolCallId / subagentId，供前端嵌套渲染。
    nested_grep = [
        e
        for e in events
        if e["type"] == "agent.tool.invoked"
        and e["payload"].get("name") == "grep"
    ]
    assert nested_grep
    assert nested_grep[0]["payload"].get("parentToolCallId") == "task_1"
    assert nested_grep[0]["payload"].get("subagentId")


class _TwoParallelTasksProvider:
    """Parent delegates two Tasks in one turn; each sub-agent echoes its prompt."""

    def __init__(self) -> None:
        self.parent_calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        names = _tool_names(request)
        messages = request.get("messages") or []
        has_tool_result = any(m.get("role") == "tool" for m in messages)
        if "Task" in names:
            self.parent_calls += 1
            if not has_tool_result:
                return ProviderResponse(
                    provider="x",
                    model=ctx.model,
                    output={"role": "assistant", "content": ""},
                    tool_calls=[
                        ToolCall(
                            id="task_a",
                            name="Task",
                            arguments={"description": "任务A", "prompt": "PROMPT-A", "subagent_type": "general"},
                        ),
                        ToolCall(
                            id="task_b",
                            name="Task",
                            arguments={"description": "任务B", "prompt": "PROMPT-B", "subagent_type": "general"},
                        ),
                    ],
                    finish_reason="tool_calls",
                )
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": "两路子代理已汇总。"},
                finish_reason="stop",
            )
        # Sub-agent turn: echo its seed prompt (last user message) as summary.
        user_msgs = [m for m in messages if m.get("role") == "user"]
        prompt = user_msgs[-1]["content"] if user_msgs else ""
        return ProviderResponse(
            provider="x",
            model=ctx.model,
            output={"role": "assistant", "content": f"完成:{prompt}"},
            finish_reason="stop",
        )

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}  # pragma: no cover

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


class _FailingTaskProvider:
    def __init__(self) -> None:
        self.parent_calls = 0

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        names = _tool_names(request)
        messages = request.get("messages") or []
        has_tool_result = any(m.get("role") == "tool" for m in messages)
        if "Task" in names:
            self.parent_calls += 1
            if not has_tool_result:
                return ProviderResponse(
                    provider="x",
                    model=ctx.model,
                    output={"role": "assistant", "content": ""},
                    tool_calls=[
                        ToolCall(
                            id="task_fails",
                            name="Task",
                            arguments={
                                "description": "失败子代理",
                                "prompt": "这个子代理会失败",
                                "subagent_type": "general",
                            },
                        )
                    ],
                    finish_reason="tool_calls",
                )
            return ProviderResponse(
                provider="x",
                model=ctx.model,
                output={"role": "assistant", "content": "父代理记录失败后完成。"},
                finish_reason="stop",
            )
        raise RuntimeError("子代理 provider 异常")

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="x", model=ctx.model, output={})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        return
        yield {}  # pragma: no cover

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True}


def test_parallel_tasks_emit_distinct_parent_tool_call_ids_and_summaries():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _TwoParallelTasksProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus, tool_registry=_registry_with_task_and_grep())

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_parallel", user_message="并行委派两个子代理")
    )

    assert out["message"]["text"] == "两路子代理已汇总。"

    events = event_bus.snapshot("sess_parallel")
    created = [e for e in events if e["type"] == "subagent.created"]
    completed = [e for e in events if e["type"] == "subagent.completed"]
    assert len(created) == 2
    assert len(completed) == 2

    parents = {e["payload"]["parentToolCallId"] for e in created}
    assert parents == {"task_a", "task_b"}

    # 每个子代理的摘要回填到对应 Task 工具完成事件。
    task_completed = {
        e["payload"]["toolCallId"]: e["payload"]["output"]
        for e in events
        if e["type"] == "agent.tool.completed" and e["payload"].get("name") == "Task"
    }
    assert task_completed.get("task_a") == "完成:PROMPT-A"
    assert task_completed.get("task_b") == "完成:PROMPT-B"


def test_failed_task_subagent_is_recorded_in_final_work_log():
    event_bus = EventBus()
    registry = ProviderRegistry()
    provider = _FailingTaskProvider()
    registry.register("package-agent", provider)
    provider_service = ProviderExecutionService(registry, event_bus)
    runtime = _make_runtime(provider_service, event_bus, tool_registry=_registry_with_task_and_grep())

    out = asyncio.run(
        runtime.run_composer_chat(session_id="sess_task_failure_log", user_message="委派失败子代理")
    )

    assert out["message"]["text"] == "父代理记录失败后完成。"
    events = event_bus.snapshot("sess_task_failure_log")
    assert any(event["type"] == "subagent.failed" for event in events)
    completed = next(event for event in events if event["type"] == "agent.completed")
    summary = completed["payload"]["workLogSummary"]
    assert summary["subagentFailures"]
    assert summary["subagentFailures"][0]["parentToolCallId"] == "task_fails"
    assert "子代理 provider 异常" in summary["subagentFailures"][0]["message"]
    assert any(entry.get("role") == "subagent" and entry.get("status") == "failed" for entry in runtime.get_run_logs(out["run"]["id"]))


def test_task_tool_schema_registered_and_excludes_recursion_by_profile():
    from src.agent_debug.prompts.builtin_subagents import (
        BUILTIN_SUBAGENTS,
        DEFAULT_READONLY_TOOLS,
        DEFAULT_WRITE_TOOLS,
    )

    # 任何内置画像或默认工具集都不包含 Task。
    for profile in BUILTIN_SUBAGENTS:
        assert "Task" not in profile.allowed_tools
    assert "Task" not in DEFAULT_READONLY_TOOLS
    assert "Task" not in DEFAULT_WRITE_TOOLS

    registry = _registry_with_task_and_grep()
    schemas = registry.json_schemas()
    names = [s["function"]["name"] for s in schemas]
    assert "Task" in names


def test_action_mode_profiles_expose_task_but_ask_does_not():
    """回归保护：build/debug/multitask/plan 必须暴露 Task，ask 不暴露。"""
    from src.agent_debug.prompts.composer_mode_prompts import resolve_composer_profile

    for mode in ("build", "debug", "multitask", "plan"):
        assert "Task" in resolve_composer_profile(mode).allowed_tools, mode
    assert "Task" not in resolve_composer_profile("ask").allowed_tools

    # 模拟 gateway 的交集：profile.allowed_tools ∩ session 工具（含 Task）。
    session_tools = [
        "read_file",
        "list_dir",
        "grep",
        "write_file",
        "create_document",
        "delete_file",
        "run_command",
        "check_command_status",
        "stop_command",
        "write_todos",
        "Task",
    ]
    build_allowed = [n for n in resolve_composer_profile("build").allowed_tools if n in session_tools]
    assert "Task" in build_allowed
    assert "delete_file" in build_allowed
    assert "run_command" in build_allowed
    assert "check_command_status" in build_allowed
    assert "stop_command" in build_allowed
    ask_allowed = [n for n in resolve_composer_profile("ask").allowed_tools if n in session_tools]
    assert "Task" not in ask_allowed
