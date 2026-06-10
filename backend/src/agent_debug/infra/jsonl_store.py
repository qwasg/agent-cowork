"""按会话追加写入的 JSONL 事件持久化。

参考 Proma ``agent-session-manager.ts`` 的 append-only JSONL 方案：每个会话
一个 ``{session_id}.jsonl`` 文件，事件逐行追加。相比旧的「纯内存事件总线」，
该存储让进程重启后仍可恢复回放历史。

设计取舍：
- 单进程串行写入（配合 EventBus 的协程串行 publish），无需文件锁。
- 截断 / 回溯（rewind）通过重写文件实现，仅在低频操作中调用。
"""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path
from typing import Any, Dict, List

logger = logging.getLogger(__name__)


def _default_base_dir() -> Path:
    override = os.getenv("AGENT_DEBUG_SESSION_DIR")
    if override:
        return Path(override).expanduser()
    return Path(__file__).resolve().parents[3] / "agent-sessions"


class JsonlEventStore:
    def __init__(self, base_dir: str | Path | None = None) -> None:
        self.base_dir = Path(base_dir) if base_dir else _default_base_dir()
        try:
            self.base_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:  # pragma: no cover - filesystem/permission
            logger.warning("无法创建会话目录 %s：%s", self.base_dir, exc)

    def _path(self, session_id: str) -> Path:
        safe = "".join(c for c in session_id if c.isalnum() or c in ("-", "_")) or "session"
        return self.base_dir / f"{safe}.jsonl"

    # --------------------------------------------------------------- write
    def append(self, session_id: str, event: Dict[str, Any]) -> None:
        path = self._path(session_id)
        try:
            with path.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps(event, ensure_ascii=False) + "\n")
        except OSError as exc:  # pragma: no cover - filesystem/permission
            logger.warning("追加事件失败 %s：%s", path, exc)

    # ---------------------------------------------------------------- read
    def read_session(self, session_id: str) -> List[Dict[str, Any]]:
        path = self._path(session_id)
        if not path.exists():
            return []
        events: List[Dict[str, Any]] = []
        try:
            for line in path.read_text(encoding="utf-8").splitlines():
                line = line.strip()
                if not line:
                    continue
                try:
                    events.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
        except OSError:
            return []
        return events

    def list_sessions(self) -> List[str]:
        if not self.base_dir.exists():
            return []
        return [p.stem for p in self.base_dir.glob("*.jsonl")]

    # ------------------------------------------------------------ mutate
    def truncate_after_seq(self, session_id: str, max_seq: int) -> None:
        """保留 ``seq <= max_seq`` 的事件，重写文件（用于 rewind）。"""
        events = self.read_session(session_id)
        kept = [e for e in events if int(e.get("seq", 0)) <= max_seq]
        self._rewrite(session_id, kept)

    def _rewrite(self, session_id: str, events: List[Dict[str, Any]]) -> None:
        path = self._path(session_id)
        try:
            with path.open("w", encoding="utf-8") as fh:
                for event in events:
                    fh.write(json.dumps(event, ensure_ascii=False) + "\n")
        except OSError as exc:  # pragma: no cover
            logger.warning("重写会话文件失败 %s：%s", path, exc)

    def delete_session(self, session_id: str) -> None:
        path = self._path(session_id)
        try:
            if path.exists():
                path.unlink()
        except OSError:  # pragma: no cover
            pass
