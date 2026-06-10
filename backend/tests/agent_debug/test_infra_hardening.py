"""基础设施加固：重试/分类、熔断、JSONL 持久化/恢复、检查点、权限、流式、压缩。"""

from __future__ import annotations

import asyncio

import pytest

from src.agent_debug.domain.checkpoint_service import CheckpointService
from src.agent_debug.domain.context_compactor import ContextCompactor
from src.agent_debug.domain.permission_service import PermissionService
from src.agent_debug.domain.workspace_tree import WorkspaceTreeService
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.infra.jsonl_store import JsonlEventStore
from src.agent_debug.infra.retry import (
    RetryConfig,
    classify_error,
    compute_backoff_seconds,
    is_retryable_error,
)
from src.agent_debug.domain.models import DebugEvent
from src.agent_debug.provider.base import ModelRequestContext, ProviderRegistry, ProviderResponse
from src.agent_debug.provider.channel_store import ChannelStore
from src.agent_debug.provider.channels import Channel, ChannelModel
from src.agent_debug.provider.mock_provider import MockProvider
from src.agent_debug.provider.service import ProviderExecutionService, extract_text_output


# ----------------------------------------------------------------- retry / classify
def test_classify_error_buckets():
    assert classify_error(TimeoutError("boom")) == "timeout"
    assert classify_error(Exception("HTTP 429 rate limit")) == "rate_limited"
    assert classify_error(Exception("503 service unavailable")) == "transient"
    assert classify_error(ValueError("bad request")) == "fatal"


def test_is_retryable_and_backoff_growth():
    assert is_retryable_error(TimeoutError("x")) is True
    assert is_retryable_error(ValueError("nope")) is False
    cfg = RetryConfig(base_delay_seconds=0.5, jitter_ratio=0.0, max_delay_seconds=100)
    assert compute_backoff_seconds(1, cfg) == pytest.approx(0.5)
    assert compute_backoff_seconds(2, cfg) == pytest.approx(1.0)
    assert compute_backoff_seconds(3, cfg) == pytest.approx(2.0)


# ----------------------------------------------------------------- circuit breaker
class _FatalProvider:
    async def chat(self, request, ctx):
        raise ValueError("permanent failure")

    async def responses(self, request, ctx):
        raise ValueError("permanent failure")

    async def stream(self, request, ctx):
        if False:
            yield {}

    async def healthcheck(self):
        return {"ok": False}


def test_circuit_breaker_trips_after_threshold():
    registry = ProviderRegistry()
    registry.register("openai", _FatalProvider())
    registry.register("mock", MockProvider())
    service = ProviderExecutionService(registry, EventBus())

    async def _run():
        for _ in range(5):
            ctx = ModelRequestContext(request_id="r", trace_id="t", model="m", timeout_ms=1000)
            await service.execute_chat_operation(
                request={"messages": [{"role": "user", "content": "hi"}]},
                ctx=ctx,
                session_id="s",
                operation="plan_generation",
                parser=lambda resp: resp,
            )

    asyncio.run(_run())
    assert service._breakers["openai"].open is True


# ----------------------------------------------------------------- jsonl + hydrate
def test_jsonl_store_roundtrip_and_hydrate(tmp_path):
    store = JsonlEventStore(base_dir=tmp_path)
    store.append("sess", {"id": "e1", "session_id": "sess", "seq": 1, "type": "a", "ts": "t", "source": {}, "payload": {}})
    store.append("sess", {"id": "e2", "session_id": "sess", "seq": 2, "type": "b", "ts": "t", "source": {}, "payload": {}})
    events = store.read_session("sess")
    assert [e["seq"] for e in events] == [1, 2]

    bus = EventBus()
    bus.hydrate_session("sess", events)
    assert bus.latest_seq("sess") == 2
    snap = bus.snapshot("sess")
    assert len(snap) == 2

    store.truncate_after_seq("sess", 1)
    assert [e["seq"] for e in store.read_session("sess")] == [1]


def test_event_bus_persists_and_truncates(tmp_path):
    store = JsonlEventStore(base_dir=tmp_path)
    bus = EventBus(persistence=store)

    async def _pub():
        for i in range(1, 4):
            await bus.publish(
                DebugEvent(id=f"e{i}", session_id="s", seq=i, type="x", ts="t", source={}, payload={})
            )

    asyncio.run(_pub())
    assert [e["seq"] for e in store.read_session("s")] == [1, 2, 3]
    bus.truncate_to_seq("s", 2)
    assert [e["seq"] for e in store.read_session("s")] == [1, 2]


# ----------------------------------------------------------------- checkpoint / rewind
def test_checkpoint_rewind_restores_file(tmp_path):
    (tmp_path / "a.txt").write_text("original", encoding="utf-8")
    workspace = WorkspaceTreeService(root=tmp_path)
    bus = EventBus()
    service = CheckpointService(workspace, bus)

    ckpt = service.create_checkpoint("s", seq=0, paths=["a.txt"], label="before")
    workspace.write_text("a.txt", "modified")
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "modified"

    result = service.rewind(ckpt.id)
    assert "a.txt" in result["restored"]
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "original"


