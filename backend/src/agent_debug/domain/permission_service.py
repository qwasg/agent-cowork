"""工具权限服务（参考 Proma ``agent-permission-service.ts``）。

支持三种权限模式：

- ``bypass``（默认）：放行所有工具。中国大模型常常无法稳定通过 SDK 内置的
  ``auto`` 分类器，故默认 bypass 以保证可用性。
- ``plan``：只读 + 受限写（仅 ``.md``）+ 只读命令；拒绝其它写/执行类工具。
- ``auto``：默认放行，但「危险」工具需上层（UI）确认；这里以保守拒绝代替交互。

另支持「会话级始终允许」白名单。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Literal, Set

PermissionMode = Literal["bypass", "plan", "auto"]

# 只读 / 安全工具：在任何模式下默认放行。
READ_ONLY_TOOLS: Set[str] = {
    "read_file",
    "list_dir",
    "grep",
    "glob",
    "search",
    "web_search",
    "web_fetch",
    "todo_read",
    "get_workspace_tree",
    "read_skill",
    "check_command_status",
}

# 写 / 执行类工具：plan 模式下默认拒绝（除受限白名单）。
MUTATING_TOOLS: Set[str] = {
    "create_document",
    "write_file",
    "edit_file",
    "apply_patch",
    "delete_file",
    "run_command",
    "stop_command",
    "shell",
}

# auto 模式下视为危险、需确认的工具。
DANGEROUS_TOOLS: Set[str] = {"run_command", "stop_command", "shell", "delete_file"}


@dataclass(frozen=True)
class PermissionDecision:
    allowed: bool
    reason: str = ""


class PermissionService:
    def __init__(self, default_mode: PermissionMode = "bypass") -> None:
        self._default_mode: PermissionMode = default_mode
        self._modes: Dict[str, PermissionMode] = {}
        # session_id -> 始终允许的工具名集合。
        self._allowlist: Dict[str, Set[str]] = {}

    def set_mode(self, session_id: str, mode: PermissionMode) -> None:
        self._modes[session_id] = mode

    def get_mode(self, session_id: str) -> PermissionMode:
        return self._modes.get(session_id, self._default_mode)

    def always_allow(self, session_id: str, tool_name: str) -> None:
        self._allowlist.setdefault(session_id, set()).add(tool_name)

    def can_use_tool(
        self, tool_name: str, arguments: Dict[str, Any] | None, *, session_id: str
    ) -> PermissionDecision:
        if tool_name in self._allowlist.get(session_id, set()):
            return PermissionDecision(True, "session-allowlist")

        mode = self.get_mode(session_id)
        if mode == "bypass":
            return PermissionDecision(True, "bypass")

        if tool_name in READ_ONLY_TOOLS:
            return PermissionDecision(True, "read-only")

        if mode == "plan":
            # plan 模式：仅允许写 .md 文档。
            if tool_name in ("write_file", "edit_file", "create_document"):
                path = str((arguments or {}).get("path") or (arguments or {}).get("file") or "")
                if path.lower().endswith(".md"):
                    return PermissionDecision(True, "plan-mode markdown write")
            return PermissionDecision(False, f"plan 模式下拒绝工具：{tool_name}")

        # auto 模式：危险工具需确认（这里保守拒绝），其余放行。
        if tool_name in DANGEROUS_TOOLS:
            return PermissionDecision(False, f"auto 模式下需要确认：{tool_name}")
        return PermissionDecision(True, "auto-allow")
