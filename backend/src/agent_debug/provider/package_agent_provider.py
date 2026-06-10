from __future__ import annotations

import importlib
import importlib.util
import os
import sys
from typing import Any, AsyncIterator, Callable, Dict

from src.agent_debug.provider.base import LLMProvider, ModelRequestContext, ProviderResponse


class PackageAgentProvider(LLMProvider):
    def __init__(self, module_name: str | None = None, factory_name: str | None = None) -> None:
        self._module_name = module_name or os.getenv("AGENT_DEBUG_PACKAGE_AGENT_MODULE", "package_agent")
        self._factory_name = factory_name or os.getenv(
            "AGENT_DEBUG_PACKAGE_AGENT_FACTORY", "create_agent"
        )
        self._provider_name = "package-agent"
        self._client: Any | None = None

    @classmethod
    def is_available(cls, module_name: str | None = None) -> bool:
        resolved_module_name = module_name or os.getenv(
            "AGENT_DEBUG_PACKAGE_AGENT_MODULE", "package_agent"
        )
        if resolved_module_name in sys.modules:
            return True
        try:
            return importlib.util.find_spec(resolved_module_name) is not None
        except (ImportError, ValueError):
            return False

    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        client = self._ensure_client()
        raw_response = await self._invoke(client, request, ctx, method_names=("chat", "run", "invoke"))
        return self._normalize_response(raw_response, ctx)

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        client = self._ensure_client()
        raw_response = await self._invoke(
            client,
            request,
            ctx,
            method_names=("responses", "chat", "run", "invoke"),
        )
        return self._normalize_response(raw_response, ctx)

    async def stream(
        self, request: Dict[str, Any], ctx: ModelRequestContext
    ) -> AsyncIterator[Dict[str, Any]]:
        response = await self.chat(request, ctx)
        yield {
            "type": "completed",
            "requestId": ctx.request_id,
            "payload": {"provider": response.provider, "output": response.output},
        }

    async def healthcheck(self) -> Dict[str, Any]:
        if not self.is_available(self._module_name):
            return {"ok": False, "reason": f"module {self._module_name} is not importable"}
        try:
            client = self._ensure_client()
            checker = getattr(client, "healthcheck", None)
            if callable(checker):
                result = checker()
                if hasattr(result, "__await__"):
                    result = await result
                if isinstance(result, dict):
                    return result
            return {"ok": True, "module": self._module_name}
        except Exception as exc:
            return {"ok": False, "reason": str(exc)}

    def _ensure_client(self) -> Any:
        if self._client is None:
            module = importlib.import_module(self._module_name)
            factory = getattr(module, self._factory_name, None)
            if callable(factory):
                self._client = factory()
            elif hasattr(module, "PackageAgent"):
                self._client = getattr(module, "PackageAgent")()
            elif hasattr(module, "agent"):
                self._client = getattr(module, "agent")
            else:
                self._client = module
        return self._client

    async def _invoke(
        self,
        client: Any,
        request: Dict[str, Any],
        ctx: ModelRequestContext,
        method_names: tuple[str, ...],
    ) -> Any:
        for method_name in method_names:
            target = getattr(client, method_name, None)
            if not callable(target):
                continue
            return await self._call_with_supported_shapes(target, request, ctx)
        raise RuntimeError(
            f"package-agent adapter could not find callable entrypoint in {self._module_name}"
        )

    async def _call_with_supported_shapes(
        self, target: Callable[..., Any], request: Dict[str, Any], ctx: ModelRequestContext
    ) -> Any:
        messages = request.get("messages", [])
        attempts = (
            lambda: target(request=request, ctx=ctx),
            lambda: target(request, ctx),
            lambda: target(messages=messages, ctx=ctx),
            lambda: target(messages=messages, context=ctx),
            lambda: target(messages),
            lambda: target(request),
        )
        last_type_error: TypeError | None = None
        for attempt in attempts:
            try:
                result = attempt()
            except TypeError as exc:
                last_type_error = exc
                continue
            if hasattr(result, "__await__"):
                return await result
            return result
        raise RuntimeError(
            "package-agent adapter could not match a supported call signature"
        ) from last_type_error

    def _normalize_response(self, value: Any, ctx: ModelRequestContext) -> ProviderResponse:
        if isinstance(value, ProviderResponse):
            return value

        if isinstance(value, dict):
            output = value.get("output", value.get("response", value))
            token_usage = value.get("token_usage") or value.get("tokenUsage") or {}
            finish_reason = value.get("finish_reason") or value.get("finishReason")
            return ProviderResponse(
                provider=str(value.get("provider") or self._provider_name),
                model=str(value.get("model") or ctx.model),
                output=output,
                token_usage=token_usage if isinstance(token_usage, dict) else {},
                finish_reason=str(finish_reason) if finish_reason is not None else None,
            )

        return ProviderResponse(
            provider=self._provider_name,
            model=ctx.model,
            output=value,
            token_usage={},
            finish_reason="stop",
        )
