"""Workspace tree service for the Agent Debug right-side IDE panel.

Produces a single-level directory listing rooted at the agent's workspace,
annotated with git status (M / U / A / D / !) so the frontend can render a
Cursor-style file tree without itself walking the filesystem or shelling out
to git.

Path containment is enforced via ``Path.resolve`` + ``is_relative_to``; any
attempt to escape the configured root raises ``ValueError`` and is converted
to a structured ``PATH_OUTSIDE_ROOT`` error by the REST gateway.
"""

from __future__ import annotations

import os
import subprocess
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

# Directories that are technically inside the workspace but almost never
# something the user wants to see in the right-side tree. They're hidden by
# default; ``show_heavy=True`` brings them back. ``.git`` is always skipped.
_HEAVY_DIRS: frozenset[str] = frozenset(
    {"node_modules", "__pycache__", "target", "dist", "build", ".venv", ".pytest_cache", ".mypy_cache", ".ruff_cache", "htmlcov"}
)

# Higher value = higher priority when aggregating per-directory git status.
_STATUS_PRIORITY: Dict[str, int] = {
    "M": 5,
    "A": 4,
    "D": 3,
    "R": 3,
    "C": 3,
    "U": 2,
    "?": 1,
    "!": 0,
}


GitStatusProvider = Callable[[Path], Dict[str, str]]


class NotAGitRepoError(RuntimeError):
    """Raised by ``checkout_head`` when the workspace root isn't a git repo.

    Surfaced as a structured ``NOT_A_GIT_REPO`` error by the REST gateway so
    the frontend can distinguish "I can't revert because there's no git here"
    from generic IO failures.
    """


