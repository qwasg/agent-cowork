"""Account registration / login / JWT-style token service.

重建说明：原文件随 ``backend/`` 目录意外丢失。接口按 ``server.py`` 的
调用面重建：

- ``register(email, password, display_name, workspace)`` → ``{"user", "token"}``
  或错误信封 ``AUTH_INVALID_INPUT`` / ``AUTH_EMAIL_TAKEN``
- ``login(email, password)`` → ``{"user", "token"}`` 或 ``AUTH_BAD_CREDENTIALS``
- ``user_from_token(token)`` → 用户 dict 或 ``None``
- ``public_user(user)`` → 去除敏感字段的用户视图
- ``update_profile(user_id, payload)`` → ``{"user"}`` 或 ``AUTH_USER_NOT_FOUND``

令牌为 HMAC-SHA256 签名的紧凑 token（``base64url(payload).signature``，
含 ``exp``），密码用 PBKDF2-HMAC-SHA256 加盐散列；签名密钥优先取
``AGENT_DEBUG_AUTH_SECRET``，否则复用 ``.agent_master.key``。
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import re
import time
from typing import Any, Dict, Optional

from src.agent_debug.infra.user_store import UserStore
from src.agent_debug.infra.utils import make_id, utc_now_iso

_EMAIL_RE = re.compile(r"^[^@\s]+@[^@\s]+\.[^@\s]+$")
_TOKEN_TTL_SECONDS = 7 * 24 * 3600
_PBKDF2_ITERATIONS = 100_000


def _b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")


def _b64url_decode(text: str) -> bytes:
    padding = "=" * (-len(text) % 4)
    return base64.urlsafe_b64decode(text + padding)


class AuthService:
    def __init__(
        self,
        user_store: UserStore | None = None,
        *,
        secret: str | bytes | None = None,
    ) -> None:
        self.users = user_store or UserStore()
        self._secret = self._resolve_secret(secret)

    # ------------------------------------------------------------------ secrets
    def _resolve_secret(self, secret: str | bytes | None) -> bytes:
        if secret:
            return secret.encode("utf-8") if isinstance(secret, str) else bytes(secret)
        env = os.getenv("AGENT_DEBUG_AUTH_SECRET", "").strip()
        if env:
            return env.encode("utf-8")
        # 复用 CryptoStore 的主密钥文件，保证重启后令牌仍有效。
        key_file = getattr(self.users._crypto, "key_file", None)
        if key_file is not None:
            try:
                if key_file.exists():
                    return key_file.read_bytes()
            except OSError:
                pass
        # 最后兜底：进程内随机密钥（重启后令牌失效，仅本地开发）。
        return os.urandom(32)

    # ----------------------------------------------------------------- password
    @staticmethod
    def _hash_password(password: str, salt: bytes) -> str:
        digest = hashlib.pbkdf2_hmac(
            "sha256", password.encode("utf-8"), salt, _PBKDF2_ITERATIONS
        )
        return _b64url(digest)

    def _make_password_record(self, password: str) -> Dict[str, str]:
        salt = os.urandom(16)
        return {
            "salt": _b64url(salt),
            "hash": self._hash_password(password, salt),
        }

    def _verify_password(self, password: str, record: Dict[str, Any]) -> bool:
        try:
            salt = _b64url_decode(str(record.get("salt") or ""))
            expected = str(record.get("hash") or "")
        except Exception:
            return False
        return hmac.compare_digest(self._hash_password(password, salt), expected)

    # ------------------------------------------------------------------- tokens
    def _sign(self, payload_b64: str) -> str:
        return _b64url(hmac.new(self._secret, payload_b64.encode("ascii"), hashlib.sha256).digest())

    def mint_token(self, user_id: str) -> str:
        payload = {"sub": user_id, "exp": int(time.time()) + _TOKEN_TTL_SECONDS}
        payload_b64 = _b64url(json.dumps(payload, separators=(",", ":")).encode("utf-8"))
        return f"{payload_b64}.{self._sign(payload_b64)}"

    def user_from_token(self, token: str) -> Optional[Dict[str, Any]]:
        if not token or "." not in token:
            return None
        payload_b64, _, signature = token.rpartition(".")
        if not payload_b64 or not hmac.compare_digest(self._sign(payload_b64), signature):
            return None
        try:
            payload = json.loads(_b64url_decode(payload_b64).decode("utf-8"))
        except Exception:
            return None
        if int(payload.get("exp") or 0) < time.time():
            return None
        user = self.users.get_by_id(str(payload.get("sub") or ""))
        return user

    # -------------------------------------------------------------------- views
    @staticmethod
    def public_user(user: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "id": user.get("id"),
            "email": user.get("email"),
            "displayName": user.get("display_name") or "",
            "workspace": user.get("workspace") or "",
            "createdAt": user.get("created_at") or "",
            "updatedAt": user.get("updated_at") or "",
        }

    # ------------------------------------------------------------------ actions
    def register(
        self,
        email: str,
        password: str,
        display_name: str = "",
        workspace: str = "",
    ) -> Dict[str, Any]:
        email = (email or "").strip().lower()
        if not _EMAIL_RE.match(email):
            return {"error": {"code": "AUTH_INVALID_INPUT", "message": "邮箱格式不正确"}}
        if len(password or "") < 6:
            return {"error": {"code": "AUTH_INVALID_INPUT", "message": "密码至少 6 位"}}
        if self.users.get_by_email(email) is not None:
            return {"error": {"code": "AUTH_EMAIL_TAKEN", "message": "该邮箱已注册"}}
        now = utc_now_iso()
        user = {
            "id": make_id("user"),
            "email": email,
            "display_name": (display_name or "").strip(),
            "workspace": (workspace or "").strip(),
            "password": self._make_password_record(password),
            "created_at": now,
            "updated_at": now,
        }
        self.users.upsert(user)
        return {"user": self.public_user(user), "token": self.mint_token(str(user["id"]))}

    def login(self, email: str, password: str) -> Dict[str, Any]:
        user = self.users.get_by_email((email or "").strip().lower())
        if not user or not self._verify_password(password or "", user.get("password") or {}):
            return {"error": {"code": "AUTH_BAD_CREDENTIALS", "message": "邮箱或密码错误"}}
        return {"user": self.public_user(user), "token": self.mint_token(str(user["id"]))}

    def update_profile(self, user_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
        user = self.users.get_by_id(user_id)
        if not user:
            return {"error": {"code": "AUTH_USER_NOT_FOUND", "message": "用户不存在"}}
        data = payload or {}
        if "displayName" in data:
            user["display_name"] = str(data.get("displayName") or "").strip()
        if "workspace" in data:
            user["workspace"] = str(data.get("workspace") or "").strip()
        new_password = str(data.get("password") or "")
        if new_password:
            if len(new_password) < 6:
                return {"error": {"code": "AUTH_INVALID_INPUT", "message": "密码至少 6 位"}}
            user["password"] = self._make_password_record(new_password)
        user["updated_at"] = utc_now_iso()
        self.users.upsert(user)
        return {"user": self.public_user(user)}
