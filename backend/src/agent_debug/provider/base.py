from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Dict, List, Optional, Protocol


@dataclass
class RetryPolicy:
    max_attempts: int = 3
    base_delay_ms: int = 300
    max_delay_ms: int = 3000
    retryable_codes: List[str] = field(default_factory=list)


@dataclass
class ModelRequestContext:
    request_id: str
    trace_id: str
    model: str
    timeout_ms: int
    retry_policy: RetryPolicy = field(default_factory=RetryPolicy)
    session_id: Optional[str] = None
    run_id: Optional[str] = None
    fallback_chain: List[str] = field(default_factory=list)
    metadata: Dict[str, str] = field(default_factory=dict)


@dataclass
class ToolCall:
    """Normalised tool-call request emitted by an LLM provider."""

    id: str
    name: str
    arguments: Dict[str, Any] = field(default_factory=dict)


@dataclass
class ProviderResponse:
    provider: str
    model: str
    output: Any
    token_usage: Dict[str, int] = field(default_factory=dict)
    finish_reason: Optional[str] = None
    tool_calls: List[ToolCall] = field(default_factory=list)
    # 推理 / 思考内容（如 DeepSeek ``reasoning_content`` 或 Anthropic thinking 块）。
    reasoning: Optional[str] = None
    # 原始 provider/model 标识，便于多渠道场景下区分厂商。
    provider_type: Optional[str] = None
    channel_id: Optional[str] = None


class LLMProvider(Protocol):
    async def chat(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        ...

    async def responses(self, request: Dict[str, Any], ctx: ModelRequestContext) -> ProviderResponse:
        ...

    async def stream(self, request: Dict[str, Any], ctx: ModelRequestContext) -> AsyncIterator[Dict[str, Any]]:
        ...

    async def healthcheck(self) -> Dict[str, Any]:
        ...


class ProviderRegistry:
    def __init__(self) -> None:
        self._providers: Dict[str, LLMProvider] = {}

    def register(self, name: str, provider: LLMProvider) -> None:
        self._providers[name] = provider

    def has(self, name: str) -> bool:
        return name in self._providers

    def names(self) -> List[str]:
        return list(self._providers.keys())

    def get(self, name: str) -> LLMProvider:
        if name not in self._providers:
            raise KeyError(f"Provider not registered: {name}")
        return self._providers[name]

    def resolve(self, chain: List[str]) -> List[LLMProvider]:
        return [self.get(name) for name in chain if name in self._providers]
