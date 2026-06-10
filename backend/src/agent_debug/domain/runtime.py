"""Agent runtime: DAG plan execution + per-task ReAct tool loop.

Two execution surfaces:

- ``run_serial_subagent(run, todo_id, plan_node_id, objective)`` — kept for
  backwards compatibility with tests like ``test_runtime_summary``. Internally
  delegates to ``_execute_task``.
- ``run_plan(run, plan_bundle)`` — drives the entire plan: builds a
  todo-id → task DAG via ``TodoEngine.ready_todos`` and runs ready batches
  concurrently up to ``run.parallel_limit``. Failures emit ``plan.node.failed``
  but do NOT abort the whole plan unless an upstream cancellation has been
  requested.

A ``_cancellation_flags`` map stores per-run cancellation booleans.
``request_cancel`` flips the flag; the loop polls between todo dispatches and
between tool iterations so long-running providers cannot block cancellation
indefinitely.
"""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any, Callable, Dict, List, Optional, Sequence

from src.agent_debug.domain.context_manager import SessionContextManager
from src.agent_debug.domain.models import (
    AgentRun,
    DebugEvent,
    PlanTask,
    TodoItem,
    asdict_safe,
)
from src.agent_debug.domain.subagent_orchestrator import SubagentOrchestrator
from src.agent_debug.domain.summary_manager import SummaryManager
from src.agent_debug.domain.todo_engine import TodoEngine
from src.agent_debug.domain.tools import (
    TASK_TOOL_NAME,
    WRITE_TODOS_TOOL_NAME,
    ToolExecutionContext,
    ToolExecutionError,
    ToolNotFoundError,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.tools.workspace_tools import serialise_tool_result
from src.agent_debug.prompts.builtin_subagents import (
    DEFAULT_READONLY_TOOLS,
    DEFAULT_WRITE_TOOLS,
    get_subagent,
)
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.base import ModelRequestContext
from src.agent_debug.provider.service import (
    ProviderExecutionError,
    ProviderExecutionService,
    extract_text_output,
)


_DEFAULT_PARALLEL_LIMIT = max(1, int(os.getenv("AGENT_DEBUG_PARALLEL_LIMIT", "4") or 4))
# 0 means no fixed ReAct tool-loop cap. Cancellation, provider timeout and
# repeated-tool protection remain as safety valves.
_TOOL_LOOP_MAX_STEPS = max(0, int(os.getenv("AGENT_DEBUG_TOOL_LOOP_STEPS", "0") or 0))
_REPEATED_TOOL_LIMIT = max(1, int(os.getenv("AGENT_DEBUG_REPEATED_TOOL_LIMIT", "8") or 8))
_PLAN_TASK_TIMEOUT_MS = max(1, int(os.getenv("AGENT_DEBUG_PLAN_TASK_TIMEOUT_MS", "60000") or 60000))
# 子代理（Task）嵌套循环的单次超时；比单步对话更宽松以容纳多步工具使用。
_SUBAGENT_TIMEOUT_MS = max(1, int(os.getenv("AGENT_DEBUG_SUBAGENT_TIMEOUT_MS", "120000") or 120000))
_EMPTY_ASSISTANT_OUTPUT_CODE = "EMPTY_ASSISTANT_OUTPUT"
_EMPTY_ASSISTANT_OUTPUT_MESSAGE = "模型已结束，但未生成可展示文本，请重试或检查 Provider 输出协议。"
_TOOL_LOOP_EXHAUSTED_CODE = "TOOL_LOOP_EXHAUSTED"
_TOOL_LOOP_EXHAUSTED_MESSAGE = "已达到用户配置的工具循环上限；请提高 AGENT_DEBUG_TOOL_LOOP_STEPS、设为 0 取消固定上限，或缩小任务范围。"
_TOOL_LOOP_REPEATED_CODE = "TOOL_LOOP_REPEATED"
_TOOL_LOOP_REPEATED_MESSAGE = "模型连续重复请求相同工具调用，已停止以避免无限循环。"
# 真流式开关：默认开启；置 0/false 回退到非流式 chat（测试/排障）。
_STREAMING_ENABLED = os.getenv("AGENT_DEBUG_STREAM", "1").strip().lower() not in ("0", "false", "no", "off")
# 单会话对话记忆保留的最大轮次（一轮 = user + assistant）。超出时丢弃最早轮次，
# 循环内的 context_compactor 仍会对窗口预算做二次压缩。
_HISTORY_MAX_TURNS = max(1, int(os.getenv("AGENT_DEBUG_HISTORY_TURNS", "40") or 40))
# 持久化用户输入的事件类型；前端 buildAgentBlocksFromEvents 对未知类型忽略，无渲染副作用。
COMPOSER_USER_MESSAGE_EVENT = "composer.user.message"
_LOG_TEXT_LIMIT = 1000
_LOG_LIST_LIMIT = 12


class RunCancelledError(RuntimeError):
    """Raised when a run is cancelled mid-flight."""


class ToolLoopExhaustedError(RuntimeError):
    """Raised when the ReAct loop exhausts its tool-call budget."""


class ToolLoopRepeatedError(RuntimeError):
    """Raised when the model repeatedly asks for identical tool calls."""


class AgentRuntimeService:
    def __init__(
        self,
        todo_engine: TodoEngine,
        subagent_orchestrator: SubagentOrchestrator,
        summary_manager: SummaryManager,
        event_bus: EventBus,
        context_manager: SessionContextManager,
        provider_service: ProviderExecutionService,
        model_resolver: Callable[[str], str] | None = None,
        tool_registry: Optional[WorkspaceToolRegistry] = None,
        tool_allowlist_resolver: Callable[[str], Sequence[str]] | None = None,
        trace_collector: Any = None,
        permission_service: Any = None,
        context_compactor: Any = None,
    ) -> None:
        self.todo_engine = todo_engine
        self.subagent_orchestrator = subagent_orchestrator
        self.summary_manager = summary_manager
        self.event_bus = event_bus
        self.context_manager = context_manager
        self.provider_service = provider_service
        self.model_resolver = model_resolver or (
            lambda _session_id: os.getenv("OPENAI_MODEL", "mock-model")
        )
        self.tool_registry = tool_registry
        self.tool_allowlist_resolver = tool_allowlist_resolver
        self.trace_collector = trace_collector
        self.permission_service = permission_service
        self.context_compactor = context_compactor
        self.runs = InMemoryTable[AgentRun]()
        self._cancellation_flags: Dict[str, bool] = {}
        self._pause_events: Dict[str, asyncio.Event] = {}
        self._run_logs: Dict[str, List[Dict[str, Any]]] = {}
        # 每个会话同一时刻只允许一个活跃 run（参考 Proma activeSessions）。
        self._active_run_by_session: Dict[str, str] = {}
        # 单会话对话记忆：``session_id -> [{"role", "content"}, ...]``。
        # 让同一会话的多轮 composer 对话共享上下文（截图问题的根因修复）。
        self._conversation_history: Dict[str, List[Dict[str, Any]]] = {}

    def _allowed_tool_names(self, session_id: str) -> Optional[List[str]]:
        registry = self.tool_registry
        if registry is None:
            return None
        if self.tool_allowlist_resolver is None:
            return registry.names()
        return list(self.tool_allowlist_resolver(session_id))

    def active_run_for_session(self, session_id: str) -> Optional[str]:
        return self._active_run_by_session.get(session_id)

    def _session_history(self, session_id: str) -> List[Dict[str, Any]]:
        """返回该会话已沉淀的对话轮次（user/assistant 交替）。

        内存缺失时（如进程重启后首轮）从事件总线快照重建，复用既有的 JSONL
        持久化 + 启动 hydrate，从而实现「单会话上下文常驻」且重启可恢复。
        """
        hist = self._conversation_history.get(session_id)
        if hist is None:
            hist = self._rebuild_history_from_events(session_id)
            self._conversation_history[session_id] = hist
        return hist

    def _rebuild_history_from_events(self, session_id: str) -> List[Dict[str, Any]]:
        try:
            events = self.event_bus.snapshot(session_id)
        except Exception:
            return []
        ordered = sorted(events, key=lambda e: int(e.get("seq", 0) or 0))
        history: List[Dict[str, Any]] = []
        for event in ordered:
            etype = event.get("type")
            payload = event.get("payload") or {}
            if etype == COMPOSER_USER_MESSAGE_EVENT:
                text = str(payload.get("text") or "")
                if text:
                    history.append({"role": "user", "content": text})
            elif etype == "agent.completed":
                text = str(payload.get("text") or "")
                if text:
                    history.append({"role": "assistant", "content": text})
        return self._bounded_history(history)

    @staticmethod
    def _bounded_history(history: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """保留最近 ``_HISTORY_MAX_TURNS`` 轮（每轮约 2 条消息），防止无限增长。"""
        max_messages = _HISTORY_MAX_TURNS * 2
        if len(history) <= max_messages:
            return history
        return history[-max_messages:]

    def _remember_turn(self, session_id: str, user_message: str, assistant_text: str) -> None:
        """把一轮 user/assistant 追加进会话记忆（就地裁剪到上限）。"""
        hist = self._session_history(session_id)
        if user_message:
            hist.append({"role": "user", "content": user_message})
        if assistant_text:
            hist.append({"role": "assistant", "content": assistant_text})
        bounded = self._bounded_history(hist)
        if bounded is not hist:
            self._conversation_history[session_id] = bounded

    def invalidate_session_history(self, session_id: str) -> None:
        self._conversation_history.pop(session_id, None)

    def is_session_busy(self, session_id: str) -> bool:
        run_id = self._active_run_by_session.get(session_id)
        if not run_id:
            return False
        run = self.runs.get(run_id)
        return bool(run and run.status in ("starting", "running"))

    async def start(
        self,
        session_id: str,
        plan_id: str,
        objective: str,
        *,
        parallel_limit: Optional[int] = None,
    ) -> AgentRun:
        context = self.context_manager.ensure(session_id)
        # 串行化：若该会话已有活跃 run，先请求取消旧 run，保证「一会话一活跃 run」。
        previous_run_id = self._active_run_by_session.get(session_id)
        if previous_run_id:
            prev = self.runs.get(previous_run_id)
            if prev and prev.status in ("starting", "running", "paused"):
                self.request_cancel(previous_run_id)
        run = AgentRun(
            id=make_id("run"),
            session_id=session_id,
            plan_id=plan_id,
            trigger="plan_execute",
            status="running",
            current_context_ref=context.active_context_ref,
            active_node_ids=[],
            active_todo_ids=[],
            parallel_limit=parallel_limit or _DEFAULT_PARALLEL_LIMIT,
            created_at=utc_now_iso(),
            updated_at=utc_now_iso(),
        )
        self.runs.save(run.id, run)
        self._cancellation_flags[run.id] = False
        self._pause_events[run.id] = asyncio.Event()
        self._pause_events[run.id].set()
        self._run_logs[run.id] = []
        self._active_run_by_session[session_id] = run.id
        await self.publish(session_id, "agent.started", "agent", run.id, asdict_safe(run), run.id)
        await self.publish(
            session_id,
            "agent.token.stream.delta",
            "agent",
            run.id,
            {"runId": run.id, "delta": f"开始执行：{objective}"},
            run.id,
        )
        return run

    def get_run(self, run_id: str) -> Optional[AgentRun]:
        return self.runs.get(run_id)

    def update_run_status(self, run_id: str, status: str) -> Optional[AgentRun]:
        run = self.runs.get(run_id)
        if not run:
            return None
        run.status = status
        run.updated_at = utc_now_iso()
        self.runs.save(run.id, run)
        pause_event = self._pause_events.get(run_id)
        if status == "paused" and pause_event is not None:
            pause_event.clear()
        elif status in {"running", "cancelled", "failed", "completed"} and pause_event is not None:
            pause_event.set()
        if status == "cancelled":
            self._cancellation_flags[run_id] = True
        return run

    def request_cancel(self, run_id: str) -> bool:
        if self.runs.get(run_id) is None:
            return False
        self._cancellation_flags[run_id] = True
        pause_event = self._pause_events.get(run_id)
        if pause_event is not None:
            pause_event.set()
        return True

    def is_cancelled(self, run_id: str) -> bool:
        return bool(self._cancellation_flags.get(run_id))

    async def _wait_if_paused(self, run: AgentRun) -> None:
        while True:
            if self.is_cancelled(run.id):
                raise RunCancelledError(f"run {run.id} cancelled")
            pause_event = self._pause_events.get(run.id)
            if pause_event is None or pause_event.is_set():
                return
            await pause_event.wait()

    async def run_plan(
        self,
        run: AgentRun,
        tasks: Sequence[PlanTask],
        todos: Sequence[TodoItem],
    ) -> Dict[str, Any]:
        """Execute every todo derived from ``tasks`` honouring the dependency
        graph encoded in ``TodoItem.dependencies``."""
        if not tasks or not todos:
            return {"executedTodoIds": [], "failedTodoIds": []}

        task_by_id: Dict[str, PlanTask] = {t.id: t for t in tasks}
        todo_to_task: Dict[str, PlanTask] = {}
        for todo in todos:
            for plan_node_id in todo.related_plan_node_ids or []:
                task = task_by_id.get(plan_node_id)
                if task is not None:
                    todo_to_task[todo.id] = task
                    break

        executed: List[str] = []
        failed: List[str] = []
        run.parallel_limit = max(1, int(run.parallel_limit or _DEFAULT_PARALLEL_LIMIT))

        while True:
            await self._wait_if_paused(run)
            if self.is_cancelled(run.id):
                break
            ready = self.todo_engine.ready_todos(run.session_id)
            ready = [t for t in ready if t.id in todo_to_task]
            if not ready:
                break
            batch = ready[: run.parallel_limit]

            for todo in batch:
                await self._wait_if_paused(run)
                self.todo_engine.mark_status(todo.id, "running")
                run.active_todo_ids = list(set(run.active_todo_ids + [todo.id]))
                await self.publish(
                    run.session_id,
                    "todo.running",
                    "todo",
                    todo.id,
                    asdict_safe(self.todo_engine.get(todo.id)),
                    run.id,
                )

            results = await asyncio.gather(
                *[
                    self._execute_task(
                        run,
                        todo,
                        todo_to_task[todo.id],
                    )
                    for todo in batch
                ],
                return_exceptions=True,
            )

            for todo, outcome in zip(batch, results):
                run.active_todo_ids = [tid for tid in run.active_todo_ids if tid != todo.id]
                if isinstance(outcome, RunCancelledError):
                    self.todo_engine.mark_status(todo.id, "cancelled", error=str(outcome))
                    await self.publish(
                        run.session_id,
                        "todo.failed",
                        "todo",
                        todo.id,
                        {"id": todo.id, "status": "cancelled", "error": str(outcome)},
                        run.id,
                    )
                elif isinstance(outcome, Exception):
                    self.todo_engine.mark_status(
                        todo.id, "failed", error=str(outcome)
                    )
                    run.failed_todo_ids = list(set(run.failed_todo_ids + [todo.id]))
                    failed.append(todo.id)
                    await self.publish(
                        run.session_id,
                        "todo.failed",
                        "todo",
                        todo.id,
                        {"id": todo.id, "status": "failed", "error": str(outcome)},
                        run.id,
                    )
                else:
                    self.todo_engine.mark_status(todo.id, "completed")
                    run.completed_todo_ids = list(set(run.completed_todo_ids + [todo.id]))
                    executed.append(todo.id)
                    self.todo_engine.unblock_dependents(todo.id)
                    await self.publish(
                        run.session_id,
                        "todo.completed",
                        "todo",
                        todo.id,
                        asdict_safe(self.todo_engine.get(todo.id)),
                        run.id,
                    )

            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)

        if self.is_cancelled(run.id):
            run.status = "cancelled"
        elif failed and not executed:
            run.status = "failed"
        else:
            run.status = "completed"
        run.updated_at = utc_now_iso()
        self.runs.save(run.id, run)
        if self._active_run_by_session.get(run.session_id) == run.id:
            self._active_run_by_session.pop(run.session_id, None)
        self._pause_events.pop(run.id, None)
        terminal_event = {
            "completed": "agent.completed",
            "failed": "agent.failed",
            "cancelled": "agent.cancelled",
        }.get(run.status)
        if terminal_event:
            work_log_summary = self._finalize_work_log(run, run.status)
            payload = {"runId": run.id, "status": run.status, "workLogSummary": work_log_summary}
            if run.status == "completed":
                payload["text"] = "计划执行完成。"
            await self.publish(run.session_id, terminal_event, "agent", run.id, payload, run.id)
        return {
            "executedTodoIds": executed,
            "failedTodoIds": failed,
            "cancelled": self.is_cancelled(run.id),
        }

    async def _execute_task(
        self,
        run: AgentRun,
        todo: TodoItem,
        task: PlanTask,
    ) -> Dict[str, Any]:
        if self.is_cancelled(run.id):
            raise RunCancelledError(f"run {run.id} cancelled before task {task.id}")
        await self._wait_if_paused(run)

        self.context_manager.checkpoint(run.session_id, run.current_context_ref)
        await self.publish(
            run.session_id,
            "plan.node.started",
            "plan",
            task.id,
            {"nodeId": task.id, "status": "running"},
            run.id,
        )
        subagent = self.subagent_orchestrator.create(
            run.id, [task.id], [todo.id], task.title
        )
        subagent_payload = asdict_safe(subagent)
        subagent_payload.setdefault("description", task.title)
        subagent_payload.setdefault("prompt", task.description)
        await self.publish(
            run.session_id, "subagent.created", "subagent", subagent.id, subagent_payload, run.id
        )

        provider_ctx = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=self.model_resolver(run.session_id),
            timeout_ms=_PLAN_TASK_TIMEOUT_MS,
            session_id=run.session_id,
            run_id=run.id,
            metadata={"operation": "subagent_execution", "subagentId": subagent.id},
        )

        try:
            provider_output = await self._run_react_loop(
                run=run,
                subagent_id=subagent.id,
                provider_ctx=provider_ctx,
                seed_user_message=task.title,
            )
        except RunCancelledError:
            self.subagent_orchestrator.cancel(subagent.id)
            await self.publish(
                run.session_id,
                "plan.node.failed",
                "plan",
                task.id,
                {"nodeId": task.id, "status": "cancelled"},
                run.id,
            )
            raise
        except Exception as exc:
            self.subagent_orchestrator.fail(subagent.id, error=str(exc))
            await self.publish(
                run.session_id,
                "plan.node.failed",
                "plan",
                task.id,
                {"nodeId": task.id, "status": "failed", "error": str(exc)},
                run.id,
            )
            raise

        completed = self.subagent_orchestrator.complete(subagent.id)
        if completed is None:
            await self.publish(
                run.session_id,
                "plan.node.failed",
                "plan",
                task.id,
                {"nodeId": task.id, "status": "failed"},
                run.id,
            )
            raise RuntimeError("Failed to complete subagent")

        summary = await self.summary_manager.summarize_subagent(
            completed, task.title, run.session_id
        )
        rolled_context = self.context_manager.rollback_with_summary(
            run.session_id, completed.context_ref, summary.id
        )
        run.current_context_ref = rolled_context.active_context_ref
        run.updated_at = utc_now_iso()
        self.runs.save(run.id, run)
        await self.publish(
            run.session_id,
            "subagent.summary.generated",
            "subagent",
            subagent.id,
            asdict_safe(summary),
            run.id,
        )
        await self.publish(
            run.session_id,
            "subagent.context.rolled_back",
            "subagent",
            subagent.id,
            {
                "subagentRunId": subagent.id,
                "summaryId": summary.id,
                "rawContextRef": completed.context_ref,
                "summaryRef": summary.id,
                "activeContextRef": rolled_context.active_context_ref,
                "replacedContextRefs": summary.lineage.get("replacedContextRefs", []),
            },
            run.id,
        )
        await self.publish(
            run.session_id,
            "plan.node.completed",
            "plan",
            task.id,
            {"nodeId": task.id, "status": "completed"},
            run.id,
        )
        return {"subagent": completed, "summary": summary, "providerOutput": provider_output}

    async def _run_react_loop(
        self,
        *,
        run: AgentRun,
        subagent_id: str,
        provider_ctx: ModelRequestContext,
        seed_user_message: str,
        system_message: str | None = None,
        history: Optional[Sequence[Dict[str, Any]]] = None,
        allowed_tools_override: Optional[Sequence[str]] = None,
        tool_choice_override: str | None = None,
        event_extra: Optional[Dict[str, Any]] = None,
    ) -> Any:
        # ``allowed_tools_override`` restricts the tool set for sub-agent runs
        # (never includes ``Task`` → no recursive delegation). ``event_extra``
        # tags every published event so nested sub-agent events can be grouped
        # under their parent Task card on the frontend.
        messages: List[Dict[str, Any]] = []
        if system_message:
            messages.append({"role": "system", "content": system_message})
        # 注入本会话历史轮次（user/assistant 交替），让同一会话多轮对话共享上下文。
        for entry in history or []:
            role = entry.get("role")
            content = entry.get("content")
            if role in ("user", "assistant") and content:
                messages.append({"role": role, "content": content})
        messages.append({"role": "user", "content": seed_user_message})
        request: Dict[str, Any] = {"messages": messages}
        if self.tool_registry is not None:
            if allowed_tools_override is not None:
                allowed_names: Optional[List[str]] = list(allowed_tools_override)
            else:
                allowed_names = self._allowed_tool_names(run.session_id)
            tool_schemas = self.tool_registry.json_schemas(allowed_names)
            if tool_schemas:
                request["tools"] = tool_schemas
                if tool_choice_override and (
                    allowed_names is None or tool_choice_override in set(allowed_names)
                ):
                    request["tool_choice"] = {
                        "type": "function",
                        "function": {"name": tool_choice_override},
                    }
                else:
                    request["tool_choice"] = "auto"

        last_output: Any = None
        repeated_tool_signature: tuple[str, ...] | None = None
        repeated_tool_count = 0
        step = 0
        while True:
            await self._wait_if_paused(run)
            if _TOOL_LOOP_MAX_STEPS > 0 and step >= _TOOL_LOOP_MAX_STEPS:
                self._append_log(
                    run.id,
                    {
                        "role": "error",
                        "code": _TOOL_LOOP_EXHAUSTED_CODE,
                        "text": _TOOL_LOOP_EXHAUSTED_MESSAGE,
                        "step": step,
                    },
                )
                raise ToolLoopExhaustedError(_TOOL_LOOP_EXHAUSTED_MESSAGE)

            if self.is_cancelled(run.id):
                raise RunCancelledError(f"run {run.id} cancelled mid-loop")

            # 上下文压缩：临近窗口预算时把早期消息压成摘要。
            if self.context_compactor is not None:
                try:
                    new_messages, compacted = await self.context_compactor.compact(
                        messages, run.session_id
                    )
                except Exception:
                    new_messages, compacted = messages, False
                if compacted:
                    messages[:] = new_messages
                    request["messages"] = messages
                    await self.publish(
                        run.session_id,
                        "agent.context.compacted",
                        "agent",
                        run.id,
                        {"runId": run.id, "step": step + 1, "messageCount": len(messages)},
                        run.id,
                        extra=event_extra,
                    )

            response = await self._provider_step(
                run=run,
                provider_ctx=provider_ctx,
                request=request,
                step=step,
                event_extra=event_extra,
            )
            last_output = response.output

            # 推理 / 思考内容入日志（中国大模型 reasoning_content / thinking）。
            if getattr(response, "reasoning", None):
                self._append_log(
                    run.id,
                    {"role": "reasoning", "text": response.reasoning, "step": step + 1},
                )

            tool_calls = list(response.tool_calls or [])
            if not tool_calls:
                try:
                    text = extract_text_output(response.output)
                except Exception:
                    text = ""
                if text:
                    messages.append({"role": "assistant", "content": text})
                    self._append_log(
                        run.id,
                        {"role": "assistant", "text": text, "step": step + 1},
                    )
                return response.output

            current_signature = tuple(_tool_call_signature(call) for call in tool_calls)
            if current_signature == repeated_tool_signature:
                repeated_tool_count += 1
            else:
                repeated_tool_signature = current_signature
                repeated_tool_count = 1
            if repeated_tool_count > _REPEATED_TOOL_LIMIT:
                self._append_log(
                    run.id,
                    {
                        "role": "error",
                        "code": _TOOL_LOOP_REPEATED_CODE,
                        "text": _TOOL_LOOP_REPEATED_MESSAGE,
                        "step": step + 1,
                        "signature": list(current_signature),
                    },
                )
                raise ToolLoopRepeatedError(_TOOL_LOOP_REPEATED_MESSAGE)

            try:
                assistant_text = extract_text_output(response.output)
            except Exception:
                assistant_text = ""
            messages.append(
                {
                    "role": "assistant",
                    "content": assistant_text,
                    # 透传推理内容，供需要「thinking 回传」的供应商（如 DeepSeek）使用。
                    "reasoning": getattr(response, "reasoning", None),
                    "tool_calls": [
                        {
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": _safe_json_dumps(call.arguments),
                            },
                        }
                        for call in tool_calls
                    ],
                }
            )

            # 工具执行：普通工具串行（参考 Proma toolUseConcurrency:1，规避并行
            # tool_use 异常）；同一轮内的多个 Task（子代理委派）调用并行执行
            # （asyncio.gather），以支持「一次派发多个子代理并排运行」。
            results_by_call_id: Dict[str, str] = {}
            # Only treat a Task call as a delegation when Task is in the effective
            # allowlist. Sub-agents (allowed_tools_override without Task) thus
            # cannot spawn nested sub-agents — a hallucinated Task falls through
            # to _dispatch_tool and is rejected as TOOL_NOT_AVAILABLE.
            task_allowed = allowed_tools_override is None or TASK_TOOL_NAME in allowed_tools_override
            task_calls = [c for c in tool_calls if c.name == TASK_TOOL_NAME and task_allowed]
            other_calls = [c for c in tool_calls if not (c.name == TASK_TOOL_NAME and task_allowed)]

            for call in other_calls:
                results_by_call_id[call.id] = await self._dispatch_tool(
                    run=run,
                    subagent_id=subagent_id,
                    tool_name=call.name,
                    arguments=call.arguments,
                    tool_call_id=call.id,
                    allowed_tools_override=allowed_tools_override,
                    event_extra=event_extra,
                )

            if task_calls:
                # 子代理不允许再次委派（allowed_tools_override 永不含 Task），
                # 这里的并行只发生在主循环层。
                gathered = await asyncio.gather(
                    *[
                        self._handle_task(
                            run=run,
                            parent_subagent_id=subagent_id,
                            task_call_id=call.id,
                            arguments=call.arguments,
                        )
                        for call in task_calls
                    ],
                    return_exceptions=True,
                )
                for call, outcome in zip(task_calls, gathered):
                    if isinstance(outcome, BaseException):
                        results_by_call_id[call.id] = _safe_json_dumps(
                            {"error": "SUBAGENT_FAILED", "message": str(outcome)}
                        )
                    else:
                        results_by_call_id[call.id] = outcome

            # 按原始 tool_calls 顺序回填 tool 消息，保持与 assistant.tool_calls 对齐。
            for call in tool_calls:
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": call.id,
                        "name": call.name,
                        "content": results_by_call_id.get(call.id, ""),
                    }
                )
            step += 1

    async def _provider_step(
        self,
        *,
        run: AgentRun,
        provider_ctx: ModelRequestContext,
        request: Dict[str, Any],
        step: int,
        event_extra: Optional[Dict[str, Any]] = None,
    ) -> Any:
        """单步模型调用：默认走真流式（产出 token / reasoning 增量事件）。"""

        async def _on_delta(event: Dict[str, Any]) -> None:
            etype = event.get("type")
            if etype == "text":
                await self.publish(
                    run.session_id,
                    "agent.token.stream.delta",
                    "agent",
                    run.id,
                    {"runId": run.id, "delta": event.get("text", ""), "step": step + 1},
                    run.id,
                    extra=event_extra,
                )
            elif etype == "reasoning":
                await self.publish(
                    run.session_id,
                    "agent.reasoning.delta",
                    "agent",
                    run.id,
                    {"runId": run.id, "delta": event.get("text", ""), "step": step + 1},
                    run.id,
                    extra=event_extra,
                )

        if _STREAMING_ENABLED:
            result = await self.provider_service.stream_chat_operation(
                request=request,
                ctx=provider_ctx,
                session_id=run.session_id,
                operation="subagent_execution",
                parser=lambda response: response,
                correlation_id=run.id,
                on_delta=_on_delta,
            )
        else:
            result = await self.provider_service.execute_chat_operation(
                request=request,
                ctx=provider_ctx,
                session_id=run.session_id,
                operation="subagent_execution",
                parser=lambda response: response,
                correlation_id=run.id,
            )
        return result.value

    async def _dispatch_tool(
        self,
        *,
        run: AgentRun,
        subagent_id: str,
        tool_name: str,
        arguments: Dict[str, Any],
        tool_call_id: str | None = None,
        allowed_tools_override: Optional[Sequence[str]] = None,
        event_extra: Optional[Dict[str, Any]] = None,
    ) -> str:
        registry = self.tool_registry
        ctx = ToolExecutionContext(
            session_id=run.session_id,
            run_id=run.id,
            subagent_id=subagent_id,
        )
        await self.publish(
            run.session_id,
            "agent.tool.invoked",
            "tool",
            tool_name,
            {
                "name": tool_name,
                "arguments": arguments,
                "runId": run.id,
                "toolCallId": tool_call_id,
            },
            run.id,
            extra=event_extra,
        )

        # 权限闸门：plan/auto 模式可拒绝危险工具（默认 bypass 放行）。
        if self.permission_service is not None:
            decision = self.permission_service.can_use_tool(tool_name, arguments, session_id=run.session_id)
            if not decision.allowed:
                await self.publish(
                    run.session_id,
                    "agent.tool.denied",
                    "tool",
                    tool_name,
                    {
                        "name": tool_name,
                        "reason": decision.reason,
                        "runId": run.id,
                        "toolCallId": tool_call_id,
                    },
                    run.id,
                    extra=event_extra,
                )
                return json.dumps({"error": "TOOL_DENIED", "message": decision.reason})

        # ``write_todos`` is model-driven todo authoring. We intercept it here
        # (rather than running the registry tool) because the actual write needs
        # the runtime-owned TodoEngine + event bus to publish ``todo.*`` events.
        if tool_name == WRITE_TODOS_TOOL_NAME:
            return await self._handle_write_todos(
                run=run,
                arguments=arguments,
                tool_call_id=tool_call_id,
            )

        span = None
        if self.trace_collector is not None:
            span = self.trace_collector.start_span(
                run.id, f"tool.{tool_name}", {"runId": run.id, "subagentId": subagent_id}
            )

        try:
            if registry is None:
                err = "tool registry not configured"
                await self.publish(
                    run.session_id,
                    "agent.tool.failed",
                    "tool",
                    tool_name,
                    {
                        "name": tool_name,
                        "code": "TOOL_NOT_AVAILABLE",
                        "message": err,
                        "runId": run.id,
                        "toolCallId": tool_call_id,
                    },
                    run.id,
                    extra=event_extra,
                )
                return json.dumps({"error": "TOOL_NOT_AVAILABLE", "message": err})
            if allowed_tools_override is not None:
                allowed_tool_names: Optional[List[str]] = list(allowed_tools_override)
            else:
                allowed_tool_names = self._allowed_tool_names(run.session_id)
            if allowed_tool_names is not None and tool_name not in allowed_tool_names:
                err = f"tool disabled for session: {tool_name}"
                await self.publish(
                    run.session_id,
                    "agent.tool.failed",
                    "tool",
                    tool_name,
                    {
                        "name": tool_name,
                        "code": "TOOL_NOT_AVAILABLE",
                        "message": err,
                        "runId": run.id,
                        "toolCallId": tool_call_id,
                    },
                    run.id,
                    extra=event_extra,
                )
                return json.dumps({"error": "TOOL_NOT_AVAILABLE", "message": err})
            try:
                result = await registry.run(tool_name, arguments, ctx)
            except ToolNotFoundError as exc:
                await self.publish(
                    run.session_id,
                    "agent.tool.failed",
                    "tool",
                    tool_name,
                    {
                        "name": tool_name,
                        "code": "TOOL_NOT_FOUND",
                        "message": str(exc),
                        "runId": run.id,
                        "toolCallId": tool_call_id,
                    },
                    run.id,
                    extra=event_extra,
                )
                return json.dumps({"error": "TOOL_NOT_FOUND", "message": str(exc)})
            except ToolExecutionError as exc:
                await self.publish(
                    run.session_id,
                    "agent.tool.failed",
                    "tool",
                    tool_name,
                    {
                        "name": tool_name,
                        "code": exc.code,
                        "message": str(exc),
                        "runId": run.id,
                        "toolCallId": tool_call_id,
                    },
                    run.id,
                    extra=event_extra,
                )
                return json.dumps({"error": exc.code, "message": str(exc)})
            except Exception as exc:  # pragma: no cover - defensive
                await self.publish(
                    run.session_id,
                    "agent.tool.failed",
                    "tool",
                    tool_name,
                    {
                        "name": tool_name,
                        "code": "TOOL_FAILED",
                        "message": str(exc),
                        "runId": run.id,
                        "toolCallId": tool_call_id,
                    },
                    run.id,
                    extra=event_extra,
                )
                return json.dumps({"error": "TOOL_FAILED", "message": str(exc)})
            output_text = result.text or ""
            output_data = _compact_log_value(result.output)
            await self.publish(
                run.session_id,
                "agent.tool.completed",
                "tool",
                tool_name,
                {
                    "name": tool_name,
                    "outputPreview": output_text[:400],
                    "output": output_text[:8000],
                    "outputData": output_data,
                    "runId": run.id,
                    "toolCallId": tool_call_id,
                },
                run.id,
                extra=event_extra,
            )
            return serialise_tool_result(result)
        finally:
            if span is not None and self.trace_collector is not None:
                self.trace_collector.finish_span(span)

    def _resolve_subagent_spec(
        self, arguments: Dict[str, Any]
    ) -> tuple[str, str, List[str]]:
        """Resolve (subagentType, systemPrompt, allowedTools) for a Task call.

        Priority: explicit ``system_prompt`` (ad-hoc sub-agent) > built-in
        ``subagent_type`` profile > a read-only fallback. The allowlist NEVER
        contains ``Task`` so sub-agents cannot delegate recursively.
        """
        custom_prompt = arguments.get("system_prompt") if isinstance(arguments, dict) else None
        subagent_type = (arguments.get("subagent_type") or "").strip() if isinstance(arguments, dict) else ""
        readonly = arguments.get("readonly") if isinstance(arguments, dict) else None

        if custom_prompt and str(custom_prompt).strip():
            label = subagent_type or "custom"
            is_readonly = True if readonly is None else bool(readonly)
            allowed = list(DEFAULT_READONLY_TOOLS if is_readonly else DEFAULT_WRITE_TOOLS)
            return label, str(custom_prompt).strip(), allowed

        profile = get_subagent(subagent_type) if subagent_type else None
        if profile is not None:
            return profile.name, profile.system_prompt, list(profile.allowed_tools)

        # Fallback: a read-only general sub-agent.
        fallback_prompt = (
            "你是通用只读子代理。围绕给定任务检索与阅读资料，产出结构化结论与关键路径，不修改工作区。"
        )
        return (subagent_type or "general"), fallback_prompt, list(DEFAULT_READONLY_TOOLS)

    async def _handle_task(
        self,
        *,
        run: AgentRun,
        parent_subagent_id: str,
        task_call_id: str | None,
        arguments: Dict[str, Any],
    ) -> str:
        """Spawn a nested sub-agent ReAct loop for a model-driven ``Task`` call.

        Runs inside the *parent* :class:`AgentRun` (no new run → no single-active
        -run conflict). All nested events are tagged with ``parentToolCallId`` so
        the frontend nests them under the Task card. Multiple Task calls in one
        turn are executed concurrently by the caller via ``asyncio.gather``.
        """
        args = arguments if isinstance(arguments, dict) else {}
        description = str(args.get("description") or "子代理任务").strip()
        prompt = str(args.get("prompt") or "").strip()

        # Task 的工具卡（父级时间线，不带 parentToolCallId）。
        await self.publish(
            run.session_id,
            "agent.tool.invoked",
            "tool",
            TASK_TOOL_NAME,
            {
                "name": TASK_TOOL_NAME,
                "arguments": args,
                "runId": run.id,
                "toolCallId": task_call_id,
            },
            run.id,
        )

        if not prompt:
            err = "argument 'prompt' is required for Task"
            await self.publish(
                run.session_id,
                "agent.tool.failed",
                "tool",
                TASK_TOOL_NAME,
                {
                    "name": TASK_TOOL_NAME,
                    "code": "TOOL_INVALID_ARGS",
                    "message": err,
                    "runId": run.id,
                    "toolCallId": task_call_id,
                },
                run.id,
            )
            return json.dumps({"error": "TOOL_INVALID_ARGS", "message": err})

        subagent_type, system_prompt, allowed_tools = self._resolve_subagent_spec(args)
        # 防御：无论来源如何，子代理 allowlist 永不含 Task。
        allowed_tools = [t for t in allowed_tools if t != TASK_TOOL_NAME]

        subagent = self.subagent_orchestrator.create(
            run.id, [], [], description or prompt[:80]
        )
        event_extra = {
            "parentToolCallId": task_call_id,
            "subagentId": subagent.id,
            "subagentType": subagent_type,
        }
        await self.publish(
            run.session_id,
            "subagent.created",
            "subagent",
            subagent.id,
            {
                "subagentId": subagent.id,
                "parentRunId": run.id,
                "parentToolCallId": task_call_id,
                "subagentType": subagent_type,
                "description": description,
                "prompt": prompt,
                "allowedTools": allowed_tools,
                "runId": run.id,
            },
            run.id,
        )

        provider_ctx = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=self.model_resolver(run.session_id),
            timeout_ms=_SUBAGENT_TIMEOUT_MS,
            session_id=run.session_id,
            run_id=run.id,
            metadata={"operation": "subagent_task", "subagentId": subagent.id},
        )

        try:
            provider_output = await self._run_react_loop(
                run=run,
                subagent_id=subagent.id,
                provider_ctx=provider_ctx,
                seed_user_message=prompt,
                system_message=system_prompt,
                history=[],
                allowed_tools_override=allowed_tools,
                event_extra=event_extra,
            )
            try:
                summary = extract_text_output(provider_output) or ""
            except Exception:
                summary = ""
            summary = summary.strip() or "（子代理未产出文本结果）"

            self.subagent_orchestrator.complete(subagent.id)
            await self.publish(
                run.session_id,
                "subagent.completed",
                "subagent",
                subagent.id,
                {
                    "subagentId": subagent.id,
                    "parentRunId": run.id,
                    "parentToolCallId": task_call_id,
                    "subagentType": subagent_type,
                    "summary": summary,
                    "runId": run.id,
                },
                run.id,
            )
            await self.publish(
                run.session_id,
                "agent.tool.completed",
                "tool",
                TASK_TOOL_NAME,
                {
                    "name": TASK_TOOL_NAME,
                    "outputPreview": summary[:400],
                    "output": summary[:8000],
                    "runId": run.id,
                    "toolCallId": task_call_id,
                    "subagentId": subagent.id,
                    "subagentType": subagent_type,
                },
                run.id,
            )
            return summary
        except Exception as exc:
            self.subagent_orchestrator.fail(subagent.id, error=str(exc))
            await self.publish(
                run.session_id,
                "subagent.failed",
                "subagent",
                subagent.id,
                {
                    "subagentId": subagent.id,
                    "parentRunId": run.id,
                    "parentToolCallId": task_call_id,
                    "subagentType": subagent_type,
                    "message": str(exc),
                    "runId": run.id,
                },
                run.id,
            )
            await self.publish(
                run.session_id,
                "agent.tool.failed",
                "tool",
                TASK_TOOL_NAME,
                {
                    "name": TASK_TOOL_NAME,
                    "code": "SUBAGENT_FAILED",
                    "message": str(exc),
                    "runId": run.id,
                    "toolCallId": task_call_id,
                    "subagentId": subagent.id,
                },
                run.id,
            )
            return json.dumps({"error": "SUBAGENT_FAILED", "message": str(exc)})

    async def _handle_write_todos(
        self,
        *,
        run: AgentRun,
        arguments: Dict[str, Any],
        tool_call_id: str | None = None,
    ) -> str:
        """Apply a model-authored ``write_todos`` call and emit ``todo.*`` events."""
        raw_items = arguments.get("todos") if isinstance(arguments, dict) else None
        if not isinstance(raw_items, list):
            err = "argument 'todos' must be an array"
            await self.publish(
                run.session_id,
                "agent.tool.failed",
                "tool",
                WRITE_TODOS_TOOL_NAME,
                {
                    "name": WRITE_TODOS_TOOL_NAME,
                    "code": "TOOL_INVALID_ARGS",
                    "message": err,
                    "runId": run.id,
                    "toolCallId": tool_call_id,
                },
                run.id,
            )
            return json.dumps({"error": "TOOL_INVALID_ARGS", "message": err})

        changes = self.todo_engine.sync_agent_todos(
            run.session_id, raw_items, run_id=run.id
        )

        kind_to_event = {
            "created": "todo.created",
            "updated": "todo.updated",
            "running": "todo.running",
            "completed": "todo.completed",
        }
        completed = 0
        running = 0
        for todo, kind in changes:
            if todo.status == "completed":
                completed += 1
            elif todo.status == "running":
                running += 1
            await self.publish(
                run.session_id,
                kind_to_event.get(kind, "todo.updated"),
                "todo",
                todo.id,
                asdict_safe(todo),
                run.id,
            )

        total = len(changes)
        summary = (
            f"已更新 {total} 条 todo：{completed} 完成 / {running} 进行中"
            if total
            else "未写入任何 todo（清单为空）"
        )
        active_titles = [
            todo.title
            for todo, _kind in changes
            if todo.status == "running" and todo.title
        ]
        if active_titles:
            summary = f"{summary}。继续执行当前任务：{active_titles[0]}"
        await self.publish(
            run.session_id,
            "agent.tool.completed",
            "tool",
            WRITE_TODOS_TOOL_NAME,
            {
                "name": WRITE_TODOS_TOOL_NAME,
                "outputPreview": summary,
                "output": summary,
                "runId": run.id,
                "toolCallId": tool_call_id,
            },
            run.id,
        )
        return summary

    async def run_serial_subagent(
        self, run: AgentRun, todo_id: str, plan_node_id: str, objective: str
    ) -> Dict[str, Any]:
        """Backwards-compatible single-task driver retained for existing tests."""
        task = PlanTask(
            id=plan_node_id,
            stage_id="legacy",
            title=objective,
            description=objective,
        )
        todo = self.todo_engine.get(todo_id)
        if todo is None:
            todo = TodoItem(
                id=todo_id,
                session_id=run.session_id,
                title=objective,
                description=objective,
                source="plan",
                owner={"type": "main-agent", "id": "main"},
                priority=50,
                related_plan_node_ids=[task.id],
                created_at=utc_now_iso(),
                updated_at=utc_now_iso(),
            )
            self.todo_engine.todos.save(todo.id, todo)
        return await self._execute_task(run, todo, task)

    async def run_composer_chat(
        self,
        *,
        session_id: str,
        user_message: str,
        system_message: str | None = None,
        timeout_ms: int = 60_000,
        allowed_tools_override: Optional[Sequence[str]] = None,
        tool_choice_override: str | None = None,
    ) -> Dict[str, Any]:
        """日常对话（build/debug/ask）的带工具流式 ReAct 驱动。

        与 ``run_plan`` 不同，这里不走 DAG/subagent 摘要，而是单轮会话级
        ReAct：直接发 ``agent.started``，复用 ``_run_react_loop``（产出
        ``agent.token.stream.delta`` / ``agent.reasoning.delta`` /
        ``agent.tool.*`` 事件），结束发 ``agent.completed`` / ``agent.failed``。
        返回 ``{"message": {"text": ...}, "run": {"id": ...}}`` 供 REST 层透传。
        """
        context = self.context_manager.ensure(session_id)
        previous_run_id = self._active_run_by_session.get(session_id)
        if previous_run_id:
            prev = self.runs.get(previous_run_id)
            if prev and prev.status in ("starting", "running", "paused"):
                self.request_cancel(previous_run_id)

        run = AgentRun(
            id=make_id("run"),
            session_id=session_id,
            plan_id="",
            trigger="composer_chat",
            status="running",
            current_context_ref=context.active_context_ref,
            active_node_ids=[],
            active_todo_ids=[],
            parallel_limit=1,
            created_at=utc_now_iso(),
            updated_at=utc_now_iso(),
        )
        self.runs.save(run.id, run)
        self._cancellation_flags[run.id] = False
        self._pause_events[run.id] = asyncio.Event()
        self._pause_events[run.id].set()
        self._run_logs[run.id] = []
        self._active_run_by_session[session_id] = run.id
        await self.publish(session_id, "agent.started", "agent", run.id, asdict_safe(run), run.id)

        # 先快照本会话历史（不含当前这轮 user）作为对话记忆喂回模型；必须在持久化
        # 当前 user 事件之前，否则冷启动（内存为空、从事件重建）会把当前消息算进历史
        # 并与 _run_react_loop 注入的当前 user 重复。
        history = list(self._session_history(session_id))
        # 持久化用户输入（落入 JSONL 事件流，重启后可还原历史轮次）。
        await self.publish(
            session_id,
            COMPOSER_USER_MESSAGE_EVENT,
            "agent",
            run.id,
            {"runId": run.id, "text": user_message},
            run.id,
        )

        subagent_id = make_id("subagent")
        provider_ctx = ModelRequestContext(
            request_id=make_id("req"),
            trace_id=make_id("trace"),
            model=self.model_resolver(session_id),
            timeout_ms=timeout_ms,
            session_id=session_id,
            run_id=run.id,
            metadata={"operation": "composer_chat", "subagentId": subagent_id},
        )

        try:
            provider_output = await self._run_react_loop(
                run=run,
                subagent_id=subagent_id,
                provider_ctx=provider_ctx,
                seed_user_message=user_message,
                system_message=system_message,
                history=history,
                allowed_tools_override=allowed_tools_override,
                tool_choice_override=tool_choice_override,
            )
            try:
                text = extract_text_output(provider_output)
            except Exception:
                text = ""
            if not text.strip():
                run.status = "failed"
                run.updated_at = utc_now_iso()
                self.runs.save(run.id, run)
                await self.publish(
                    session_id,
                    "agent.failed",
                    "agent",
                    run.id,
                    {
                        "runId": run.id,
                        "status": "failed",
                        "errorCode": _EMPTY_ASSISTANT_OUTPUT_CODE,
                        "error": _EMPTY_ASSISTANT_OUTPUT_MESSAGE,
                        "workLogSummary": self._finalize_work_log(
                            run,
                            "failed",
                            error_code=_EMPTY_ASSISTANT_OUTPUT_CODE,
                            error=_EMPTY_ASSISTANT_OUTPUT_MESSAGE,
                        ),
                    },
                    run.id,
                )
                return {
                    "message": {
                        "text": f"（无法完成对话：{_EMPTY_ASSISTANT_OUTPUT_MESSAGE}）"
                    },
                    "run": {"id": run.id},
                }
            run.status = "completed"
            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)
            auto_completed_todos = self.todo_engine.complete_agent_todos_for_run(run.id)
            if auto_completed_todos:
                run.active_todo_ids = [
                    tid for tid in run.active_todo_ids if tid not in {todo.id for todo in auto_completed_todos}
                ]
                run.completed_todo_ids = list(
                    set(run.completed_todo_ids + [todo.id for todo in auto_completed_todos])
                )
                run.updated_at = utc_now_iso()
                self.runs.save(run.id, run)
                for todo in auto_completed_todos:
                    await self.publish(
                        session_id,
                        "todo.completed",
                        "todo",
                        todo.id,
                        asdict_safe(todo),
                        run.id,
                    )
            group_id = self.todo_engine.current_agent_group_id(run.session_id)
            if group_id and not self.todo_engine.has_open_agent_todos(run.session_id, group_id):
                self.todo_engine.archive_agent_group(run.session_id, group_id)
            # 把本轮 user/assistant 追加进会话记忆，供后续轮次复用。
            self._remember_turn(session_id, user_message, text)
            await self.publish(
                session_id,
                "agent.completed",
                "agent",
                run.id,
                {
                    "runId": run.id,
                    "status": "completed",
                    "text": text,
                    "workLogSummary": self._finalize_work_log(run, "completed"),
                },
                run.id,
            )
            return {"message": {"text": text}, "run": {"id": run.id}}
        except RunCancelledError:
            run.status = "cancelled"
            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)
            await self.publish(
                session_id,
                "agent.cancelled",
                "agent",
                run.id,
                {
                    "runId": run.id,
                    "status": "cancelled",
                    "workLogSummary": self._finalize_work_log(
                        run,
                        "cancelled",
                        error_code="RUN_CANCELLED",
                        error="Agent 已中止",
                    ),
                },
                run.id,
            )
            return {"message": {"text": "Agent 已中止，已保留中止前的事件输出。"}, "run": {"id": run.id}}
        except ToolLoopExhaustedError as exc:
            run.status = "failed"
            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)
            await self.publish(
                session_id,
                "agent.failed",
                "agent",
                run.id,
                {
                    "runId": run.id,
                    "status": "failed",
                    "errorCode": _TOOL_LOOP_EXHAUSTED_CODE,
                    "error": str(exc),
                    "workLogSummary": self._finalize_work_log(
                        run,
                        "failed",
                        error_code=_TOOL_LOOP_EXHAUSTED_CODE,
                        error=str(exc),
                    ),
                },
                run.id,
            )
            return {"message": {"text": f"（无法完成对话：{exc}）"}, "run": {"id": run.id}}
        except ToolLoopRepeatedError as exc:
            run.status = "failed"
            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)
            await self.publish(
                session_id,
                "agent.failed",
                "agent",
                run.id,
                {
                    "runId": run.id,
                    "status": "failed",
                    "errorCode": _TOOL_LOOP_REPEATED_CODE,
                    "error": str(exc),
                    "workLogSummary": self._finalize_work_log(
                        run,
                        "failed",
                        error_code=_TOOL_LOOP_REPEATED_CODE,
                        error=str(exc),
                    ),
                },
                run.id,
            )
            return {"message": {"text": f"（无法完成对话：{exc}）"}, "run": {"id": run.id}}
        except ProviderExecutionError as exc:
            run.status = "failed"
            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)
            await self.publish(
                session_id,
                "agent.failed",
                "agent",
                run.id,
                {
                    "runId": run.id,
                    "status": "failed",
                    "errorCode": exc.error_code,
                    "error": str(exc),
                    "workLogSummary": self._finalize_work_log(
                        run,
                        "failed",
                        error_code=exc.error_code,
                        error=str(exc),
                    ),
                },
                run.id,
            )
            text = f"（无法完成对话：{exc}）\n请检查 Agent Debug 的 Provider 配置与网络。"
            return {"message": {"text": text}, "run": {"id": run.id}}
        except Exception as exc:  # pragma: no cover - defensive
            run.status = "failed"
            run.updated_at = utc_now_iso()
            self.runs.save(run.id, run)
            await self.publish(
                session_id,
                "agent.failed",
                "agent",
                run.id,
                {
                    "runId": run.id,
                    "status": "failed",
                    "error": str(exc),
                    "workLogSummary": self._finalize_work_log(
                        run,
                        "failed",
                        error_code="UNEXPECTED_ERROR",
                        error=str(exc),
                    ),
                },
                run.id,
            )
            return {"message": {"text": f"（无法完成对话：{exc}）"}, "run": {"id": run.id}}
        finally:
            if self._active_run_by_session.get(session_id) == run.id:
                self._active_run_by_session.pop(session_id, None)
            self._pause_events.pop(run.id, None)

    def _append_log(self, run_id: str, entry: Dict[str, Any]) -> None:
        bucket = self._run_logs.setdefault(run_id, [])
        bucket.append({"ts": utc_now_iso(), **entry})
        if len(bucket) > 1000:
            del bucket[: len(bucket) - 1000]

    def get_run_logs(self, run_id: str) -> List[Dict[str, Any]]:
        return list(self._run_logs.get(run_id, []))

    def get_run_log_summary(self, run_id: str) -> Dict[str, Any]:
        run = self.runs.get(run_id)
        if run is None:
            return {}
        return self._build_final_work_log(run, run.status)

    def _finalize_work_log(
        self,
        run: AgentRun,
        status: str,
        *,
        error_code: str | None = None,
        error: str | None = None,
    ) -> Dict[str, Any]:
        summary = self._build_final_work_log(
            run,
            status,
            error_code=error_code,
            error=error,
        )
        self._append_log(run.id, {"role": "work_log_summary", "summary": summary})
        return summary

    def _build_final_work_log(
        self,
        run: AgentRun,
        status: str,
        *,
        error_code: str | None = None,
        error: str | None = None,
    ) -> Dict[str, Any]:
        events = self._events_for_run(run)
        issues: List[Dict[str, Any]] = []
        tool_failures: List[Dict[str, Any]] = []
        subagent_failures: List[Dict[str, Any]] = []
        provider_failures: List[Dict[str, Any]] = []
        commands: Dict[str, Dict[str, Any]] = {}
        subagents: Dict[str, Dict[str, Any]] = {}

        if error_code or error:
            issues.append(
                {
                    "type": "run",
                    "code": error_code or "RUN_ERROR",
                    "message": _truncate_log_text(error or ""),
                }
            )

        for event in events:
            event_type = str(event.get("type") or "")
            payload = event.get("payload") if isinstance(event.get("payload"), dict) else {}
            if event_type in {"agent.tool.failed", "agent.tool.denied"}:
                item = {
                    "name": payload.get("name"),
                    "toolCallId": payload.get("toolCallId"),
                    "code": payload.get("code") or ("TOOL_DENIED" if event_type.endswith("denied") else "TOOL_FAILED"),
                    "message": _truncate_log_text(payload.get("message") or payload.get("reason") or ""),
                    "subagentId": payload.get("subagentId"),
                    "parentToolCallId": payload.get("parentToolCallId"),
                }
                tool_failures.append(item)
                issues.append({"type": "tool", **item})
            elif event_type == "provider.request.failed":
                item = {
                    "provider": payload.get("provider"),
                    "model": payload.get("model"),
                    "operation": payload.get("operation"),
                    "attempt": payload.get("attempt"),
                    "code": payload.get("errorCode"),
                    "message": _truncate_log_text(payload.get("error") or ""),
                }
                provider_failures.append(item)
                issues.append({"type": "provider", **item})
            elif event_type == "subagent.created":
                sid = str(payload.get("subagentId") or "")
                if sid:
                    subagents[sid] = {
                        "subagentId": sid,
                        "status": "running",
                        "description": payload.get("description"),
                        "subagentType": payload.get("subagentType"),
                        "parentToolCallId": payload.get("parentToolCallId"),
                    }
            elif event_type == "subagent.completed":
                sid = str(payload.get("subagentId") or "")
                if sid:
                    item = subagents.setdefault(sid, {"subagentId": sid})
                    item.update(
                        {
                            "status": "completed",
                            "subagentType": payload.get("subagentType") or item.get("subagentType"),
                            "parentToolCallId": payload.get("parentToolCallId") or item.get("parentToolCallId"),
                            "summary": _truncate_log_text(payload.get("summary") or ""),
                        }
                    )
            elif event_type == "subagent.failed":
                sid = str(payload.get("subagentId") or "")
                item = {
                    "subagentId": sid,
                    "subagentType": payload.get("subagentType"),
                    "parentToolCallId": payload.get("parentToolCallId"),
                    "message": _truncate_log_text(payload.get("message") or ""),
                }
                subagent_failures.append(item)
                issues.append({"type": "subagent", **item})
                if sid:
                    subagents.setdefault(sid, {"subagentId": sid}).update({"status": "failed", **item})
            elif event_type == "agent.tool.invoked" and payload.get("name") in {
                "run_command",
                "check_command_status",
                "stop_command",
            }:
                tool_call_id = str(payload.get("toolCallId") or payload.get("name") or "")
                args = payload.get("arguments") if isinstance(payload.get("arguments"), dict) else {}
                commands[tool_call_id] = {
                    "toolCallId": payload.get("toolCallId"),
                    "name": payload.get("name"),
                    "command": _truncate_log_text(args.get("command") or ""),
                    "commandId": args.get("command_id"),
                    "shell": args.get("shell"),
                    "cwd": args.get("cwd"),
                    "status": "invoked",
                }
            elif event_type == "agent.tool.completed" and payload.get("name") in {
                "run_command",
                "check_command_status",
                "stop_command",
            }:
                tool_call_id = str(payload.get("toolCallId") or payload.get("name") or "")
                item = commands.setdefault(
                    tool_call_id,
                    {"toolCallId": payload.get("toolCallId"), "name": payload.get("name")},
                )
                output_data = payload.get("outputData") if isinstance(payload.get("outputData"), dict) else {}
                item.update(
                    {
                        "status": output_data.get("status") or "completed",
                        "commandId": output_data.get("commandId") or item.get("commandId"),
                        "exitCode": output_data.get("exitCode"),
                        "pid": output_data.get("pid"),
                        "stdout": _truncate_log_text(output_data.get("stdout") or output_data.get("stdoutTail") or ""),
                        "stderr": _truncate_log_text(output_data.get("stderr") or output_data.get("stderrTail") or ""),
                        "outputPreview": _truncate_log_text(payload.get("outputPreview") or ""),
                    }
                )

        return {
            "runId": run.id,
            "status": status,
            "issues": issues[:50],
            "toolFailures": tool_failures[:50],
            "subagentFailures": subagent_failures[:50],
            "providerFailures": provider_failures[:50],
            "commands": list(commands.values())[:50],
            "subagents": list(subagents.values())[:50],
            "counts": {
                "issues": len(issues),
                "toolFailures": len(tool_failures),
                "subagentFailures": len(subagent_failures),
                "providerFailures": len(provider_failures),
                "commands": len(commands),
                "subagents": len(subagents),
                "logEntries": len(self._run_logs.get(run.id, [])),
            },
        }

    def _events_for_run(self, run: AgentRun) -> List[Dict[str, Any]]:
        try:
            events = self.event_bus.snapshot(run.session_id)
        except Exception:
            return []
        return [
            event
            for event in events
            if event.get("correlationId") == run.id or event.get("correlation_id") == run.id
        ]

    def _record_runtime_event(self, run_id: str | None, event_type: str, payload: Dict[str, Any]) -> None:
        if not run_id or run_id not in self._run_logs:
            return
        entry: Dict[str, Any] | None = None
        if event_type.startswith("agent.tool."):
            entry = {
                "role": "tool",
                "status": event_type.removeprefix("agent.tool."),
                "name": payload.get("name"),
                "toolCallId": payload.get("toolCallId"),
                "subagentId": payload.get("subagentId"),
                "parentToolCallId": payload.get("parentToolCallId"),
            }
            if "arguments" in payload:
                entry["arguments"] = _compact_log_value(payload.get("arguments"))
            if "outputPreview" in payload:
                entry["outputPreview"] = _truncate_log_text(payload.get("outputPreview") or "")
            if "outputData" in payload:
                entry["outputData"] = _compact_log_value(payload.get("outputData"))
            if "code" in payload:
                entry["code"] = payload.get("code")
            if "message" in payload or "reason" in payload:
                entry["message"] = _truncate_log_text(payload.get("message") or payload.get("reason") or "")
        elif event_type.startswith("subagent."):
            entry = {
                "role": "subagent",
                "status": event_type.removeprefix("subagent."),
                "subagentId": payload.get("subagentId"),
                "subagentType": payload.get("subagentType"),
                "parentToolCallId": payload.get("parentToolCallId"),
                "description": payload.get("description"),
            }
            if "summary" in payload:
                entry["summary"] = _truncate_log_text(payload.get("summary") or "")
            if "message" in payload:
                entry["message"] = _truncate_log_text(payload.get("message") or "")
        elif event_type.startswith("todo."):
            entry = {
                "role": "todo",
                "status": event_type.removeprefix("todo."),
                "todoId": payload.get("id"),
                "title": payload.get("title"),
                "todoStatus": payload.get("status"),
            }
        elif event_type in {"agent.completed", "agent.failed", "agent.cancelled"}:
            entry = {
                "role": "run",
                "status": payload.get("status") or event_type.removeprefix("agent."),
                "errorCode": payload.get("errorCode"),
                "error": _truncate_log_text(payload.get("error") or ""),
            }
        if entry is not None:
            self._append_log(run_id, entry)

    async def publish(
        self,
        session_id: str,
        event_type: str,
        domain: str,
        source_id: str,
        payload: Dict[str, Any],
        correlation_id: Optional[str] = None,
        *,
        extra: Optional[Dict[str, Any]] = None,
    ) -> None:
        # ``extra`` carries sub-agent nesting tags (parentToolCallId / subagentId /
        # subagentType) so the frontend can attach nested events to their parent
        # Task card. Existing payload keys win over extra to avoid clobbering.
        if extra:
            merged = {**extra, **payload}
        else:
            merged = payload
        self._record_runtime_event(correlation_id, event_type, merged)
        event = DebugEvent(
            id=make_id("evt"),
            session_id=session_id,
            seq=self.event_bus.next_seq(session_id),
            type=event_type,
            ts=utc_now_iso(),
            source={"domain": domain, "id": source_id},
            payload=merged,
            correlation_id=correlation_id,
        )
        await self.event_bus.publish(event)


