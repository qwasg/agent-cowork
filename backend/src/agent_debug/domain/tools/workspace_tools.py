"""Workspace-bound built-in tools: read_file / list_dir / grep / write_file / create_document.

Every tool reuses :class:`WorkspaceTreeService` for its path containment
checks so we cannot accidentally escape the configured workspace root via the
agent loop. Errors are surfaced as :class:`ToolExecutionError` with stable
codes so the runtime layer can publish them as ``agent.tool.failed`` events
with the same vocabulary used by the REST gateway.
"""

from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List

from src.agent_debug.domain.tools.base import (
    AgentTool,
    ToolExecutionContext,
    ToolExecutionError,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.workspace_tree import WorkspaceTreeService


_GREP_DEFAULT_LIMIT = 200
_GREP_HARD_LIMIT = 2000
_GREP_MAX_FILES = 5000
_GREP_BLOCKED_DIRS: frozenset[str] = frozenset(
    {".git", "node_modules", "__pycache__", "dist", "build", "target", ".venv", ".pytest_cache"}
)
_DOCUMENT_SUFFIXES: frozenset[str] = frozenset({".md", ".markdown", ".txt"})


def _str_arg(args: Dict[str, Any], key: str, *, required: bool = True, default: str = "") -> str:
    value = args.get(key)
    if value is None:
        if required:
            raise ToolExecutionError("TOOL_INVALID_ARGS", f"missing argument: {key}")
        return default
    if not isinstance(value, str):
        raise ToolExecutionError("TOOL_INVALID_ARGS", f"argument {key} must be a string")
    return value


def _int_arg(args: Dict[str, Any], key: str, *, default: int) -> int:
    value = args.get(key)
    if value is None:
        return default
    try:
        return int(value)
    except (TypeError, ValueError) as exc:
        raise ToolExecutionError("TOOL_INVALID_ARGS", f"argument {key} must be int") from exc


@dataclass
class ReadFileTool(AgentTool):
    workspace: WorkspaceTreeService
    name: str = "read_file"
    description: str = (
        "Read a UTF-8 text file from the workspace by relative path. "
        "Returns the full file content (subject to the workspace size cap)."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path.",
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        rel_path = _str_arg(args, "path")
        try:
            payload = self.workspace.read_text(rel_path)
        except FileNotFoundError as exc:
            raise ToolExecutionError("PATH_NOT_FOUND", str(exc))
        except IsADirectoryError as exc:
            raise ToolExecutionError("PATH_IS_DIRECTORY", str(exc))
        except ValueError as exc:
            raise ToolExecutionError("PATH_OUTSIDE_ROOT", str(exc))
        except OSError as exc:
            raise ToolExecutionError("FILESYSTEM_ERROR", str(exc))
        text = str(payload.get("content") or "")
        return ToolResult(output=payload, text=text)


@dataclass
class ListDirTool(AgentTool):
    workspace: WorkspaceTreeService
    name: str = "list_dir"
    description: str = (
        "List the immediate children of a workspace directory (single level). "
        "Returns names, kinds and git status."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative directory path. Empty means root.",
                },
                "show_heavy": {
                    "type": "boolean",
                    "description": "Include heavy dirs like node_modules / __pycache__.",
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        rel_path = _str_arg(args, "path", required=False, default="")
        show_heavy = bool(args.get("show_heavy", False))
        try:
            payload = self.workspace.list(rel_path, show_heavy=show_heavy)
        except FileNotFoundError as exc:
            raise ToolExecutionError("PATH_NOT_FOUND", str(exc))
        except NotADirectoryError as exc:
            raise ToolExecutionError("PATH_NOT_DIRECTORY", str(exc))
        except ValueError as exc:
            raise ToolExecutionError("PATH_OUTSIDE_ROOT", str(exc))
        text = "\n".join(
            f"{entry['kind'][:1]} {entry['relPath']}" for entry in payload.get("entries", [])
        )
        return ToolResult(output=payload, text=text)


@dataclass
class GrepTool(AgentTool):
    workspace: WorkspaceTreeService
    name: str = "grep"
    description: str = (
        "Search workspace files for a regex pattern (Python ``re`` syntax). "
        "Returns up to ``limit`` matches with file path, line number and a snippet."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Python re.compile pattern.",
                },
                "path": {
                    "type": "string",
                    "description": "Workspace-relative root to search under (default = workspace root).",
                },
                "ignore_case": {"type": "boolean"},
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": _GREP_HARD_LIMIT,
                    "description": (
                        f"Max matches to return (default {_GREP_DEFAULT_LIMIT}, "
                        f"hard cap {_GREP_HARD_LIMIT})."
                    ),
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        pattern_src = _str_arg(args, "pattern")
        flags = re.IGNORECASE if bool(args.get("ignore_case", False)) else 0
        try:
            pattern = re.compile(pattern_src, flags)
        except re.error as exc:
            raise ToolExecutionError("TOOL_INVALID_ARGS", f"invalid regex: {exc}")

        rel_path = _str_arg(args, "path", required=False, default="")
        limit = max(1, min(_GREP_HARD_LIMIT, _int_arg(args, "limit", default=_GREP_DEFAULT_LIMIT)))

        # Resolve the search root within the workspace boundaries.
        try:
            search_root = self.workspace._resolve(rel_path or "")  # noqa: SLF001 - controlled re-use
        except ValueError as exc:
            raise ToolExecutionError("PATH_OUTSIDE_ROOT", str(exc))
        if not search_root.exists():
            raise ToolExecutionError("PATH_NOT_FOUND", str(search_root))
        if not search_root.is_dir():
            # Allow grep on a single file by adjusting the iteration.
            files_iter: List[Path] = [search_root]
        else:
            files_iter = list(self._walk(search_root))

        matches: List[Dict[str, Any]] = []
        for file_path in files_iter:
            try:
                with file_path.open("r", encoding="utf-8", errors="replace") as fh:
                    for line_no, line in enumerate(fh, start=1):
                        if not pattern.search(line):
                            continue
                        snippet = line.rstrip("\n")[:400]
                        try:
                            rel = file_path.resolve().relative_to(self.workspace.root).as_posix()
                        except ValueError:
                            rel = str(file_path)
                        matches.append(
                            {
                                "path": rel,
                                "line": line_no,
                                "text": snippet,
                            }
                        )
                        if len(matches) >= limit:
                            break
            except OSError:
                continue
            if len(matches) >= limit:
                break

        text_lines = [f"{m['path']}:{m['line']}: {m['text']}" for m in matches]
        return ToolResult(output={"matches": matches, "limit": limit}, text="\n".join(text_lines))

    def _walk(self, root: Path) -> List[Path]:
        files: List[Path] = []
        scanned = 0
        for current_root, dirnames, filenames in os.walk(root):
            # Prune blocked dirs in place
            dirnames[:] = [d for d in dirnames if d not in _GREP_BLOCKED_DIRS]
            for fname in filenames:
                if fname.startswith("."):
                    # Skip hidden files but allow walking into them via dirnames=…
                    continue
                files.append(Path(current_root) / fname)
                scanned += 1
                if scanned >= _GREP_MAX_FILES:
                    return files
        return files


@dataclass
class WriteFileTool(AgentTool):
    workspace: WorkspaceTreeService
    name: str = "write_file"
    description: str = (
        "Create or overwrite a UTF-8 text file in the workspace by relative path. "
        "Creates parent directories when needed."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["path", "content"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path.",
                },
                "content": {
                    "type": "string",
                    "description": "UTF-8 text content to write.",
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        del ctx
        rel_path = _str_arg(args, "path")
        content = _str_arg(args, "content")
        try:
            payload = self.workspace.write_text(rel_path, content)
        except IsADirectoryError as exc:
            raise ToolExecutionError("PATH_IS_DIRECTORY", str(exc))
        except ValueError as exc:
            raise ToolExecutionError("PATH_OUTSIDE_ROOT", str(exc))
        except TypeError as exc:
            raise ToolExecutionError("TOOL_INVALID_ARGS", str(exc))
        except OSError as exc:
            raise ToolExecutionError("FILESYSTEM_ERROR", str(exc))
        text = f"wrote {payload.get('bytesWritten', 0)} bytes to {rel_path}"
        return ToolResult(output=payload, text=text)


@dataclass
class CreateDocumentTool(AgentTool):
    workspace: WorkspaceTreeService
    name: str = "create_document"
    description: str = (
        "Create or overwrite a workspace document (.md, .markdown, .txt) with UTF-8 text content."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["path", "content"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative document path ending in .md, .markdown, or .txt.",
                },
                "content": {
                    "type": "string",
                    "description": "Document body to write.",
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        del ctx
        rel_path = _str_arg(args, "path")
        content = _str_arg(args, "content")
        if Path(rel_path).suffix.lower() not in _DOCUMENT_SUFFIXES:
            raise ToolExecutionError(
                "TOOL_INVALID_ARGS",
                "document path must end with .md, .markdown, or .txt",
            )
        try:
            payload = self.workspace.write_text(rel_path, content)
        except IsADirectoryError as exc:
            raise ToolExecutionError("PATH_IS_DIRECTORY", str(exc))
        except ValueError as exc:
            raise ToolExecutionError("PATH_OUTSIDE_ROOT", str(exc))
        except TypeError as exc:
            raise ToolExecutionError("TOOL_INVALID_ARGS", str(exc))
        except OSError as exc:
            raise ToolExecutionError("FILESYSTEM_ERROR", str(exc))
        output = dict(payload)
        output["kind"] = "document"
        text = f"created document {rel_path}"
        return ToolResult(output=output, text=text)


def build_default_workspace_tools(
    workspace: WorkspaceTreeService,
) -> WorkspaceToolRegistry:
    from src.agent_debug.domain.tools.skill_tools import register_skill_tools
    from src.agent_debug.domain.tools.subagent_tools import TaskTool
    from src.agent_debug.domain.tools.todo_tools import TodoWriteTool

    registry = WorkspaceToolRegistry()
    registry.register(ReadFileTool(workspace=workspace))
    registry.register(ListDirTool(workspace=workspace))
    registry.register(GrepTool(workspace=workspace))
    registry.register(WriteFileTool(workspace=workspace))
    registry.register(CreateDocumentTool(workspace=workspace))
    # Model-driven todo authoring; execution intercepted by the runtime.
    registry.register(TodoWriteTool())
    # Sub-agent delegation (Task); execution intercepted by the runtime, which
    # spawns a nested ReAct loop with a restricted tool allowlist.
    registry.register(TaskTool())
    # Disk SKILL.md support: let the model read full skill content on demand.
    register_skill_tools(registry, workspace=workspace)
    return registry


def serialise_tool_result(result: ToolResult) -> str:
    """Serialise a tool result for the ``role: tool`` message content."""
    if result.text:
        # Wrap structured payload alongside the text snippet so the model can
        # see both. Keep it concise to avoid blowing context.
        return result.text
    try:
        return json.dumps(result.output, ensure_ascii=False)[:4000]
    except TypeError:
        return str(result.output)[:4000]
