"""Agent Debug LLM prompt templates (Composer modes, plan generation, etc.)."""

from src.agent_debug.prompts.composer_mode_prompts import (
    ComposerModeProfile,
    PLAN_GENERATION_SYSTEM,
    build_chat_system_content,
    normalize_composer_chat_mode,
    normalize_composer_runtime_mode,
    resolve_chat_system_intro,
    resolve_composer_profile,
)
from src.agent_debug.prompts.builtin_subagents import (
    BUILTIN_SUBAGENTS,
    DEFAULT_READONLY_TOOLS,
    DEFAULT_WRITE_TOOLS,
    SubagentProfile,
    get_subagent,
    render_subagents_prompt,
    subagents_as_dicts,
)
from src.agent_debug.prompts.skills_discovery import (
    SkillRecord,
    discover_skill_records,
    discover_workspace_skills,
    resolve_skill_path,
)
from src.agent_debug.prompts.system_prompt_assembly import (
    assemble_composer_system_message,
)

__all__ = [
    "BUILTIN_SUBAGENTS",
    "DEFAULT_READONLY_TOOLS",
    "DEFAULT_WRITE_TOOLS",
    "ComposerModeProfile",
    "PLAN_GENERATION_SYSTEM",
    "SkillRecord",
    "SubagentProfile",
    "assemble_composer_system_message",
    "build_chat_system_content",
    "discover_skill_records",
    "discover_workspace_skills",
    "resolve_skill_path",
    "get_subagent",
    "normalize_composer_chat_mode",
    "normalize_composer_runtime_mode",
    "render_subagents_prompt",
    "resolve_chat_system_intro",
    "resolve_composer_profile",
    "subagents_as_dicts",
]
