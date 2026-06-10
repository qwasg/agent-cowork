"""Encrypted persistence for the Tavily search configuration.

重建说明：原文件随 ``backend/`` 目录意外丢失。存储方式对齐
``ChannelStore``：``backend/agent_search_config.json``，优先用
``CryptoStore`` 的 Fernet cipher 整体加密，cryptography 缺失时降级明文。
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Optional

from src.agent_debug.domain.search_config import SearchApiConfig
from src.agent_debug.infra.crypto_store import CryptoStore
from src.agent_debug.infra.utils import utc_now_iso

logger = logging.getLogger(__name__)


class SearchConfigStore:
    def __init__(
        self,
        store_dir: str | Path | None = None,
        *,
        crypto: CryptoStore | None = None,
    ) -> None:
        # parents[3] == backend/（与 ChannelStore 一致）。
        self.store_dir = Path(store_dir) if store_dir else Path(__file__).resolve().parents[3]
        self.config_file = self.store_dir / "agent_search_config.json"
        self._crypto = crypto or CryptoStore(workspace_dir=self.store_dir)
        self._cache: Optional[SearchApiConfig] = None

    # --------------------------------------------------------------- persistence
    def _read_raw(self) -> dict:
        if not self.config_file.exists():
            return {}
        try:
            blob = self.config_file.read_bytes()
        except OSError:
            return {}
        cipher = getattr(self._crypto, "_cipher", None)
        if cipher is not None:
            try:
                decrypted = cipher.decrypt(blob)
                data = json.loads(decrypted.decode("utf-8"))
                return data if isinstance(data, dict) else {}
            except Exception:
                pass
        try:
            data = json.loads(blob.decode("utf-8"))
            return data if isinstance(data, dict) else {}
        except Exception:
            return {}

    def _write_raw(self, config: SearchApiConfig) -> None:
        data = json.dumps(config.to_dict(), ensure_ascii=False, indent=2).encode("utf-8")
        cipher = getattr(self._crypto, "_cipher", None)
        if cipher is not None:
            try:
                self.config_file.write_bytes(cipher.encrypt(data))
                return
            except Exception as exc:  # pragma: no cover - filesystem/permission
                logger.warning("加密写入搜索配置失败，降级为明文：%s", exc)
        logger.warning("cryptography 不可用，搜索配置以明文存储（仅限本地开发）")
        self.config_file.write_bytes(data)

    # ------------------------------------------------------------------- queries
    def get_config(self) -> SearchApiConfig:
        if self._cache is None:
            self._cache = SearchApiConfig.from_dict(self._read_raw())
        return self._cache

    def save_config(self, config: SearchApiConfig) -> SearchApiConfig:
        now = utc_now_iso()
        config.updated_at = now
        if not config.created_at:
            config.created_at = now
        self._write_raw(config)
        self._cache = config
        return config
