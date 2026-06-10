"""Encrypted JSON persistence for user accounts.

重建说明：原文件随 ``backend/`` 目录意外丢失。存储方式对齐
``ChannelStore``：``backend/agent_users.json``，优先用 ``CryptoStore``
的 Fernet cipher 整体加密，cryptography 缺失时降级明文。
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Dict, List, Optional

from src.agent_debug.infra.crypto_store import CryptoStore

logger = logging.getLogger(__name__)


class UserStore:
    def __init__(
        self,
        store_dir: str | Path | None = None,
        *,
        crypto: CryptoStore | None = None,
    ) -> None:
        # parents[3] == backend/（与 ChannelStore 一致）。
        self.store_dir = Path(store_dir) if store_dir else Path(__file__).resolve().parents[3]
        self.users_file = self.store_dir / "agent_users.json"
        self._crypto = crypto or CryptoStore(workspace_dir=self.store_dir)
        self._cache: Optional[List[Dict]] = None

    # --------------------------------------------------------------- persistence
    def _read_raw(self) -> List[Dict]:
        if not self.users_file.exists():
            return []
        try:
            blob = self.users_file.read_bytes()
        except OSError:
            return []
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

    def _write_raw(self, users: List[Dict]) -> None:
        data = json.dumps(users, ensure_ascii=False, indent=2).encode("utf-8")
        cipher = getattr(self._crypto, "_cipher", None)
        if cipher is not None:
            try:
                self.users_file.write_bytes(cipher.encrypt(data))
                return
            except Exception as exc:  # pragma: no cover - filesystem/permission
                logger.warning("加密写入用户数据失败，降级为明文：%s", exc)
        logger.warning("cryptography 不可用，用户数据以明文存储（仅限本地开发）")
        self.users_file.write_bytes(data)

    # ------------------------------------------------------------------- queries
    def list_users(self) -> List[Dict]:
        if self._cache is None:
            self._cache = self._read_raw()
        return list(self._cache)

    def get_by_email(self, email: str) -> Optional[Dict]:
        target = (email or "").strip().lower()
        for user in self.list_users():
            if str(user.get("email", "")).strip().lower() == target:
                return dict(user)
        return None

    def get_by_id(self, user_id: str) -> Optional[Dict]:
        for user in self.list_users():
            if str(user.get("id")) == user_id:
                return dict(user)
        return None

    # ------------------------------------------------------------------ mutation
    def upsert(self, user: Dict) -> Dict:
        users = self.list_users()
        for i, existing in enumerate(users):
            if str(existing.get("id")) == str(user.get("id")):
                users[i] = dict(user)
                break
        else:
            users.append(dict(user))
        self._cache = users
        self._write_raw(users)
        return dict(user)
