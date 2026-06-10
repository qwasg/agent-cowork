"""Skill tools: let the agent read full ``SKILL.md`` content on demand.

The system prompt only injects a short ``(name, summary)`` blurb per skill via
:func:`format_skills_system_reminder`. When a skill is relevant the model calls
``read_skill`` to pull the full ``SKILL.md`` body (capped) and then follows its
instructions. Discovery (including user-level skill homes outside the workspace)
is handled by :mod:`src.agent_debug.prompts.skills_discovery`, so we read the
resolved absolute path directly rather than going through the workspace
containment checks.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict

from src.agent_debug.domain.tools.base import (
    AgentTool,
    ToolExecutionContext,
    ToolExecutionError,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.workspace_tree import WorkspaceTreeService
from src.agent_debug.prompts.skills_discovery import (
    discover_skill_records,
    resolve_skill_path,
)

# Skills are documentation: keep generous but bounded so we never blow the
# context window with a runaway file.
_SKILL_CONTENT_CAP = 16_000


@dataclass
class ReadSkillTool(AgentTool):
    """Read the full ``SKILL.md`` for a discovered skill by name."""

    workspace: WorkspaceTreeService
    name: str = "read_skill"
    description: str = (
        "Read the full SKILL.md content for an available skill by name. "
        "Call this when a skill listed in the system reminder is relevant to the "
        "task, then follow the skill's instructions."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["skill"],
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name (directory name) to read.",
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        skill = args.get("skill")
        if not isinstance(skill, str) or not skill.strip():
            raise ToolExecutionError("TOOL_INVALID_ARGS", "missing argument: skill")
        skill = skill.strip()
        root = self.workspace.root
        path = resolve_skill_path(root, skill)
        if path is None:
            available = ", ".join(record.name for record in discover_skill_records(root))
            hint = f" Available skills: {available}." if available else ""
            raise ToolExecutionError(
                "TOOL_NOT_FOUND", f"unknown skill: {skill}.{hint}"
            )
        try:
            content = Path(path).read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            raise ToolExecutionError("FILESYSTEM_ERROR", str(exc))
        truncated = len(content) > _SKILL_CONTENT_CAP
        body = content[:_SKILL_CONTENT_CAP]
        if truncated:
            body += "\n\n…[truncated]"
        return ToolResult(
            output={
                "skill": skill,
                "path": path,
                "content": body,
                "truncated": truncated,
            },
            text=body,
        )


def register_skill_tools(
    registry: WorkspaceToolRegistry, *, workspace: WorkspaceTreeService
) -> None:
    """Register skill-related tools onto ``registry``."""
    registry.register(ReadSkillTool(workspace=workspace))
