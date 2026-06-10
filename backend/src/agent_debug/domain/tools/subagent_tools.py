"""Sub-agent delegation tool (Task-style).

The ``Task`` tool lets the main agent delegate a self-contained piece of work to
a child sub-agent. The model may either pick a built-in sub-agent profile via
``subagent_type`` (explorer / researcher / code-reviewer / general) or spin up an
ad-hoc sub-agent by supplying a custom ``system_prompt``.

Like ``write_todos``, execution is intercepted by
:meth:`AgentRuntimeService._dispatch_tool`: the actual sub-agent run needs the
runtime-owned ReAct loop, sub-agent orchestrator and event bus to spawn a nested
loop (with a restricted tool allowlist) and publish ``subagent.*`` events. The
``run`` method here is only a defensive no-op fallback.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List

from src.agent_debug.domain.tools.base import (
    AgentTool,
    ToolExecutionContext,
    ToolResult,
)
from src.agent_debug.prompts.builtin_subagents import BUILTIN_SUBAGENTS


TASK_TOOL_NAME = "Task"


def _subagent_type_names() -> List[str]:
    return [profile.name for profile in BUILTIN_SUBAGENTS]


@dataclass
class TaskTool(AgentTool):
    name: str = TASK_TOOL_NAME
    description: str = (
        "Delegate a self-contained task to a child sub-agent that runs its own "
        "tool-using loop and returns a summary. Use this to parallelize research "
        "or exploration: you can emit MULTIPLE Task calls in a single turn and "
        "they run concurrently. Pick a built-in 'subagent_type' or provide a "
        "custom 'system_prompt' for an ad-hoc sub-agent. Always give a highly "
        "detailed 'prompt' because the sub-agent does NOT see this conversation. "
        "Sub-agents cannot delegate further (no nested Task calls)."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["description", "prompt"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": (
                        "Short (3-6 word) title for this sub-agent task, shown as "
                        "the card label, e.g. 'Explore backend agent & tools'."
                    ),
                },
                "prompt": {
                    "type": "string",
                    "description": (
                        "Detailed, self-contained instructions for the sub-agent. "
                        "Include all context it needs; it cannot see the parent "
                        "conversation."
                    ),
                },
                "subagent_type": {
                    "type": "string",
                    "enum": _subagent_type_names(),
                    "description": (
                        "Which built-in sub-agent profile to use. Determines the "
                        "system prompt and the allowed tool set."
                    ),
                },
                "system_prompt": {
                    "type": "string",
                    "description": (
                        "Optional custom system prompt for an ad-hoc sub-agent. "
                        "If provided it overrides 'subagent_type'."
                    ),
                },
                "readonly": {
                    "type": "boolean",
                    "description": (
                        "If true, restrict the ad-hoc sub-agent to read-only tools. "
                        "Defaults to true for custom sub-agents."
                    ),
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        # Defensive fallback only — the runtime intercepts this tool by name and
        # spawns the real nested sub-agent loop. We never reach here normally.
        description = args.get("description") if isinstance(args, dict) else None
        return ToolResult(
            output={"accepted": False, "description": description},
            text="Task was not handled by the runtime (no-op fallback).",
        )
