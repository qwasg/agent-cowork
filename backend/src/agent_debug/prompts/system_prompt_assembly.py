"""Layered system prompt assembly (STATIC / DYNAMIC) aligned with easy-agent shape."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Final, Mapping

from src.agent_debug.prompts.skills_reminder import format_skills_system_reminder

# Log / snapshot boundaries (align with easy-agent static vs dynamic).
SYSTEM_STATIC_CONTEXT_START: Final = "<SYSTEM_STATIC_CONTEXT>"
SYSTEM_STATIC_CONTEXT_END: Final = "</SYSTEM_STATIC_CONTEXT>"
SYSTEM_DYNAMIC_CONTEXT_START: Final = "<SYSTEM_DYNAMIC_CONTEXT>"
SYSTEM_DYNAMIC_CONTEXT_END: Final = "</SYSTEM_DYNAMIC_CONTEXT>"

# Stated once in static block (not repeated per mode).
_SYSTEM_DIRECT_ANSWER = (
    "Be direct: answer the substance first. Do not preface with meta-commentary about how you will "
    "answer, your role, or output format; avoid filler, redundant disclaimers, and long warm-ups "
    "unless the user explicitly asks for them."
)

# IDE / Agent Debug–worded static persona (inspired by easy-agent static sections, not terminal tool names).
STATIC_PERSONA = (
    "You are a coding assistant in the Agent Debug IDE, working against the configured workspace. "
    "Prefer reading and reasoning from real project files, diffs, and logs over generic advice. "
    "When changing code, preserve existing behavior unless the user explicitly asks otherwise. "
    "If something fails, state the failure briefly and suggest the next concrete step.\n"
    f"{_SYSTEM_DIRECT_ANSWER}"
)

# Short mode overlays — "Build mode" / "Debug mode" / "Ask mode" keep test and API expectations.
_MODE_BUILD = (
    "**Build mode**: Prioritize implementation, refactors, and code iteration. "
    "Do not output multi-step project plan JSON or a full execution plan unless the user explicitly "
    "asks for a plan."
)
_MODE_DEBUG = (
    "**Debug mode**: Prioritize reproduction, likely root cause, logs, stack traces, and config checks. "
    "Suggest minimal, testable hypotheses. Avoid product roadmaps or large design documents unless asked."
)
_MODE_ASK = (
    "**Ask mode**: Prioritize concepts, comparison, and clear explanations. "
    "Do not produce long implementation checklists or full project plans unless the user requests them."
)

MODE_OVERLAYS: Final[Dict[str, str]] = {
    "build": _MODE_BUILD,
    "debug": _MODE_DEBUG,
    "ask": _MODE_ASK,
}

_CONTEXT_TITLE = (
    "## Current workspace context (for reference; do not repeat verbatim to the user)"
)


# 中国大模型特化指引：CJK 写入拆分、语言偏好、推理透传等。
CHINA_AI_GUIDANCE = (
    "When the user's message is primarily in Chinese, reply in Chinese. "
    "For very large file writes containing CJK text (roughly >10k characters), split the write "
    "into multiple smaller edits to avoid provider truncation. "
    "If the model returns reasoning/thinking content, treat it as internal: do not echo it verbatim "
    "to the user unless asked."
)


def static_block_for_mode(mode_key: str) -> str:
    """Content inside SYSTEM_STATIC only (persona + mode overlay). ``mode_key`` is build|debug|ask."""
    overlay = MODE_OVERLAYS.get(mode_key, MODE_OVERLAYS["build"])
    return f"{STATIC_PERSONA}\n\n{overlay}\n\n{CHINA_AI_GUIDANCE}"


def _format_environment_block(
    workspace_root: Any,
    workspace_info: Mapping[str, Any] | None,
) -> str:
    lines: list[str] = []
    if workspace_root is not None:
        try:
            p = Path(workspace_root).resolve()
        except OSError:
            p = None
        if p is not None:
            lines.append(f"- **Workspace root**: `{p}`")
    if workspace_info and isinstance(workspace_info, Mapping):
        branch = workspace_info.get("branch")
        if branch:
            lines.append(f"- **Branch**: {branch}")
    if not lines:
        return ""
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S")
    lines.append(f"- **UTC time**: {now} UTC")
    return "#### Environment\n" + "\n".join(lines)


def _format_project_memory_block(agent_md_text: str | None) -> str:
    if not (agent_md_text and agent_md_text.strip()):
        return ""
    body = agent_md_text.strip()
    return f"#### Project memory (AGENT.md)\n```markdown\n{body}\n```"


def _format_context_window_block(context_window: Dict[str, Any] | None) -> str:
    if not isinstance(context_window, dict) or not context_window:
        return ""
    try:
        extra = json.dumps(context_window, ensure_ascii=False)[:4000]
    except (TypeError, ValueError):
        return ""
    return f"{_CONTEXT_TITLE}\n```json\n{extra}\n```"


def assemble_composer_system_message(
    mode_key: str,
    context_window: Dict[str, Any] | None = None,
    *,
    workspace_root: Any = None,
    agent_md_text: str | None = None,
    workspace_info: Mapping[str, Any] | None = None,
    skill_items: list[tuple[str, str]] | None = None,
) -> str:
    """Build full system message: STATIC (tagged) + DYNAMIC (tagged), easy-agent–like ordering."""
    static_inner = static_block_for_mode(mode_key)
    static = (
        f"{SYSTEM_STATIC_CONTEXT_START}\n{static_inner}\n{SYSTEM_STATIC_CONTEXT_END}"
    )

    dynamic_parts: list[str] = []
    env = _format_environment_block(workspace_root, workspace_info)
    if env:
        dynamic_parts.append(env)
    mem = _format_project_memory_block(agent_md_text)
    if mem:
        dynamic_parts.append(mem)
    ctx = _format_context_window_block(context_window)
    if ctx:
        dynamic_parts.append(ctx)
    skills = format_skills_system_reminder(skill_items)
    if skills:
        dynamic_parts.append(skills)

    if not dynamic_parts:
        return static
    dynamic_body = "\n\n".join(dynamic_parts)
    dynamic = f"{SYSTEM_DYNAMIC_CONTEXT_START}\n{dynamic_body}\n{SYSTEM_DYNAMIC_CONTEXT_END}"
    return f"{static}\n\n{dynamic}"


SYSTEM_DIRECT_ANSWER = _SYSTEM_DIRECT_ANSWER  # public alias

__all__ = [
    "MODE_OVERLAYS",
    "STATIC_PERSONA",
    "SYSTEM_DIRECT_ANSWER",
    "SYSTEM_DYNAMIC_CONTEXT_END",
    "SYSTEM_DYNAMIC_CONTEXT_START",
    "SYSTEM_STATIC_CONTEXT_END",
    "SYSTEM_STATIC_CONTEXT_START",
    "assemble_composer_system_message",
    "static_block_for_mode",
]
