"""Tests for disk SKILL.md discovery + the ``read_skill`` tool + REST surface."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from src.agent_debug.api.rest_gateway import AgentDebugRestGateway
from src.agent_debug.domain.permission_service import READ_ONLY_TOOLS
from src.agent_debug.domain.tools.base import (
    ToolExecutionContext,
    ToolExecutionError,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.tools.skill_tools import ReadSkillTool, register_skill_tools
from src.agent_debug.domain.workspace_tree import WorkspaceTreeService
from src.agent_debug.prompts.skills_discovery import (
    discover_skill_records,
    discover_workspace_skills,
    resolve_skill_path,
)
from src.agent_debug.prompts.skills_reminder import format_skills_system_reminder


_SKILL_BODY = (
    "# Demo Skill\n\n"
    "Use this skill to do demo things. It explains the demo workflow in detail.\n\n"
    "## Steps\n\n1. First do this.\n2. Then do that.\n"
)


def _make_workspace_skill(root: Path, name: str = "demo", body: str = _SKILL_BODY) -> Path:
    skill_dir = root / ".cursor" / "skills" / name
    skill_dir.mkdir(parents=True, exist_ok=True)
    skill_md = skill_dir / "SKILL.md"
    skill_md.write_text(body, encoding="utf-8")
    return skill_md


@pytest.fixture(autouse=True)
def _empty_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """Point ``Path.home()`` at an empty dir so user-level skills don't leak in."""
    home = tmp_path / "_home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("USERPROFILE", str(home))
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def test_discover_skill_records_finds_workspace_skill(tmp_path: Path) -> None:
    _make_workspace_skill(tmp_path)
    records = discover_skill_records(tmp_path)
    assert len(records) == 1
    record = records[0]
    assert record.name == "demo"
    assert record.scope == "workspace"
    assert record.summary.startswith("Use this skill to do demo things")
    assert Path(record.path).is_file()

    # Backwards-compatible tuple view.
    assert discover_workspace_skills(tmp_path) == [(record.name, record.summary)]


def test_resolve_skill_path(tmp_path: Path) -> None:
    skill_md = _make_workspace_skill(tmp_path)
    resolved = resolve_skill_path(tmp_path, "demo")
    assert resolved is not None
    assert Path(resolved) == skill_md.resolve()
    assert resolve_skill_path(tmp_path, "missing") is None


def test_read_skill_tool_returns_full_content(tmp_path: Path) -> None:
    _make_workspace_skill(tmp_path)
    workspace = WorkspaceTreeService(root=tmp_path)
    tool = ReadSkillTool(workspace=workspace)
    ctx = ToolExecutionContext(session_id="s1")

    result = asyncio.run(tool.run({"skill": "demo"}, ctx))
    assert "## Steps" in result.text
    assert result.output["skill"] == "demo"
    assert result.output["truncated"] is False


def test_read_skill_tool_unknown_raises(tmp_path: Path) -> None:
    _make_workspace_skill(tmp_path)
    workspace = WorkspaceTreeService(root=tmp_path)
    tool = ReadSkillTool(workspace=workspace)
    ctx = ToolExecutionContext(session_id="s1")

    with pytest.raises(ToolExecutionError) as excinfo:
        asyncio.run(tool.run({"skill": "nope"}, ctx))
    assert excinfo.value.code == "TOOL_NOT_FOUND"


def test_register_skill_tools_and_permission() -> None:
    workspace = WorkspaceTreeService()
    registry = WorkspaceToolRegistry()
    register_skill_tools(registry, workspace=workspace)
    assert "read_skill" in registry.names()
    assert "read_skill" in READ_ONLY_TOOLS


def test_reminder_mentions_read_skill() -> None:
    reminder = format_skills_system_reminder([("demo", "demo summary")])
    assert "read_skill" in reminder
    assert "demo" in reminder
    assert format_skills_system_reminder([]) == ""


def test_rest_list_and_read_skill(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import src.agent_debug.domain.workspace_tree as wt

    monkeypatch.setattr(wt, "_persisted_root_file", lambda: tmp_path / ".agent_workspace_root")
    _make_workspace_skill(tmp_path)

    gateway = AgentDebugRestGateway()
    gateway.workspace_tree.set_root(tmp_path)

    listing = gateway.list_skills()
    names = [item["name"] for item in listing["items"]]
    assert "demo" in names

    content = gateway.read_skill_content("demo")
    assert "## Steps" in content["skill"]["content"]

    missing = gateway.read_skill_content("nope")
    assert missing["error"]["code"] == "SKILL_NOT_FOUND"