def test_checkpoint_rewind_deletes_new_file(tmp_path):
    workspace = WorkspaceTreeService(root=tmp_path)
    service = CheckpointService(workspace, EventBus())
    ckpt = service.create_checkpoint("s", seq=0, paths=["new.txt"], label="before")  # 不存在
    workspace.write_text("new.txt", "created later")
    result = service.rewind(ckpt.id)
    assert "new.txt" in result["deleted"]
    assert not (tmp_path / "new.txt").exists()


# ----------------------------------------------------------------- permissions
def test_permission_modes():
    svc = PermissionService()
    # 默认 bypass
    assert svc.can_use_tool("write_file", {"path": "a.py"}, session_id="s").allowed is True

    svc.set_mode("s", "plan")
    assert svc.can_use_tool("read_file", {}, session_id="s").allowed is True
    assert svc.can_use_tool("check_command_status", {}, session_id="s").allowed is True
    assert svc.can_use_tool("write_file", {"path": "a.py"}, session_id="s").allowed is False
    assert svc.can_use_tool("write_file", {"path": "notes.md"}, session_id="s").allowed is True
    assert svc.can_use_tool("create_document", {"path": "notes.md"}, session_id="s").allowed is True
    assert svc.can_use_tool("create_document", {"path": "notes.txt"}, session_id="s").allowed is False
    assert svc.can_use_tool("delete_file", {"path": "notes.md"}, session_id="s").allowed is False
    assert svc.can_use_tool("run_command", {}, session_id="s").allowed is False
    assert svc.can_use_tool("stop_command", {}, session_id="s").allowed is False

    svc.set_mode("s", "auto")
    assert svc.can_use_tool("run_command", {}, session_id="s").allowed is False
    assert svc.can_use_tool("stop_command", {}, session_id="s").allowed is False
    assert svc.can_use_tool("delete_file", {"path": "notes.md"}, session_id="s").allowed is False
    svc.always_allow("s", "delete_file")
    assert svc.can_use_tool("delete_file", {"path": "notes.md"}, session_id="s").allowed is True
    svc.always_allow("s", "stop_command")
    assert svc.can_use_tool("stop_command", {}, session_id="s").allowed is True
    svc.always_allow("s", "run_command")
    assert svc.can_use_tool("run_command", {}, session_id="s").allowed is True


# ----------------------------------------------------------------- channel store
def test_channel_store_roundtrip_and_provider_build(tmp_path):
    store = ChannelStore(store_dir=tmp_path)
    channel = Channel(
        id="c1",
        name="DeepSeek",
        provider="deepseek",
        api_key="sk-test",
        models=[ChannelModel(id="deepseek-chat")],
    )
    store.upsert_channel(channel)

    reopened = ChannelStore(store_dir=tmp_path)
    fetched = reopened.get_channel("c1")
    assert fetched is not None
    assert fetched.api_key == "sk-test"
    assert fetched.provider == "deepseek"

    provider = reopened.build_provider(fetched)
    assert getattr(provider, "_provider_type", None) == "deepseek"

    assert reopened.delete_channel("c1") is True
    assert ChannelStore(store_dir=tmp_path).get_channel("c1") is None


# ----------------------------------------------------------------- streaming
class _StreamingProvider:
    _provider_name = "stub"
    _provider_type = "deepseek"
    _channel_id = "c1"
    _default_model = "deepseek-chat"

    async def chat(self, request, ctx):
        return ProviderResponse(provider="stub", model="deepseek-chat", output="non-stream")

    async def stream(self, request, ctx):
        yield {"type": "reasoning", "text": "思考"}
        yield {"type": "text", "text": "你好"}
        yield {"type": "text", "text": "，世界"}
        yield {"type": "tool_call", "index": 0, "id": "t1", "name": "grep", "arguments_delta": '{"q":'}
        yield {"type": "tool_call", "index": 0, "id": None, "name": None, "arguments_delta": '"x"}'}
        yield {"type": "done", "finish_reason": "tool_calls"}

    async def responses(self, request, ctx):
        return await self.chat(request, ctx)

    async def healthcheck(self):
        return {"ok": True}


def test_stream_chat_operation_accumulates_text_reasoning_tools():
    registry = ProviderRegistry()
    registry.register("deepseek", _StreamingProvider())
    service = ProviderExecutionService(registry, EventBus())
    deltas: list = []

    async def _run():
        ctx = ModelRequestContext(request_id="r", trace_id="t", model="deepseek-chat", timeout_ms=5000)
        return await service.stream_chat_operation(
            request={"messages": [{"role": "user", "content": "hi"}]},
            ctx=ctx,
            session_id="s",
            operation="subagent_execution",
            parser=lambda resp: resp,
            fallback_chain=["deepseek"],
            on_delta=lambda ev: deltas.append(ev),
        )

    result = asyncio.run(_run())
    resp = result.value
    assert resp.output == "你好，世界"
    assert resp.reasoning == "思考"
    assert resp.tool_calls[0].name == "grep"
    assert resp.tool_calls[0].arguments == {"q": "x"}
    assert any(d["type"] == "text" for d in deltas)
    assert any(d["type"] == "reasoning" for d in deltas)


