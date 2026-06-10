"""检查点 / 回溯（checkpoint & rewind）服务。

参考 Proma 的 ``enableFileCheckpointing`` + ``rewindSession``：在 agent 改动
工作区文件前/后对相关文件做内容快照，并把检查点与当时的事件序号（seq）绑定。
回溯时同时：

1. 把快照内的文件还原到当时内容（不存在则删除）。
2. 把事件流截断到该 seq（内存缓冲 + JSONL 持久化）。

快照存于内存表（可选落盘）；属于低频操作，实现以正确性优先。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional

from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.infra.memory_store import InMemoryTable
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.domain.workspace_tree import WorkspaceTreeService


@dataclass
class Checkpoint:
    id: str
    session_id: str
    seq: int
    label: str = ""
    # rel_path -> 文件内容；None 表示「当时该文件不存在」。
    files: Dict[str, Optional[str]] = field(default_factory=dict)
    created_at: str = ""


class CheckpointService:
    def __init__(self, workspace_tree: WorkspaceTreeService, event_bus: EventBus) -> None:
        self.workspace_tree = workspace_tree
        self.event_bus = event_bus
        self.checkpoints = InMemoryTable[Checkpoint]()

    def create_checkpoint(
        self,
        session_id: str,
        *,
        seq: Optional[int] = None,
        paths: Optional[List[str]] = None,
        label: str = "",
    ) -> Checkpoint:
        resolved_seq = seq if seq is not None else self.event_bus.latest_seq(session_id)
        files: Dict[str, Optional[str]] = {}
        for rel in paths or []:
            files[rel] = self._read_or_none(rel)
        checkpoint = Checkpoint(
            id=make_id("ckpt"),
            session_id=session_id,
            seq=resolved_seq,
            label=label,
            files=files,
            created_at=utc_now_iso(),
        )
        self.checkpoints.save(checkpoint.id, checkpoint)
        return checkpoint

    def snapshot_file(self, checkpoint_id: str, rel_path: str) -> None:
        """在改动某文件前，把其原始内容并入已存在的检查点。"""
        checkpoint = self.checkpoints.get(checkpoint_id)
        if checkpoint is None:
            return
        if rel_path not in checkpoint.files:
            checkpoint.files[rel_path] = self._read_or_none(rel_path)
            self.checkpoints.save(checkpoint.id, checkpoint)

    def list_checkpoints(self, session_id: str) -> List[Checkpoint]:
        return [c for c in self.checkpoints.list_all() if c.session_id == session_id]

    def rewind(self, checkpoint_id: str) -> Dict[str, object]:
        checkpoint = self.checkpoints.get(checkpoint_id)
        if checkpoint is None:
            raise KeyError(f"checkpoint not found: {checkpoint_id}")

        restored: List[str] = []
        deleted: List[str] = []
        for rel, content in checkpoint.files.items():
            if content is None:
                if self._delete(rel):
                    deleted.append(rel)
            else:
                self.workspace_tree.write_text(rel, content)
                restored.append(rel)

        # 截断事件流到检查点时的 seq。
        self.event_bus.truncate_to_seq(checkpoint.session_id, checkpoint.seq)
        return {
            "checkpointId": checkpoint.id,
            "sessionId": checkpoint.session_id,
            "seq": checkpoint.seq,
            "restored": restored,
            "deleted": deleted,
        }

    # ------------------------------------------------------------------ helpers
    def _read_or_none(self, rel_path: str) -> Optional[str]:
        try:
            return self.workspace_tree.read_text(rel_path).get("content")
        except (FileNotFoundError, IsADirectoryError, ValueError):
            return None

    def _delete(self, rel_path: str) -> bool:
        try:
            target = self.workspace_tree._resolve_any(rel_path)  # noqa: SLF001 - 复用容器内路径校验
        except ValueError:
            return False
        path = Path(target)
        try:
            if path.exists() and path.is_file():
                path.unlink()
                return True
        except OSError:
            return False
        return False
