"""按所选模型精确路由到对应渠道（含其 API Key）。"""

from __future__ import annotations

from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.provider.base import (
    ModelRequestContext,
    ProviderRegistry,
    ProviderResponse,
)
from src.agent_debug.provider.channel_store import ChannelStore
from src.agent_debug.provider.channels import Channel, ChannelModel
from src.agent_debug.provider.service import ProviderExecutionService


class _DummyProvider:
    def __init__(self, name: str) -> None:
        self._name = name

    async def chat(self, request, ctx):  # pragma: no cover - 不在本测试触发
        return ProviderResponse(provider=self._name, model=ctx.model, output="")


def _make_channel(channel_id: str, model_id: str, *, enabled: bool = True, model_enabled: bool = True) -> Channel:
    return Channel(
        id=channel_id,
        name=f"渠道-{channel_id}",
        provider="deepseek",
        api_key="sk-test",
        models=[ChannelModel(id=model_id, name=model_id, enabled=model_enabled)],
        enabled=enabled,
    )


def test_find_channel_for_model(tmp_path):
    store = ChannelStore(store_dir=tmp_path)
    store.upsert_channel(_make_channel("c1", "deepseek-chat"))
    store.upsert_channel(_make_channel("c2", "kimi-k2"))

    assert store.find_channel_for_model("deepseek-chat").id == "c1"
    assert store.find_channel_for_model("kimi-k2").id == "c2"
    assert store.find_channel_for_model("unknown-model") is None
    assert store.find_channel_for_model(None) is None


def test_find_channel_for_model_skips_disabled(tmp_path):
    store = ChannelStore(store_dir=tmp_path)
    store.upsert_channel(_make_channel("c1", "deepseek-chat", enabled=False))
    store.upsert_channel(_make_channel("c2", "kimi-k2", model_enabled=False))

    assert store.find_channel_for_model("deepseek-chat") is None
    assert store.find_channel_for_model("kimi-k2") is None


def _service_with_channel(channel_id: str) -> ProviderExecutionService:
    registry = ProviderRegistry()
    registry.register(f"channel:{channel_id}", _DummyProvider(f"channel:{channel_id}"))
    registry.register("deepseek", _DummyProvider("deepseek"))
    registry.register("mock", _DummyProvider("mock"))
    return ProviderExecutionService(registry, EventBus())


def _ctx(model: str) -> ModelRequestContext:
    return ModelRequestContext(request_id="r", trace_id="t", model=model, timeout_ms=1000)


def test_resolve_chain_prepends_matched_channel():
    service = _service_with_channel("c1")
    service.model_channel_resolver = lambda mid: "channel:c1" if mid == "deepseek-chat" else None

    chain = service._resolve_chain(None, "composer_chat", ctx=_ctx("deepseek-chat"))

    assert chain[0] == "channel:c1"
    # 仍保留默认链路用于回退，且不重复。
    assert "deepseek" in chain
    assert chain.count("channel:c1") == 1


def test_resolve_chain_without_match_keeps_default():
    service = _service_with_channel("c1")
    service.model_channel_resolver = lambda mid: "channel:c1" if mid == "deepseek-chat" else None

    chain = service._resolve_chain(None, "composer_chat", ctx=_ctx("some-other-model"))

    assert chain[0] != "channel:c1"
    assert "deepseek" in chain


def test_resolve_chain_ignores_unregistered_channel():
    service = _service_with_channel("c1")
    # 解析到一个未注册的渠道（例如未配置 API Key）时应忽略，不污染链路。
    service.model_channel_resolver = lambda mid: "channel:missing"

    chain = service._resolve_chain(None, "composer_chat", ctx=_ctx("deepseek-chat"))

    assert "channel:missing" not in chain
    assert "deepseek" in chain
