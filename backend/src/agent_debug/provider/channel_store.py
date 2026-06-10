"""多渠道（Channel）持久化与供应商构建。

参考 Proma ``channel-manager.ts``：把用户配置的若干供应商渠道（含 API Key、
Base URL、模型清单）加密落盘，并据此按协议构造对应的 LLM 适配器。

- 存储：``agent_channels.json``，整体经 ``CryptoStore`` 的 Fernet 加密（无
  cryptography 时降级为明文并告警，仅用于本地开发）。
- 构建：根据 ``Channel.protocol`` 选择 ``OpenAICompatibleProvider`` 或
  ``AnthropicProvider``，并注入国内厂商特化（鉴权头、超时等）。
"""

from __future__ import annotations

import json
import logging
from dataclasses import asdict
from pathlib import Path
from typing import Dict, List, Optional

from src.agent_debug.infra.crypto_store import CryptoStore
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.provider.anthropic_adapter import AnthropicProvider
from src.agent_debug.provider.base import LLMProvider
from src.agent_debug.provider.channels import (
    Channel,
    ChannelModel,
    ProviderType,
    default_base_url,
    is_anthropic_protocol,
)
from src.agent_debug.provider.openai_compat_adapter import OpenAICompatibleProvider

logger = logging.getLogger(__name__)

# 国内 Anthropic 渠道需要的特化鉴权/请求头。
_CUSTOM_HEADERS: Dict[ProviderType, Dict[str, str]] = {
    "kimi-coding": {"User-Agent": "KimiCLI/1.3"},
}
# 响应较慢的供应商使用更长的连接超时（秒）。
_LONG_TIMEOUT_PROVIDERS: Dict[ProviderType, float] = {
    "minimax": 600.0,
}


def _channel_from_dict(data: dict) -> Channel:
    models = [
        ChannelModel(id=m.get("id"), name=m.get("name", ""), enabled=bool(m.get("enabled", True)))
        for m in data.get("models", [])
        if isinstance(m, dict) and m.get("id")
    ]
    return Channel(
        id=data.get("id") or make_id("chan"),
        name=data.get("name") or data.get("id") or "channel",
        provider=data.get("provider", "custom"),
        base_url=data.get("base_url") or data.get("baseUrl") or "",
        api_key=data.get("api_key") or data.get("apiKey") or "",
        models=models,
        enabled=bool(data.get("enabled", True)),
        created_at=data.get("created_at") or data.get("createdAt") or "",
        updated_at=data.get("updated_at") or data.get("updatedAt") or "",
    )


class ChannelStore:
    def __init__(
        self,
        store_dir: str | Path | None = None,
        *,
        crypto: CryptoStore | None = None,
    ) -> None:
        self.store_dir = Path(store_dir) if store_dir else Path(__file__).resolve().parents[3]
        self.channels_file = self.store_dir / "agent_channels.json"
        self._crypto = crypto or CryptoStore(workspace_dir=self.store_dir)
        self._cache: Optional[List[Channel]] = None

    # --------------------------------------------------------------- persistence
    def _read_raw(self) -> List[dict]:
        if not self.channels_file.exists():
            return []
        try:
            blob = self.channels_file.read_bytes()
        except OSError:
            return []
        # 优先按加密读取，失败则按明文 JSON 兜底。
        cipher = getattr(self._crypto, "_cipher", None)
        if cipher is not None:
            try:
                decrypted = cipher.decrypt(blob)
                data = json.loads(decrypted.decode("utf-8"))
                return data if isinstance(data, list) else []
            except Exception:
                pass
        try:
            data = json.loads(blob.decode("utf-8"))
            return data if isinstance(data, list) else []
        except Exception:
            return []

    def _write_raw(self, channels: List[Channel]) -> None:
        payload = [asdict(c) for c in channels]
        data = json.dumps(payload, ensure_ascii=False, indent=2).encode("utf-8")
        cipher = getattr(self._crypto, "_cipher", None)
        if cipher is not None:
            try:
                self.channels_file.write_bytes(cipher.encrypt(data))
                return
            except Exception as exc:  # pragma: no cover - filesystem/permission
                logger.warning("加密写入渠道失败，降级为明文：%s", exc)
        logger.warning("cryptography 不可用，渠道以明文存储（仅限本地开发）")
        self.channels_file.write_bytes(data)

    # ------------------------------------------------------------------- queries
    def list_channels(self) -> List[Channel]:
        if self._cache is None:
            self._cache = [_channel_from_dict(d) for d in self._read_raw()]
        return list(self._cache)

    def get_channel(self, channel_id: str) -> Optional[Channel]:
        for c in self.list_channels():
            if c.id == channel_id:
                return c
        return None

    def enabled_channels(self) -> List[Channel]:
        return [c for c in self.list_channels() if c.enabled]

    def find_channel_for_model(self, model_id: str | None) -> Optional[Channel]:
        """返回首个「已启用且包含该已启用模型」的渠道。

        用于按所选模型精确路由：composer 选中某渠道贡献的模型时，
        据此把请求路由到对应渠道（及其 API Key）。
        """
        if not model_id:
            return None
        target = str(model_id).strip()
        if not target:
            return None
        for channel in self.enabled_channels():
            for model in channel.models:
                if model.enabled and model.id == target:
                    return channel
        return None

    # ------------------------------------------------------------------ mutations
    def upsert_channel(self, channel: Channel) -> Channel:
        channels = self.list_channels()
        now = utc_now_iso()
        if not channel.created_at:
            channel.created_at = now
        channel.updated_at = now
        if not channel.base_url:
            channel.base_url = default_base_url(channel.provider)
        replaced = False
        for idx, existing in enumerate(channels):
            if existing.id == channel.id:
                channels[idx] = channel
                replaced = True
                break
        if not replaced:
            channels.append(channel)
        self._cache = channels
        self._write_raw(channels)
        return channel

    def delete_channel(self, channel_id: str) -> bool:
        channels = self.list_channels()
        remaining = [c for c in channels if c.id != channel_id]
        if len(remaining) == len(channels):
            return False
        self._cache = remaining
        self._write_raw(remaining)
        return True

    # --------------------------------------------------------------- provider build
    def build_provider(self, channel: Channel) -> LLMProvider:
        default_model = channel.primary_model_id()
        headers = dict(_CUSTOM_HEADERS.get(channel.provider, {}))
        if is_anthropic_protocol(channel.provider):
            auth_token = channel.api_key if channel.provider == "kimi-coding" else None
            return AnthropicProvider(
                api_key=None if auth_token else channel.api_key,
                auth_token=auth_token,
                base_url=channel.base_url,
                provider_type=channel.provider,
                channel_id=channel.id,
                default_model=default_model,
                extra_headers=headers or None,
            )
        return OpenAICompatibleProvider(
            api_key=channel.api_key,
            base_url=channel.base_url,
            provider_type=channel.provider,
            channel_id=channel.id,
            default_model=default_model,
            extra_headers=headers or None,
            timeout_seconds=_LONG_TIMEOUT_PROVIDERS.get(channel.provider),
        )