def _safe_json_dumps(value: Any) -> str:
    try:
        return json.dumps(value, ensure_ascii=False)
    except (TypeError, ValueError):
        return "{}"


def _truncate_log_text(value: Any, limit: int = _LOG_TEXT_LIMIT) -> str:
    text = str(value or "")
    if len(text) <= limit:
        return text
    return f"{text[:limit]}…"


def _compact_log_value(value: Any, *, depth: int = 0) -> Any:
    if depth > 3:
        return _truncate_log_text(value)
    if isinstance(value, str):
        return _truncate_log_text(value)
    if isinstance(value, (int, float, bool)) or value is None:
        return value
    if isinstance(value, dict):
        compact: Dict[str, Any] = {}
        for key, item in list(value.items())[:_LOG_LIST_LIMIT]:
            compact[str(key)] = _compact_log_value(item, depth=depth + 1)
        if len(value) > _LOG_LIST_LIMIT:
            compact["_truncatedKeys"] = len(value) - _LOG_LIST_LIMIT
        return compact
    if isinstance(value, (list, tuple)):
        compact_items = [_compact_log_value(item, depth=depth + 1) for item in list(value)[:_LOG_LIST_LIMIT]]
        if len(value) > _LOG_LIST_LIMIT:
            compact_items.append({"_truncatedItems": len(value) - _LOG_LIST_LIMIT})
        return compact_items
    return _truncate_log_text(value)


def _tool_call_signature(call: Any) -> str:
    """Stable signature for repeated-tool-loop detection; ignores provider call ids."""
    try:
        args = json.dumps(call.arguments, ensure_ascii=False, sort_keys=True)
    except (TypeError, ValueError):
        args = "{}"
    return f"{call.name}:{args}"
