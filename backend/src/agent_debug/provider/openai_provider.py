"""向后兼容封装：``OpenAIProvider``。

历史代码与测试依赖 ``OpenAIProvider(api_key, base_url)`` 以及 ``_base_url`` /
``_provider_name == "openai-compatible"`` 等属性。实际实现已迁移到
``openai_compat_adapter.OpenAICompatibleProvider``，此处仅做薄封装。
"""

from __future__ import annotations

from src.agent_debug.provider.openai_compat_adapter import (  # noqa: F401
    OpenAICompatibleProvider,
    _normalise_tool_calls,
    _parse_tool_call_arguments,
)


class OpenAIProvider(OpenAICompatibleProvider):
    def __init__(self, api_key: str | None = None, base_url: str | None = None) -> None:
        super().__init__(api_key=api_key, base_url=base_url, provider_type="custom")
