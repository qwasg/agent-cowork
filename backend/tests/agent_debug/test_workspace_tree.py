"""Workspace tree: directory browser + persistent root marker.

Covers the Cursor-style workspace switcher backend contract:
- ``browse_directories`` lists sub-directories of arbitrary local paths
  (not bound to the current workspace root) and reports errors structurally;
- ``WorkspaceTreeService`` persists the selected root so a backend restart
  resumes the user's last workspace ("常驻目录").
"""

from __future__ import annotations

from pathlib import Path

import pytest

from src.agent_debug.domain import workspace_tree as wt
from src.agent_debug.domain.workspace_tree import WorkspaceTreeService, browse_directories
from src.agent_debug.api.rest_gateway import AgentDebugRestGateway


def test_browse_directories_lists_subdirectories(tmp_path: Path) -> None:
    (tmp_path / "alpha").mkdir()
    (tmp_path / "beta").mkdir()
    (tmp_path / "beta" / "nested").mkdir()
    (tmp_path / ".hidden").mkdir()
    (tmp_path / "afile.txt").write_text("x", encoding="utf-8")

    result = browse_directories(str(tmp_path))

    names = [entry["name"] for entry in result["entries"]]
    assert names == ["alpha", "beta"]  # sorted, no hidden, no files
    beta = next(e for e in result["entries"] if e["name"] == "beta")
    alpha = next(e for e in result["entries"] if e["name"] == "alpha")
    assert beta["hasChildren"] is True
    assert alpha["hasChildren"] is False
    assert result["path"] == str(tmp_path.resolve())
    assert result["parent"] == str(tmp_path.resolve().parent)
    assert isinstance(result["places"], list)


def test_browse_directories_includes_desktop_quick_place(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home = tmp_path / "home"
    (home / "Desktop").mkdir(parents=True)
    monkeypatch.setattr(wt.Path, "home", classmethod(lambda cls: home))

    result = browse_directories(str(tmp_path))

    desktop = next((p for p in result["places"] if p["name"] == "桌面"), None)
    assert desktop is not None
    assert desktop["path"] == str(home / "Desktop")
    # Quick places are also present at the drive-list / root branch.
    root_result = browse_directories("")
    assert any(p["name"] == "桌面" for p in root_result["places"])


def test_browse_directories_invalid_path_raises() -> None:
    with pytest.raises(ValueError):
        browse_directories(str(Path("/this/path/should/not/exist/xyz")))


def test_gateway_browse_directories_wraps_errors() -> None:
    gateway = AgentDebugRestGateway()
    bad = gateway.browse_directories("/definitely/missing/dir/zzz")
    assert bad["error"]["code"] in {"INVALID_PATH", "PATH_NOT_FOUND"}


def test_set_root_persists_and_restores(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    marker = tmp_path / ".agent_workspace_root"
    monkeypatch.setattr(wt, "_persisted_root_file", lambda: marker)
    # Ensure env injection does not shadow the persisted marker in this test.
    monkeypatch.delenv("AGENT_DEBUG_WORKSPACE_ROOT", raising=False)

    workspace = tmp_path / "ws"
    workspace.mkdir()

    service = WorkspaceTreeService(root=tmp_path)
    service.set_root(workspace)

    assert marker.read_text(encoding="utf-8").strip() == str(workspace.resolve())

    # A fresh service (simulating a backend restart) resumes the persisted root.
    restored = WorkspaceTreeService()
    assert restored.root == workspace.resolve()
