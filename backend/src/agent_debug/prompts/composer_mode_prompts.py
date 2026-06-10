"""Composer chat (build / debug / ask) and plan-generation system prompts.

Adds :class:`ComposerModeProfile` that captures per-mode runtime settings
(timeout, temperature, tool allowlist) so the gateway can dispatch the right
flavour of request without scattering ``if mode ==`` branches.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, FrozenSet, List, Tuple

from src.agent_debug.prompts.builtin_subagents import render_subagents_prompt
from src.agent_debug.prompts.system_prompt_assembly import (
    assemble_composer_system_message,
    static_block_for_mode,
)

# --- Plan mode: JSON plan structure (used by PlanEngine, not chat composer) ---

PLAN_GENERATION_SYSTEM = (
    "You generate execution plans for an agent-debug platform. "
    "Return JSON only with keys: objective, assumptions, risks, openQuestions, stages. "
    "Each stage must include title, summary, riskFlags, tasks. "
    "Each task must include title, description, priority, parallelism, ownerType, estimatedCost, steps. "
    "Each step must include title and actionType."
)

# Backward-compatible aliases: full static block (persona + mode) per key.
SYSTEM_BUILD = static_block_for_mode("build")
SYSTEM_DEBUG = static_block_for_mode("debug")
SYSTEM_ASK = static_block_for_mode("ask")

_CHAT_INTROS: Dict[str, str] = {
    "build": SYSTEM_BUILD,
    "debug": SYSTEM_DEBUG,
    "ask": SYSTEM_ASK,
}


@dataclass(frozen=True)
class ComposerModeProfile:
    """Runtime knobs derived from the composer mode.

    ``allowed_tools`` is a *set* of tool names that the gateway should expose
    when constructing the OpenAI ``tools=`` payload. Empty means "no tools"
    (e.g. Ask mode prefers explanation over actions).
    """

    mode: str
    timeout_ms: int
    temperature: float
    allowed_tools: FrozenSet[str] = field(default_factory=frozenset)
    label: str = ""


_ACTION_MODE_TOOLS = frozenset(
    {
        "read_file",
        "list_dir",
        "grep",
        "web_search",
        "web_fetch",
        "write_file",
        "create_document",
        "delete_file",
        "run_command",
        "check_command_status",
        "stop_command",
        "write_todos",
        "Task",
    }
)


_BUILD_PROFILE = ComposerModeProfile(
    mode="build",
    timeout_ms=60_000,
    temperature=0.2,
    allowed_tools=_ACTION_MODE_TOOLS,
    label="Build",
)
_DEBUG_PROFILE = ComposerModeProfile(
    mode="debug",
    timeout_ms=90_000,
    temperature=0.1,
    allowed_tools=_ACTION_MODE_TOOLS,
    label="Debug",
)
_ASK_PROFILE = ComposerModeProfile(
    mode="ask",
    timeout_ms=45_000,
    temperature=0.3,
    allowed_tools=frozenset(),
    label="Ask",
)
_MULTITASK_PROFILE = ComposerModeProfile(
    mode="multitask",
    timeout_ms=120_000,
    temperature=0.2,
    allowed_tools=_ACTION_MODE_TOOLS,
    label="Multitask",
)
_PLAN_PROFILE = ComposerModeProfile(
    mode="plan",
    timeout_ms=120_000,
    temperature=0.2,
    allowed_tools=_ACTION_MODE_TOOLS,
    label="Plan",
)

# Appended to the chat system message for action-oriented modes so the agent
# proactively maintains a todo list via the ``write_todos`` tool.
_TODO_TOOL_GUIDANCE = (
    "\n\n## Todo planning\n"
    "For any non-trivial task that takes 3 or more steps, call the `write_todos` "
    "tool FIRST to lay out the full todo list, then call it again after each step "
    "to update progress. Always pass the COMPLETE list (full replace) and reuse a "
    "stable `id` per item so updates map to the same todo. Keep exactly one item "
    "`in_progress` at a time, and mark items `completed` as soon as they are done. "
    "Skip this only for trivial single-step requests."
)


def _build_subagent_guidance() -> str:
    """Guidance + roster so the agent knows it can delegate via the Task tool."""
    return (
        "\n\n## 委派子代理（Task）\n"
        "当一个任务可以拆成相互独立的子任务（例如同时探索多个模块、并行调研多个方向）时，"
        "使用 `Task` 工具委派子代理。你可以在**同一轮**里发出多个 `Task` 调用，它们会**并行**执行。\n"
        "- 每个 `Task` 必须给出简短的 `description`（卡片标题）与详尽的 `prompt`（子代理看不到当前对话，需自包含）。\n"
        "- 用 `subagent_type` 选择内置画像，或用 `system_prompt` 定义临时子代理；只读任务可设 `readonly: true`。\n"
        "- 子代理不能再委派（无嵌套 Task）。委派后请综合各子代理返回的摘要继续推进。\n\n"
        + render_subagents_prompt()
    )


_SUBAGENT_GUIDANCE = _build_subagent_guidance()


_PROFILES: Dict[str, ComposerModeProfile] = {
    "build": _BUILD_PROFILE,
    "debug": _DEBUG_PROFILE,
    "ask": _ASK_PROFILE,
    "multitask": _MULTITASK_PROFILE,
    "plan": _PLAN_PROFILE,
}


def normalize_composer_chat_mode(mode: str | None) -> str:
    """Map composer mode to build/debug/ask; default build.

    ``plan`` and ``multitask`` are *runtime* modes (not chat intro flavours);
    they fall back to the build chat intro because they still produce
    natural-language replies in addition to the structured side-effects.
    """
    m = (mode or "build").strip().lower()
    if m in _CHAT_INTROS:
        return m
    return "build"


def normalize_composer_runtime_mode(mode: str | None) -> str:
    """Return the canonical runtime mode key (preserves plan/multitask)."""
    m = (mode or "build").strip().lower()
    if m in _PROFILES:
        return m
    return "build"


def resolve_chat_system_intro(mode: str | None) -> str:
    return _CHAT_INTROS[normalize_composer_chat_mode(mode)]


def resolve_composer_profile(mode: str | None) -> ComposerModeProfile:
    return _PROFILES[normalize_composer_runtime_mode(mode)]


def build_chat_system_content(
    mode: str | None,
    context_window: Dict[str, Any] | None = None,
    *,
    workspace_root: Any = None,
    agent_md_text: str | None = None,
    workspace_info: Dict[str, Any] | None = None,
    skill_items: List[Tuple[str, str]] | None = None,
) -> str:
    mode_key = normalize_composer_chat_mode(mode)
    message = assemble_composer_system_message(
        mode_key,
        context_window,
        workspace_root=workspace_root,
        agent_md_text=agent_md_text,
        workspace_info=workspace_info,
        skill_items=skill_items,
    )
    # Ask mode prefers explanation over actions, so we don't nudge it to plan
    # todos or delegate sub-agents; every action-oriented mode gets the
    # todo-planning + sub-agent delegation guidance.
    if mode_key != "ask":
        message = f"{message}{_TODO_TOOL_GUIDANCE}{_SUBAGENT_GUIDANCE}"
    return message


__all__ = [
    "ComposerModeProfile",
    "PLAN_GENERATION_SYSTEM",
    "SYSTEM_ASK",
    "SYSTEM_BUILD",
    "SYSTEM_DEBUG",
    "build_chat_system_content",
    "normalize_composer_chat_mode",
    "normalize_composer_runtime_mode",
    "resolve_chat_system_intro",
    "resolve_composer_profile",
]
