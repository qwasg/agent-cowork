"""Agent-authored todo list tool (TodoWrite-style).

The tool lets the conversational agent *plan its own work* by emitting a full
todo list during the ReAct loop. Unlike :func:`TodoEngine.materialize_from_tasks`
(which is driven by plan execution), this tool is model-driven: the agent calls
``write_todos`` with the complete list each time, mirroring the Cursor / Claude
``TodoWrite`` contract so state converges deterministically.

Execution is intercepted by :class:`AgentRuntimeService._dispatch_tool` because
the actual write needs the runtime-owned :class:`TodoEngine` and event bus to
publish ``todo.*`` events. The ``run`` method here is only a defensive fallback
in case the interception is ever removed; it never mutates state.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict

from src.agent_debug.domain.tools.base import (
    AgentTool,
    ToolExecutionContext,
    ToolResult,
)


WRITE_TODOS_TOOL_NAME = "write_todos"


@dataclass
class TodoWriteTool(AgentTool):
    name: str = WRITE_TODOS_TOOL_NAME
    description: str = (
        "Create or update the todo list for the current task. Pass the COMPLETE "
        "list every time (full replace semantics): keep already-known items with "
        "their ids and only change their status. Use this at the start of any "
        "non-trivial multi-step task to lay out the plan, then call it again to "
        "update progress. Keep exactly one item 'in_progress' at a time."
    )

    @property
    def parameters(self) -> Dict[str, Any]:
        return {
            "type": "object",
            "additionalProperties": False,
            "required": ["todos"],
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The full ordered todo list for the task.",
                    "items": {
                        "type": "object",
                        "additionalProperties": False,
                        "required": ["content", "status"],
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": (
                                    "Stable client id you assign and reuse across "
                                    "calls so status updates map to the same todo."
                                ),
                            },
                            "content": {
                                "type": "string",
                                "description": "Short imperative description of the todo.",
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status of this todo.",
                            },
                            "note": {
                                "type": "string",
                                "description": "Optional extra detail / summary for the todo.",
                            },
                        },
                    },
                },
            },
        }

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        # Defensive fallback only — the runtime intercepts this tool by name and
        # performs the real write + event publishing. We never reach here in the
        # normal flow.
        todos = args.get("todos") if isinstance(args, dict) else None
        count = len(todos) if isinstance(todos, list) else 0
        return ToolResult(
            output={"accepted": False, "count": count},
            text="write_todos was not handled by the runtime (no-op fallback).",
        )
