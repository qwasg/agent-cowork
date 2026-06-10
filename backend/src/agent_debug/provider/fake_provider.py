from __future__ import annotations

from typing import Any, AsyncIterator, Dict, List

from src.agent_debug.provider.base import LLMProvider, ModelRequestContext, ProviderResponse


class FakeProvider(LLMProvider):
    def __init__(self, scripted_outputs: List[Dict[str, Any]] | None = None) -> None:
        self._scripted_outputs = scripted_outputs or []

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        del request
        output = self._scripted_outputs.pop(0) if self._scripted_outputs else {"message": "fake"}
        return ProviderResponse(provider="fake", model=ctx.model, output=output, token_usage={"input": 1, "output": 1})

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        return ProviderResponse(provider="fake", model=ctx.model, output=request, token_usage={"input": 1, "output": 1})

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        del request
        yield {"type": "delta", "requestId": ctx.request_id, "payload": {"delta": "fake"}}
        yield {"type": "completed", "requestId": ctx.request_id, "payload": {}}

    async def healthcheck(self) -> Dict[str, Any]:
        return {"ok": True, "latencyMs": 0}