def _default_git_status_provider(root: Path) -> Dict[str, str]:
    """Run ``git status --porcelain=v1 -uall`` against ``root`` and return a
    mapping ``{relPath -> singleCharStatus}``.

    Falls back to an empty mapping if git isn't available or the directory
    isn't a repo. Errors are intentionally swallowed because the workspace
    tree must continue to function on non-git workspaces.
    """

    if not (root / ".git").exists():
        return {}
    try:
        completed = subprocess.run(
            ["git", "status", "--porcelain=v1", "-uall"],
            cwd=str(root),
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        return {}
    if completed.returncode != 0:
        return {}
    return _parse_porcelain(completed.stdout)


def _parse_porcelain(text: str) -> Dict[str, str]:
    statuses: Dict[str, str] = {}
    for raw in text.splitlines():
        if len(raw) < 4:
            continue
        x, y, sep, rest = raw[0], raw[1], raw[2], raw[3:]
        if sep != " ":
            continue
        path = rest
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        path = path.strip().strip('"').replace("\\", "/")
        if not path:
            continue
        if x == "?" or y == "?":
            statuses[path] = "U"
            continue
        if x == "!" or y == "!":
            statuses[path] = "!"
            continue
        primary = x if x != " " else y
        if primary in {"R", "C"}:
            statuses[path] = "M"
        elif primary in {"M", "A", "D"}:
            statuses[path] = primary
        else:
            statuses[path] = "M"
    return statuses


def _aggregate_dir_status(rel_dir: str, statuses: Dict[str, str]) -> Optional[str]:
    """Return the highest-priority single-char status across any file whose
    relative path starts with ``rel_dir + "/"``. Used to surface a folder-level
    git indicator (matches Cursor's behaviour in the screenshot)."""

    if not rel_dir:
        return None
    prefix = rel_dir + "/"
    best: Optional[str] = None
    best_rank = -1
    for path, status in statuses.items():
        if not path.startswith(prefix):
            continue
        rank = _STATUS_PRIORITY.get(status, 0)
        if rank > best_rank:
            best = status
            best_rank = rank
    return best


def _resolve_default_root() -> Path:
    """Pick a sensible workspace root when none is configured.

    Walk up from ``cwd`` looking for a ``.git`` directory; if found use that
    ancestor. Otherwise fall back to ``cwd`` itself.
    """

    cwd = Path.cwd().resolve()
    for candidate in (cwd, *cwd.parents):
        if (candidate / ".git").exists():
            return candidate
    return cwd


def _persisted_root_file() -> Path:
    """Location of the persisted workspace-root marker.

    Stored next to the backend package working directory so a backend restart
    can resume the user's last-selected workspace ("常驻目录"). Kept as a
    dotfile alongside the other ``.agent_*`` runtime state files.
    """

    return Path.cwd().resolve() / ".agent_workspace_root"


def _load_persisted_root() -> Optional[Path]:
    marker = _persisted_root_file()
    try:
        raw = marker.read_text(encoding="utf-8").strip()
    except (OSError, ValueError):
        return None
    if not raw:
        return None
    candidate = Path(raw)
    try:
        resolved = candidate.resolve(strict=True)
    except (OSError, ValueError):
        return None
    return resolved if resolved.is_dir() else None


def _save_persisted_root(root: Path) -> None:
    marker = _persisted_root_file()
    try:
        marker.write_text(str(root), encoding="utf-8")
    except OSError:
        # Persistence is best-effort; a read-only cwd must not break switching.
        pass


def _windows_drives() -> List[str]:
    """Enumerate available drive roots on Windows (``C:\\``, ``H:\\`` ...).

    Returns an empty list on non-Windows platforms where a single ``/`` root
    is used instead.
    """

    if os.name != "nt":
        return []
    drives: List[str] = []
    for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ":
        root = f"{letter}:\\"
        if os.path.exists(root):
            drives.append(root)
    return drives


def _quick_places() -> List[Dict[str, Any]]:
    """Quick-access shortcuts for the folder picker ("快速访问").

    Currently only the user's Desktop. OneDrive-redirected desktops are
    handled by falling back to ``~/OneDrive/Desktop``. Entries are only
    included when the directory actually exists so the frontend never offers a
    dead shortcut.
    """

    places: List[Dict[str, Any]] = []
    try:
        home = Path.home()
    except (RuntimeError, OSError):
        return places
    desktop_candidates = [home / "Desktop", home / "OneDrive" / "Desktop"]
    for candidate in desktop_candidates:
        try:
            if candidate.is_dir():
                places.append({"name": "桌面", "path": str(candidate), "icon": "monitor"})
                break
        except OSError:
            continue
    return places


def browse_directories(path: str = "") -> Dict[str, Any]:
    """List immediate sub-directories of ``path`` for the workspace picker.

    Unlike :class:`WorkspaceTreeService`, this is intentionally *not* bound to
    the current workspace root: it powers a Cursor-style folder browser so the
    user can navigate the local machine's filesystem and pick any directory as
    the new workspace root. It is read-only and only ever returns directories.

    Shape::

        {
          "path": "H:\\proj",       # resolved absolute path ("" at drive list)
          "parent": "H:\\",          # parent path or None at a filesystem root
          "separator": "\\",         # os.sep, for the frontend breadcrumb
          "drives": ["C:\\", ...],   # Windows drive roots (empty elsewhere)
          "places": [{"name", "path", "icon"}],  # quick-access shortcuts
          "entries": [{"name", "path", "hasChildren"}],
        }
    """

    cleaned = (path or "").strip()
    drives = _windows_drives()
    places = _quick_places()

    # Empty path: on Windows show the drive list (no single FS root); on POSIX
    # start at "/".
    if not cleaned:
        if drives:
            return {
                "path": "",
                "parent": None,
                "separator": os.sep,
                "drives": drives,
                "places": places,
                "entries": [
                    {"name": drive, "path": drive, "hasChildren": True}
                    for drive in drives
                ],
            }
        cleaned = os.sep

    try:
        target = Path(cleaned).expanduser().resolve(strict=True)
    except (FileNotFoundError, OSError) as exc:
        raise ValueError(f"路径不存在或无法解析: {exc}") from exc
    if not target.is_dir():
        raise NotADirectoryError(str(path))

    entries: List[Dict[str, Any]] = []
    try:
        iterator = list(os.scandir(target))
    except (PermissionError, OSError) as exc:
        raise ValueError(f"无法读取目录: {exc}") from exc
    for entry in iterator:
        name = entry.name
        if name.startswith("."):
            continue
        try:
            if not entry.is_dir(follow_symlinks=False):
                continue
        except OSError:
            continue
        child = Path(entry.path)
        has_children = False
        try:
            with os.scandir(child) as sub:
                for sub_entry in sub:
                    try:
                        if sub_entry.is_dir(follow_symlinks=False) and not sub_entry.name.startswith("."):
                            has_children = True
                            break
                    except OSError:
                        continue
        except (PermissionError, OSError):
            has_children = False
        entries.append(
            {
                "name": name,
                "path": str(child),
                "hasChildren": has_children,
            }
        )
    entries.sort(key=lambda item: item["name"].lower())

    parent = target.parent
    parent_str: Optional[str]
    if parent == target:
        # Filesystem root: on Windows step back to the drive list ("").
        parent_str = "" if drives else None
    else:
        parent_str = str(parent)

    return {
        "path": str(target),
        "parent": parent_str,
        "separator": os.sep,
        "drives": drives,
        "places": places,
        "entries": entries,
    }


@dataclass(frozen=True)
class _CachedStatus:
    statuses: Dict[str, str]
    index_mtime: float


class WorkspaceTreeService:
    """Lazily lists workspace directories with git annotations.

    The git status cache is invalidated whenever ``.git/index``'s mtime
    changes. Directory listing itself is not cached; ``os.scandir`` is fast
    enough for the right-panel UI's per-click refresh pattern.
    """

    def __init__(
        self,
        root: Optional[Path] = None,
        *,
        git_status_provider: Optional[GitStatusProvider] = None,
    ) -> None:
        env_root = os.getenv("AGENT_DEBUG_WORKSPACE_ROOT")
        persisted_root = _load_persisted_root()
        if root is not None:
            chosen = Path(root)
        elif env_root:
            # Explicit env injection (e.g. Tauri shell) wins over the persisted
            # marker so a desktop launch can pin the workspace deterministically.
            chosen = Path(env_root)
        elif persisted_root is not None:
            chosen = persisted_root
        else:
            chosen = _resolve_default_root()
        self._root = chosen.resolve()
        self._git_status_provider = git_status_provider or _default_git_status_provider
        self._cache: Optional[_CachedStatus] = None
        self._lock = threading.Lock()

    @property
    def root(self) -> Path:
        with self._lock:
            return self._root

    def set_root(self, new_root: Path | str) -> Dict[str, Any]:
        """Switch the workspace root at runtime.

        Validates the path exists + is a directory, swaps it under a lock,
        and clears the git status cache so the next ``list``/``info`` call
        reflects the new tree immediately.
        """
        candidate = Path(new_root).expanduser()
        try:
            resolved = candidate.resolve(strict=True)
        except (FileNotFoundError, OSError) as exc:
            raise ValueError(f"路径不存在或无法解析: {exc}") from exc
        if not resolved.is_dir():
            raise ValueError("目标路径不是目录")
        with self._lock:
            self._root = resolved
            self._cache = None
        _save_persisted_root(resolved)
        return self.info()

    def info(self) -> Dict[str, Any]:
        with self._lock:
            root = self._root
        return {
            "root": str(root),
            "branch": self._read_branch(),
        }

    _MAX_TEXT_BYTES: int = 5 * 1024 * 1024

    def read_text(self, path: str | Path) -> Dict[str, Any]:
        target = self._resolve_any(path)
        if not target.exists():
            raise FileNotFoundError(str(path))
        if target.is_dir():
            raise IsADirectoryError(str(path))
        size = target.stat().st_size
        if size > self._MAX_TEXT_BYTES:
            raise ValueError(
                f"文件过大 ({size} bytes)，单文件上限为 {self._MAX_TEXT_BYTES // 1024 // 1024} MB"
            )
        content = target.read_text(encoding="utf-8", errors="replace")
        return {"path": str(target), "content": content}

    def write_text(self, path: str | Path, content: str) -> Dict[str, Any]:
        if not isinstance(content, str):
            raise TypeError("content 必须是字符串")
        target = self._resolve_any(path)
        if target.exists() and target.is_dir():
            raise IsADirectoryError(str(path))
        target.parent.mkdir(parents=True, exist_ok=True)
        data = content.encode("utf-8")
        target.write_bytes(data)
        return {"path": str(target), "bytesWritten": len(data)}

    def delete_file(self, path: str | Path) -> Dict[str, Any]:
        target = self._resolve_any(path)
        if not target.exists():
            raise FileNotFoundError(str(path))
        if target.is_dir():
            raise IsADirectoryError(str(path))
        target.unlink()
        return {"path": str(target), "deleted": True}

    def read_local_text(self, path: str | Path) -> Dict[str, Any]:
        target = self._resolve_local_absolute(path, require_exists=True)
        if target.is_dir():
            raise IsADirectoryError(str(path))
        size = target.stat().st_size
        if size > self._MAX_TEXT_BYTES:
            raise ValueError(
                f"文件过大 ({size} bytes)，单文件上限为 {self._MAX_TEXT_BYTES // 1024 // 1024} MB"
            )
        content = target.read_text(encoding="utf-8", errors="replace")
        return {"path": str(target), "content": content}

    def write_local_text(self, path: str | Path, content: str) -> Dict[str, Any]:
        if not isinstance(content, str):
            raise TypeError("content 必须是字符串")
        target = self._resolve_local_absolute(path, require_exists=False)
        if target.exists() and target.is_dir():
            raise IsADirectoryError(str(path))
        target.parent.mkdir(parents=True, exist_ok=True)
        data = content.encode("utf-8")
        target.write_bytes(data)
        return {"path": str(target), "bytesWritten": len(data)}

    def checkout_head(self, path: str | Path) -> Dict[str, Any]:
        """Restore a single workspace file to its ``HEAD`` content via
        ``git checkout HEAD -- <path>``.

        Validates path containment with the same `_resolve_any` helper used
        by `read_text` / `write_text`. Raises:
          * ``ValueError`` for paths outside the root or unresolvable input;
          * ``IsADirectoryError`` if the target is a directory (we don't
            support recursive revert here on purpose);
          * ``NotAGitRepoError`` if the workspace root has no ``.git``;
          * ``OSError`` if ``git`` is not installed or returns non-zero.
        """
        target = self._resolve_any(path)
        if target.is_dir():
            raise IsADirectoryError(str(path))
        with self._lock:
            root = self._root
        if not (root / ".git").exists():
            raise NotAGitRepoError("workspace 不是 git 仓库，无法回滚到 HEAD")
        rel = target.relative_to(root).as_posix()
        try:
            completed = subprocess.run(
                ["git", "checkout", "HEAD", "--", rel],
                cwd=str(root),
                capture_output=True,
                text=True,
                timeout=10,
            )
        except FileNotFoundError as exc:
            raise OSError("未找到 git 可执行程序") from exc
        except subprocess.TimeoutExpired as exc:
            raise OSError("git checkout 超时") from exc
        if completed.returncode != 0:
            stderr = (completed.stderr or "").strip() or "git checkout 失败"
            raise OSError(stderr)
        # The git index mtime invalidation will pick up on the next status
        # poll; nothing else to do here.
        return {"path": rel, "reverted": True}

    def _resolve_any(self, path: str | Path) -> Path:
        """Resolve `path` (absolute or relative to root) and require it to
        live inside the current workspace root. Used by ``read_text`` /
        ``write_text``."""
        candidate = Path(path)
        with self._lock:
            root = self._root
        if candidate.is_absolute():
            target = candidate
        else:
            target = root / candidate
        try:
            resolved = target.resolve()
        except OSError as exc:
            raise ValueError(f"无法解析路径: {exc}") from exc
        if resolved != root and root not in resolved.parents:
            raise ValueError("拒绝访问工作区之外的路径")
        return resolved

    def _resolve_local_absolute(self, path: str | Path, *, require_exists: bool) -> Path:
        candidate = Path(path).expanduser()
        if not candidate.is_absolute():
            raise ValueError("path 必须是绝对路径")
        try:
            return candidate.resolve(strict=require_exists)
        except FileNotFoundError as exc:
            raise FileNotFoundError(str(path)) from exc
        except OSError as exc:
            raise ValueError(f"无法解析路径: {exc}") from exc

    def list(self, rel_path: str = "", *, show_heavy: bool = False) -> Dict[str, Any]:
        target = self._resolve(rel_path)
        if not target.exists():
            raise FileNotFoundError(rel_path)
        if not target.is_dir():
            raise NotADirectoryError(rel_path)
        statuses = self._statuses()
        entries: List[Dict[str, Any]] = []
        try:
            iterator = list(os.scandir(target))
        except PermissionError:
            iterator = []
        for entry in iterator:
            name = entry.name
            if name == ".git":
                continue
            try:
                is_dir = entry.is_dir(follow_symlinks=False)
            except OSError:
                continue
            heavy = name in _HEAVY_DIRS
            if heavy and not show_heavy:
                continue
            hidden = name.startswith(".") or heavy
            try:
                stat_result = entry.stat(follow_symlinks=False)
            except OSError:
                stat_result = None
            try:
                rel_entry = (Path(entry.path).resolve()).relative_to(self._root)
            except ValueError:
                continue
            rel_str = rel_entry.as_posix()
            git_status = (
                _aggregate_dir_status(rel_str, statuses)
                if is_dir
                else statuses.get(rel_str)
            )
            entries.append(
                {
                    "name": name,
                    "kind": "dir" if is_dir else "file",
                    "relPath": rel_str,
                    "size": None if is_dir or stat_result is None else stat_result.st_size,
                    "modifiedAt": None if stat_result is None else stat_result.st_mtime,
                    "gitStatus": git_status,
                    "hidden": hidden,
                }
            )
        entries.sort(key=_sort_key)
        return {
            "root": str(self._root),
            "relPath": rel_path,
            "entries": entries,
            "gitBranch": self._read_branch(),
        }

    def _resolve(self, rel_path: str) -> Path:
        cleaned = (rel_path or "").strip()
        if cleaned.startswith("/") or cleaned.startswith("\\"):
            raise ValueError("rel_path must be a relative path inside the workspace root")
        if Path(cleaned).is_absolute():
            raise ValueError("rel_path must be a relative path inside the workspace root")
        candidate = (self._root / cleaned).resolve()
        if candidate != self._root and self._root not in candidate.parents:
            raise ValueError("rel_path escapes the workspace root")
        return candidate

    def _statuses(self) -> Dict[str, str]:
        index_path = self._root / ".git" / "index"
        try:
            mtime = index_path.stat().st_mtime
        except OSError:
            mtime = 0.0
        cache = self._cache
        if cache is not None and cache.index_mtime == mtime:
            return cache.statuses
        statuses = self._git_status_provider(self._root)
        self._cache = _CachedStatus(statuses=statuses, index_mtime=mtime)
        return statuses

    def _read_branch(self) -> Optional[str]:
        head = self._root / ".git" / "HEAD"
        if not head.exists():
            return None
        try:
            content = head.read_text(encoding="utf-8", errors="ignore").strip()
        except OSError:
            return None
        if content.startswith("ref:"):
            ref = content.split(":", 1)[1].strip()
            if ref.startswith("refs/heads/"):
                return ref[len("refs/heads/") :]
            return ref
        return "detached"

    def invalidate_cache(self) -> None:
        self._cache = None


def _sort_key(entry: Dict[str, Any]) -> Tuple[int, str]:
    return (0 if entry["kind"] == "dir" else 1, entry["name"].lower())
