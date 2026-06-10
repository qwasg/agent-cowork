from __future__ import annotations

import httpx

from src.agent_debug.api import rest_gateway as gateway_mod
from src.agent_debug.api.rest_gateway import AgentDebugRestGateway
from src.agent_debug.provider.channels import Channel, ChannelModel


def _response(url: str, payload: dict, status_code: int = 200) -> httpx.Response:
    return httpx.Response(status_code, json=payload, request=httpx.Request("GET", url))


def test_fetch_channel_models_openai_compatible(monkeypatch):
    seen = {}

    def fake_get(url, **kwargs):
        seen["url"] = url
        seen["headers"] = kwargs.get("headers")
        return _response(url, {"data": [{"id": "b-model"}, {"id": "a-model"}]})

    monkeypatch.setattr(gateway_mod.httpx, "get", fake_get)
    result = AgentDebugRestGateway().fetch_channel_models(
        {"provider": "deepseek", "baseUrl": "https://api.deepseek.com/v1", "apiKey": "sk-test"}
    )

    assert result["success"] is True
    assert seen["url"] == "https://api.deepseek.com/v1/models"
    assert seen["headers"]["Authorization"] == "Bearer sk-test"
    assert [m["id"] for m in result["models"]] == ["a-model", "b-model"]


def test_fetch_channel_models_anthropic_compatible(monkeypatch):
    seen = {}

    def fake_get(url, **kwargs):
        seen["url"] = url
        seen["headers"] = kwargs.get("headers")
        return _response(url, {"data": [{"id": "claude-x", "display_name": "Claude X"}]})

    monkeypatch.setattr(gateway_mod.httpx, "get", fake_get)
    result = AgentDebugRestGateway().fetch_channel_models(
        {"provider": "anthropic", "baseUrl": "https://api.anthropic.com", "apiKey": "sk-ant"}
    )

    assert result["success"] is True
    assert seen["url"] == "https://api.anthropic.com/v1/models"
    assert seen["headers"]["x-api-key"] == "sk-ant"
    assert result["models"] == [{"id": "claude-x", "name": "Claude X", "enabled": True}]


def test_fetch_channel_models_google_filters_generate_content(monkeypatch):
    def fake_get(url, **kwargs):
        assert kwargs["params"]["key"] == "google-key"
        return _response(
            url,
            {
                "models": [
                    {
                        "name": "models/gemini-2.0-flash",
                        "displayName": "Gemini 2.0 Flash",
                        "supportedGenerationMethods": ["generateContent"],
                    },
                    {
                        "name": "models/text-embedding-004",
                        "supportedGenerationMethods": ["embedContent"],
                    },
                ]
            },
        )

    monkeypatch.setattr(gateway_mod.httpx, "get", fake_get)
    result = AgentDebugRestGateway().fetch_channel_models(
        {
            "provider": "google",
            "baseUrl": "https://generativelanguage.googleapis.com",
            "apiKey": "google-key",
        }
    )

    assert result["success"] is True
    assert result["models"] == [
        {"id": "gemini-2.0-flash", "name": "Gemini 2.0 Flash", "enabled": True}
    ]


def test_fetch_channel_models_reuses_saved_channel_key(monkeypatch, tmp_path):
    gateway = AgentDebugRestGateway()
    gateway.channel_store.store_dir = tmp_path
    gateway.channel_store.channels_file = tmp_path / "agent_channels.json"
    gateway.channel_store._cache = []
    gateway.channel_store.upsert_channel(
        Channel(
            id="chan-test",
            name="DeepSeek",
            provider="deepseek",
            api_key="saved-key",
            models=[ChannelModel(id="old-model")],
        )
    )

    seen = {}

    def fake_get(url, **kwargs):
        seen["headers"] = kwargs.get("headers")
        return _response(url, {"data": [{"id": "deepseek-chat"}]})

    monkeypatch.setattr(gateway_mod.httpx, "get", fake_get)
    result = gateway.fetch_channel_models(
        {"channelId": "chan-test", "provider": "deepseek", "baseUrl": "https://api.deepseek.com/v1"}
    )

    assert result["success"] is True
    assert seen["headers"]["Authorization"] == "Bearer saved-key"
    assert result["models"][0]["id"] == "deepseek-chat"