def test_stream_chat_operation_falls_back_to_chat_for_non_streaming_mock():
    registry = ProviderRegistry()
    registry.register("mock", MockProvider())
    service = ProviderExecutionService(registry, EventBus())

    async def _run():
        ctx = ModelRequestContext(request_id="r", trace_id="t", model="mock-model", timeout_ms=5000)
        return await service.stream_chat_operation(
            request={"messages": [{"role": "user", "content": "build something"}]},
            ctx=ctx,
            session_id="s",
            operation="plan_generation",
            parser=lambda resp: resp,
            fallback_chain=["mock"],
        )

    result = asyncio.run(_run())
    # mock 的旧协议流被忽略 -> 回退到非流式 chat，输出为结构化计划。
    assert isinstance(result.value.output, dict)
    assert "objective" in result.value.output


# ----------------------------------------------------------------- context compaction
def test_context_compactor_compacts_when_over_budget():
    registry = ProviderRegistry()
    registry.register("mock", MockProvider())
    service = ProviderExecutionService(registry, EventBus())
    compactor = ContextCompactor(service, context_budget=50, keep_recent=2)

    messages = [{"role": "system", "content": "系统"}]
    for i in range(10):
        messages.append({"role": "user", "content": f"消息内容编号 {i} " * 5})
        messages.append({"role": "assistant", "content": f"回复内容编号 {i} " * 5})

    new_messages, compacted = asyncio.run(compactor.compact(messages, "s"))
    assert compacted is True
    # system + 压缩摘要 + 最近 keep_recent 条
    assert new_messages[0]["role"] == "system"
    assert any("历史对话压缩摘要" in str(m.get("content", "")) for m in new_messages)
    assert len(new_messages) < len(messages)


def test_context_compactor_preserves_tool_call_pairs():
    registry = ProviderRegistry()
    registry.register("mock", MockProvider())
    service = ProviderExecutionService(registry, EventBus())
    compactor = ContextCompactor(service, context_budget=50, keep_recent=2)

    tool_calls = [
        {
            "id": "call_1",
            "type": "function",
            "function": {"name": "read_file", "arguments": "{\"path\":\"x\"}"},
        }
    ]
    messages = [
        {"role": "system", "content": "系统"},
        {"role": "user", "content": "很长的上下文 " * 30},
        {"role": "assistant", "content": "准备读文件", "tool_calls": tool_calls},
        {"role": "tool", "tool_call_id": "call_1", "name": "read_file", "content": "文件内容"},
        {"role": "user", "content": "继续"},
    ]

    new_messages, compacted = asyncio.run(compactor.compact(messages, "s"))

    assert compacted is True
    non_system = [m for m in new_messages if m.get("role") != "system"]
    assert compactor._is_valid_tool_sequence(non_system)
    roles = [m["role"] for m in new_messages]
    assert roles[-3:] == ["assistant", "tool", "user"]


def test_context_compactor_uses_session_budget_resolver():
    registry = ProviderRegistry()
    registry.register("mock", MockProvider())
    service = ProviderExecutionService(registry, EventBus())
    compactor = ContextCompactor(
        service,
        context_budget=50,
        context_budget_resolver=lambda _session_id: 1_000_000,
        keep_recent=2,
    )

    messages = [{"role": "system", "content": "系统"}]
    for i in range(10):
        messages.append({"role": "user", "content": f"消息内容编号 {i} " * 5})
        messages.append({"role": "assistant", "content": f"回复内容编号 {i} " * 5})

    new_messages, compacted = asyncio.run(compactor.compact(messages, "s"))

    assert compacted is False
    assert new_messages is messages
    assert compactor._budget_for_session("s") == 925_000


def test_context_compactor_reserves_output_headroom_before_compacting():
    registry = ProviderRegistry()
    registry.register("mock", MockProvider())
    service = ProviderExecutionService(registry, EventBus())
    compactor = ContextCompactor(
        service,
        context_budget=80_000,
        context_budget_resolver=lambda _session_id: 80_000,
        keep_recent=2,
    )

    assert compactor._budget_for_session("s") == 5_000

    messages = [{"role": "system", "content": "系统"}]
    for i in range(20):
        messages.append({"role": "user", "content": f"消息内容编号 {i} " * 40})
        messages.append({"role": "assistant", "content": f"回复内容编号 {i} " * 40})

    new_messages, compacted = asyncio.run(compactor.compact(messages, "s"))

    assert compacted is True
    assert len(new_messages) < len(messages)
