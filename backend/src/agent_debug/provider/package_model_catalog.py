from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any

from src.agent_debug.domain.models import AgentModelOption, AgentModelPreferences

_MODEL_LABELS: dict[str, tuple[str, str, bool]] = {
    "sonnet": ("Claude Sonnet", "balanced", True),
    "opus": ("Claude Opus", "premium", True),
    "haiku": ("Claude Haiku", "cheap", False),
    "deepseek-chat": ("DeepSeek Chat", "balanced", False),
    "deepseek-reasoner": ("DeepSeek Reasoner", "premium", True),
    "deepseek-v4-flash": ("DeepSeek V4 Flash", "balanced", False),
    "deepseek-v4-pro": ("DeepSeek V4 Pro", "premium", True),
}

_DEEPSEEK_CONTEXT_WINDOW_TOKENS = 1_000_000


class PackageModelCatalog:
    def __init__(
        self,
        package_root: str | Path | None = None,
        preferences_file: str | Path = "agent_model_preferences.json",
        channel_store: Any = None,
    ) -> None:
        self.package_root = Path(package_root) if package_root else self._default_package_root()
        self.preferences_file = Path(preferences_file)
        self._channel_store = channel_store

    def _channel_models(self) -> list[tuple[str, str, str]]:
        """返回渠道贡献的模型 ``(model_id, label, provider_type)`` 列表。"""
        store = self._channel_store
        if store is None:
            try:
                from src.agent_debug.provider.channel_store import ChannelStore

                store = ChannelStore()
            except Exception:
                return []
        out: list[tuple[str, str, str]] = []
        try:
            for channel in store.enabled_channels():
                for model in channel.models:
                    if not model.enabled:
                        continue
                    label = model.name or model.id
                    out.append((model.id, f"{label} · {channel.name}", channel.provider))
        except Exception:
            return []
        return out

    def list_models(self) -> list[AgentModelOption]:
        model_ids = self._model_ids_for_settings_list()
        default_model_id = self.get_default_model_id()
        package_availability = "available" if self.package_root.exists() else "unavailable"
        openai_configured = bool(os.getenv("OPENAI_API_KEY", "").strip())
        channel_label_map = {mid: (lbl, ptype) for mid, lbl, ptype in self._channel_models()}
        options: list[AgentModelOption] = []
        for model_id in model_ids:
            label, tier, supports_reasoning = _MODEL_LABELS.get(
                model_id, (model_id.replace("-", " ").title(), "balanced", False)
            )
            if model_id in channel_label_map:
                ch_label, ch_provider = channel_label_map[model_id]
                label = ch_label
                provider = ch_provider
                source = "channel"
                availability = "available"
                supports_reasoning = supports_reasoning or ("reasoner" in model_id or "thinking" in model_id)
            else:
                provider = "openai-compatible" if openai_configured and model_id in self._openai_user_model_ids() else "package-agent"
                source = "OPENAI_MODEL / AGENT_DEBUG_OPENAI_MODEL_IDS" if provider == "openai-compatible" else "package/sdk-tools.d.ts"
                availability = "available" if provider == "openai-compatible" else package_availability
            options.append(
                AgentModelOption(
                    id=model_id,
                    label=label,
                    provider=provider,
                    source=source,
                    tier=tier,
                    supports_reasoning=supports_reasoning,
                    context_window_tokens=self.context_window_tokens(model_id, provider=provider),
                    availability=availability,
                    is_default=model_id == default_model_id,
                )
            )
        return [option for option in options if option.availability == "available"]

    def context_window_tokens(self, model_id: str | None, *, provider: str | None = None) -> int | None:
        model = str(model_id or "").strip().lower()
        provider_name = str(provider or "").strip().lower()
        if provider_name == "deepseek" or model.startswith("deepseek-") or "deepseek" in model:
            return _DEEPSEEK_CONTEXT_WINDOW_TOKENS
        return None

    def get_preferences(self) -> AgentModelPreferences:
        return AgentModelPreferences(global_default_model_id=self.get_default_model_id())

    def get_default_model_id(self) -> str | None:
        # Prefer the live OpenAI-compatible model when configured so a stale
        # local preference like "sonnet" cannot break DeepSeek smoke tests.
        if os.getenv("OPENAI_API_KEY", "").strip() and os.getenv("AGENT_DEBUG_PREFER_OPENAI_ENV_MODEL", "1") not in (
            "0",
            "false",
            "no",
        ):
            env_model = (os.getenv("OPENAI_MODEL") or "").strip()
            if env_model and self.is_valid_model(env_model):
                return env_model

        preferences = self._load_preferences()
        preferred_model_id = preferences.get("global_default_model_id")
        if self.is_valid_model(preferred_model_id):
            return preferred_model_id

        env_model = os.getenv("OPENAI_MODEL")
        if self.is_valid_model(env_model):
            return env_model

        model_ids = self._available_discover_model_ids()
        return model_ids[0] if model_ids else None

    def set_default_model_id(self, model_id: str) -> AgentModelPreferences:
        normalized_model_id = self.normalize_model_id(model_id)
        if normalized_model_id is None:
            raise ValueError(f"Unknown model id: {model_id}")

        payload = {
            "global_default_model_id": normalized_model_id,
        }
        self.preferences_file.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
        return AgentModelPreferences(global_default_model_id=normalized_model_id)

    def resolve_model(self, session_override: str | None = None) -> str:
        normalized_session_model = self.normalize_model_id(session_override)
        if normalized_session_model is not None:
            return normalized_session_model

        default_model_id = self.get_default_model_id()
        if default_model_id is not None:
            return default_model_id

        env_model = os.getenv("OPENAI_MODEL")
        if env_model:
            return env_model

        return "mock-model"

    def normalize_model_id(self, model_id: str | None) -> str | None:
        if not model_id:
            return None
        candidate = str(model_id).strip()
        if not candidate:
            return None
        available_ids = set(self._available_discover_model_ids())
        return candidate if candidate in available_ids else None

    def is_valid_model(self, model_id: str | None) -> bool:
        return self.normalize_model_id(model_id) is not None

    def _openai_user_model_ids(self) -> set[str]:
        if not os.getenv("OPENAI_API_KEY", "").strip():
            return set()
        ids: set[str] = set()
        for part in (os.getenv("AGENT_DEBUG_OPENAI_MODEL_IDS") or "").split(","):
            p = part.strip()
            if p:
                ids.add(p)
        env_model = (os.getenv("OPENAI_MODEL") or "").strip()
        if env_model:
            ids.add(env_model)
        return ids

    def _all_discover_model_ids(self) -> list[str]:
        sdk_tools_path = self.package_root / "sdk-tools.d.ts"
        if not sdk_tools_path.exists():
            base = list(_MODEL_LABELS.keys())
        else:
            content = sdk_tools_path.read_text(encoding="utf-8")
            match = re.search(r'model\?:\s*([^;]+);', content)
            if not match:
                base = list(_MODEL_LABELS.keys())
            else:
                model_ids = re.findall(r'"([^"]+)"', match.group(1))
                unique_ids: list[str] = []
                for model_id in model_ids:
                    if model_id not in unique_ids:
                        unique_ids.append(model_id)
                base = unique_ids or list(_MODEL_LABELS.keys())

        channel_ids = [mid for mid, _, _ in self._channel_models()]
        extra = sorted(self._openai_user_model_ids() - set(base) - set(channel_ids))
        channel_extra: list[str] = []
        for mid in channel_ids:
            if mid not in base and mid not in extra and mid not in channel_extra:
                channel_extra.append(mid)
        return channel_extra + extra + base

    def _available_discover_model_ids(self) -> list[str]:
        """模型选择器只暴露当前能够路由到真实 provider 的模型。"""
        all_ids = self._all_discover_model_ids()
        channel_ids = {mid for mid, _, _ in self._channel_models()}
        openai_ids = self._openai_user_model_ids()
        openai_configured = bool(os.getenv("OPENAI_API_KEY", "").strip())
        package_available = self.package_root.exists()
        available: list[str] = []
        for model_id in all_ids:
            if model_id in channel_ids:
                available.append(model_id)
            elif openai_configured and model_id in openai_ids:
                available.append(model_id)
            elif package_available and model_id not in openai_ids:
                available.append(model_id)
        return available

    def _only_openai_models_for_settings_ui(self) -> bool:
        v = (os.getenv("AGENT_DEBUG_ONLY_OPENAI_MODELS") or "").strip().lower()
        if v in ("0", "false", "no", "off", "all"):
            return False
        return v in ("1", "true", "yes", "on")

    def _model_ids_for_settings_list(self) -> list[str]:
        all_ids = self._all_discover_model_ids()
        if not self._only_openai_models_for_settings_ui():
            return all_ids
        if not os.getenv("OPENAI_API_KEY", "").strip():
            return all_ids
        openai_ids = self._openai_user_model_ids()
        if not openai_ids:
            return all_ids
        filtered = [mid for mid in all_ids if mid in openai_ids]
        return filtered if filtered else all_ids

    def _load_preferences(self) -> dict[str, Any]:
        if not self.preferences_file.exists():
            return {}
        try:
            return json.loads(self.preferences_file.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}

    def _default_package_root(self) -> Path:
        return Path(__file__).resolve().parents[4] / "package"
