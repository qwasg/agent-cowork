"""开发联调用 LLM 默认配置（OpenAI-compatible，例如 DeepSeek）。

本模块只在非 pytest、非显式禁用时生效，且严禁在仓库内硬编码任何
``OPENAI_API_KEY``。密钥必须由开发者通过环境变量或本地 ``.env.local``
注入；所有提交/CI 必须从干净环境读取。

加载顺序（仅 setdefault，不覆盖已有 ENV）：
1. ``backend/.env.local`` (优先，个人本地配置，git ignored)
2. ``backend/.env``       (可选，团队默认非密钥配置)
3. 内置 OpenAI-compatible 缺省（仅 base_url / model，绝不含密钥）

控制开关：
- ``AGENT_DEBUG_NO_BAKED_LLM=1``      跳过本模块（用于纯 mock 调试）
- ``AGENT_DEBUG_DEV_DEFAULTS=0``      等价开关
- ``AGENT_DEBUG_ENV_FILE=path``       追加额外的 env 文件
"""

from __future__ import annotations

import logging
import os
import sys
from pathlib import Path
from typing import Iterable

logger = logging.getLogger(__name__)

# OpenAI-compatible defaults that DO NOT include any secret. Safe to commit.
_DEEPSEEK_BASE_URL = "https://api.deepseek.com/v1"
_DEEPSEEK_MODEL = "deepseek-chat"


def _truthy(value: str | None) -> bool:
    return (value or "").strip().lower() in {"1", "true", "yes", "on"}


def _candidate_env_files() -> Iterable[Path]:
    """Yield env files in priority order.

    When ``AGENT_DEBUG_ENV_FILE`` is set, it is treated as the *primary* source
    so tests / explicit invocations can fully override the defaults that live
    in ``backend/.env.local``. Without it, the historical order (.env.local
    first, then .env) is preserved.
    """
    extra = os.getenv("AGENT_DEBUG_ENV_FILE")
    if extra:
        yield Path(extra).expanduser().resolve()
    backend_root = Path(__file__).resolve().parents[2]
    yield backend_root / ".env.local"
    yield backend_root / ".env"


def _parse_env_file(path: Path) -> dict[str, str]:
    """Minimal ``KEY=VALUE`` parser. Ignores blanks and ``#`` comment lines."""
    if not path.is_file():
        return {}
    pairs: dict[str, str] = {}
    try:
        for raw_line in path.read_text(encoding="utf-8").splitlines():
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if "=" not in line:
                continue
            key, _, raw_value = line.partition("=")
            key = key.strip()
            value = raw_value.strip().strip('"').strip("'")
            if key:
                pairs[key] = value
    except OSError as exc:  # pragma: no cover - filesystem permissions
        logger.warning("Failed to read env file %s: %s", path, exc)
    return pairs


def apply_dev_llm_defaults() -> None:
    """Inject local-dev OpenAI-compatible defaults without overriding ENV.

    No-op in pytest or when explicitly disabled. NEVER bakes secrets in code.
    """
    if "pytest" in sys.modules:
        return
    if _truthy(os.getenv("AGENT_DEBUG_NO_BAKED_LLM")):
        return
    if os.getenv("AGENT_DEBUG_DEV_DEFAULTS", "1").strip().lower() in {"0", "false", "no"}:
        return

    for env_file in _candidate_env_files():
        for key, value in _parse_env_file(env_file).items():
            os.environ.setdefault(key, value)

    os.environ.setdefault("OPENAI_BASE_URL", _DEEPSEEK_BASE_URL)
    os.environ.setdefault("OPENAI_MODEL", _DEEPSEEK_MODEL)
    os.environ.setdefault("AGENT_DEBUG_PREFER_OPENAI_ENV_MODEL", "1")
    # 走 DeepSeek 等 OpenAI 兼容基座时，设置页只展示环境变量中的模型 id（不混入 package/Claude）
    if "deepseek" in (os.getenv("OPENAI_BASE_URL") or "").lower():
        os.environ.setdefault("AGENT_DEBUG_ONLY_OPENAI_MODELS", "1")

    if not (os.getenv("OPENAI_API_KEY") or "").strip():
        logger.info(
            "OPENAI_API_KEY not set; OpenAI-compatible provider will not be registered. "
            "Set the env var or place it in backend/.env.local to enable real LLM smoke."
        )
