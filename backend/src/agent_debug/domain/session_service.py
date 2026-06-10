from __future__ import annotations

import json
import logging
import os
import tempfile
import threading
from pathlib import Path

from src.agent_debug.domain.models import DebugSession
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso

logger = logging.getLogger(__name__)

DEFAULT_SESSION_TITLES = {
    "",
    "Agent Debug Session",
    "新的 Agent Debug 会话",
}
_TITLE_MAX_CHARS = 18


def is_default_session_title(title: str | None) -> bool:
    return (title or "").strip() in DEFAULT_SESSION_TITLES


def _default_data_file() -> str:
    """会话索引文件的稳定锚定路径。

    历史实现使用相对 cwd 的 ``"agent_sessions.json"``，导致从项目根目录与
    ``backend/`` 启动时各自读写不同的文件，表现为「删除后重启又复活」。这里
    统一锚定到与 ``JsonlEventStore`` 相同的基准目录（默认为 ``backend/``），
    并支持 ``AGENT_DEBUG_DATA_DIR`` 覆盖。
    """
    override = os.getenv("AGENT_DEBUG_DATA_DIR")
    base = Path(override).expanduser() if override else Path(__file__).resolve().parents[3]
    return str(base / "agent_sessions.json")


def derive_session_title_from_input(user_input: str) -> str:
    text = " ".join(str(user_input or "").strip().split())
    for prefix in ("/plan", "/ask", "/debug", "/build"):
        if text.lower().startswith(prefix):
            text = text[len(prefix):].strip()
            break
    for sep in ("。", "！", "？", "；", "\n", ".", "!", "?", ";"):
        if sep in text:
            text = text.split(sep, 1)[0].strip()
    text = text.strip(" #*-_`'\"“”‘’")
    if not text:
        return "新会话"
    return text[:_TITLE_MAX_CHARS]


class SessionService:
    """Persists ``DebugSession`` records as a single JSON array.

    Writes go through a tmp-file + atomic ``os.replace`` to survive crashes
    and concurrent writers. A process-wide ``RLock`` serialises the writes
    so two coroutines patching different sessions never partially overwrite
    each other.
    """

    def __init__(self, data_file: str | None = None) -> None:
        self.sessions = InMemoryTable[DebugSession]()
        self.data_file = os.path.abspath(data_file or _default_data_file())
        self._write_lock = threading.RLock()
        self._load()

    def _read_file(self, path: str) -> bool:
        """读取一份会话索引文件，成功填充内存返回 True。"""
        if not os.path.exists(path):
            return False
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
                for item in data:
                    session = DebugSession(**item)
                    self.sessions.save(session.id, session)
            return True
        except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
            logger.warning(
                "Failed to load agent sessions from %s: %s", path, exc, exc_info=True
            )
        except Exception as exc:
            logger.warning(
                "Unexpected error loading agent sessions from %s: %s",
                path,
                exc,
                exc_info=True,
            )
        return False

    def _load(self) -> None:
        if self._read_file(self.data_file):
            return
        # 兼容历史数据：锚定文件不存在时，尝试从旧的 cwd 相对路径迁移一次。
        legacy = os.path.abspath("agent_sessions.json")
        if legacy != self.data_file and self._read_file(legacy):
            logger.info(
                "Migrating legacy agent sessions from %s to %s", legacy, self.data_file
            )
            self._save()

    def _save(self) -> None:
        with self._write_lock:
            payload = self.sessions.dump()
            target = os.path.abspath(self.data_file)
            target_dir = os.path.dirname(target) or "."
            try:
                os.makedirs(target_dir, exist_ok=True)
                with tempfile.NamedTemporaryFile(
                    mode="w",
                    encoding="utf-8",
                    dir=target_dir,
                    prefix=".agent_sessions.",
                    suffix=".tmp",
                    delete=False,
                ) as tmp:
                    json.dump(payload, tmp, ensure_ascii=False, indent=2)
                    tmp.flush()
                    try:
                        os.fsync(tmp.fileno())
                    except OSError:
                        pass
                    tmp_path = tmp.name
                os.replace(tmp_path, target)
            except OSError as exc:
                logger.error(
                    "Failed to write agent sessions to %s: %s",
                    self.data_file,
                    exc,
                    exc_info=True,
                )

    def create(
        self,
        title: str,
        selected_model_id: str | None = None,
        web_search_enabled: bool = False,
    ) -> DebugSession:
        now = utc_now_iso()
        session = DebugSession(
            id=make_id("sess"),
            title=title,
            status="idle",
            mode="hybrid",
            selected_model_id=selected_model_id,
            web_search_enabled=bool(web_search_enabled),
            created_at=now,
            updated_at=now,
        )
        self.sessions.save(session.id, session)
        self._save()
        return session

    def get(self, session_id: str) -> DebugSession | None:
        return self.sessions.get(session_id)

    def update_selected_model(self, session_id: str, selected_model_id: str | None) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        session.selected_model_id = selected_model_id
        session.updated_at = utc_now_iso()
        self.sessions.save(session.id, session)
        self._save()
        return session

    def update_active_plan(self, session_id: str, plan_id: str | None, status: str | None = None) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        session.active_plan_id = plan_id
        if status is not None:
            session.status = status
        session.updated_at = utc_now_iso()
        self.sessions.save(session.id, session)
        self._save()
        return session

    def update_active_run(self, session_id: str, run_id: str | None, status: str | None = None) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        session.active_run_id = run_id
        if status is not None:
            session.status = status
        session.updated_at = utc_now_iso()
        self.sessions.save(session.id, session)
        self._save()
        return session

    def update_title(
        self,
        session_id: str,
        title: str,
        *,
        manual: bool = True,
    ) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        clean = (title or "").strip()
        if not clean:
            return session
        session.title = clean
        if manual:
            session.title_manually_set = True
        session.updated_at = utc_now_iso()
        self.sessions.save(session.id, session)
        self._save()
        return session

    def auto_title_from_input(self, session_id: str, user_input: str) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        if getattr(session, "title_manually_set", False):
            return session
        if not is_default_session_title(session.title):
            return session
        title = derive_session_title_from_input(user_input)
        if not title:
            return session
        return self.update_title(session_id, title, manual=False)

    def set_pinned(self, session_id: str, pinned: bool) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        session.pinned = bool(pinned)
        self.sessions.save(session.id, session)
        self._save()
        return session

    def set_web_search_enabled(
        self, session_id: str, web_search_enabled: bool
    ) -> DebugSession | None:
        session = self.sessions.get(session_id)
        if session is None:
            return None
        session.web_search_enabled = bool(web_search_enabled)
        session.updated_at = utc_now_iso()
        self.sessions.save(session.id, session)
        self._save()
        return session

    def delete(self, session_id: str) -> bool:
        removed = self.sessions.delete(session_id)
        if removed:
            self._save()
        return removed
