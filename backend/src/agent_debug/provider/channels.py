"""渠道（Channel）与供应商（Provider）类型定义。

参考 Proma ``packages/shared/src/types/channel.ts`` 移植到 Python，统一描述：

- ``ProviderType``：受支持的 AI 供应商类型（含主流中国大模型）。
- ``PROVIDER_DEFAULT_URLS`` / ``PROVIDER_LABELS``：默认 Base URL 与展示名。
- ``PROVIDER_PROTOCOL``：每个供应商走哪种协议（OpenAI 兼容 vs Anthropic Messages）。
- ``Channel`` / ``ChannelModel``：用户配置的渠道与其模型清单。

设计目标是为「中国 AI 特化」打底：默认 Base URL、协议归类、Agent 兼容性
都以国内常见配置为准，避免上层硬编码厂商细节。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Literal, Optional

# 受支持的供应商类型。保持与 Proma 一致的命名，便于前后端/文档对齐。
ProviderType = Literal[
    "anthropic",
    "openai",
    "deepseek",
    "google",
    "kimi-api",
    "kimi-coding",
    "zhipu",
    "minimax",
    "doubao",
    "qwen",
    "custom",
]

# 协议归类：决定上层选用哪个适配器构造请求体。
#   - ``openai``：走 ``/chat/completions``（OpenAI 兼容）。
#   - ``anthropic``：走 ``/v1/messages``（Anthropic Messages 协议）。
ProviderProtocol = Literal["openai", "anthropic", "google"]

ALL_PROVIDER_TYPES: tuple[ProviderType, ...] = (
    "anthropic",
    "openai",
    "deepseek",
    "google",
    "kimi-api",
    "kimi-coding",
    "zhipu",
    "minimax",
    "doubao",
    "qwen",
    "custom",
)

# 各供应商默认 Base URL（国内厂商以官方推荐端点为准）。
PROVIDER_DEFAULT_URLS: Dict[ProviderType, str] = {
    "anthropic": "https://api.anthropic.com",
    "openai": "https://api.openai.com/v1",
    # DeepSeek 同时提供 OpenAI 兼容 (/v1) 和 Anthropic (/anthropic) 端点；
    # 默认走 OpenAI 兼容，思考能力通过 thinking_capability 单独处理。
    "deepseek": "https://api.deepseek.com/v1",
    "google": "https://generativelanguage.googleapis.com",
    "kimi-api": "https://api.moonshot.cn/anthropic",
    "kimi-coding": "https://api.kimi.com/coding/v1",
    "zhipu": "https://open.bigmodel.cn/api/paas/v4",
    "minimax": "https://api.minimaxi.com/anthropic",
    "doubao": "https://ark.cn-beijing.volces.com/api/v3",
    "qwen": "https://dashscope.aliyuncs.com/compatible-mode/v1",
    "custom": "",
}

PROVIDER_LABELS: Dict[ProviderType, str] = {
    "anthropic": "Anthropic",
    "openai": "OpenAI",
    "deepseek": "DeepSeek",
    "google": "Google Gemini",
    "kimi-api": "Kimi API (Anthropic 协议)",
    "kimi-coding": "Kimi Coding Plan",
    "zhipu": "智谱 AI (GLM)",
    "minimax": "MiniMax",
    "doubao": "豆包 (火山方舟)",
    "qwen": "通义千问 (DashScope)",
    "custom": "OpenAI 兼容自定义",
}

# 每个供应商默认协议。Anthropic 协议族用于 Agent 工具循环更稳定的供应商。
PROVIDER_PROTOCOL: Dict[ProviderType, ProviderProtocol] = {
    "anthropic": "anthropic",
    "openai": "openai",
    "deepseek": "openai",
    "google": "google",
    "kimi-api": "anthropic",
    "kimi-coding": "anthropic",
    "zhipu": "openai",
    "minimax": "anthropic",
    "doubao": "openai",
    "qwen": "openai",
    "custom": "openai",
}

# 被视为「中国大模型」的供应商集合，用于特化默认值（权限、超时、提示词等）。
CHINA_PROVIDERS: frozenset[ProviderType] = frozenset(
    {"deepseek", "kimi-api", "kimi-coding", "zhipu", "minimax", "doubao", "qwen"}
)


def provider_protocol(provider: ProviderType) -> ProviderProtocol:
    return PROVIDER_PROTOCOL.get(provider, "openai")


def default_base_url(provider: ProviderType) -> str:
    return PROVIDER_DEFAULT_URLS.get(provider, "")


def provider_label(provider: ProviderType) -> str:
    return PROVIDER_LABELS.get(provider, provider)


def is_china_provider(provider: ProviderType) -> bool:
    return provider in CHINA_PROVIDERS


def is_anthropic_protocol(provider: ProviderType) -> bool:
    return provider_protocol(provider) == "anthropic"


@dataclass
class ChannelModel:
    """渠道内的单个模型配置。"""

    id: str
    name: str = ""
    enabled: bool = True

    def __post_init__(self) -> None:
        if not self.name:
            self.name = self.id


@dataclass
class Channel:
    """用户配置的供应商渠道。

    ``api_key`` 在内存对象中可能为明文；持久化由 ``ChannelStore`` 负责加密。
    """

    id: str
    name: str
    provider: ProviderType
    base_url: str = ""
    api_key: str = ""
    models: List[ChannelModel] = field(default_factory=list)
    enabled: bool = True
    created_at: str = ""
    updated_at: str = ""

    def __post_init__(self) -> None:
        if not self.base_url:
            self.base_url = default_base_url(self.provider)

    @property
    def protocol(self) -> ProviderProtocol:
        return provider_protocol(self.provider)

    @property
    def is_china(self) -> bool:
        return is_china_provider(self.provider)

    def enabled_model_ids(self) -> List[str]:
        return [m.id for m in self.models if m.enabled]

    def primary_model_id(self) -> Optional[str]:
        enabled = self.enabled_model_ids()
        return enabled[0] if enabled else (self.models[0].id if self.models else None)
