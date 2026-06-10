"""Tool calling abstractions used by the agent runtime."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Dict, List, Optional, Protocol


class ToolNotFoundError(LookupError):
    """Raised when the runtime is asked to run a tool that is not registered."""


class ToolExecutionError(RuntimeError):
    """Raised by tools when they cannot complete the call.

    Carries a stable ``code`` for the structured agent event payload (e.g.
    ``TOOL_INVALID_ARGS`` / ``TOOL_FAILED`` / ``PATH_OUTSIDE_ROOT``).
    """

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


@dataclass
class ToolExecutionContext:
    """Runtime metadata passed to every tool invocation."""

    session_id: str
    run_id: Optional[str] = None
    subagent_id: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass
class ToolResult:
    """Structured tool result returned to the model and to event subscribers."""

    output: Any
    text: str = ""
    artifacts: List[Dict[str, Any]] = field(default_factory=list)


class AgentTool(Protocol):
    name: str
    description: str
    parameters: Dict[str, Any]

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult: ...


class WorkspaceToolRegistry:
    """In-memory registry mapping ``tool_name`` to :class:`AgentTool` instances."""

    def __init__(self) -> None:
        self._tools: Dict[str, AgentTool] = {}

    def register(self, tool: AgentTool) -> None:
        if not getattr(tool, "name", None):
            raise ValueError("Tool must declare a non-empty name")
        self._tools[tool.name] = tool

    def has(self, name: str) -> bool:
        return name in self._tools

    def get(self, name: str) -> AgentTool:
        try:
            return self._tools[name]
        except KeyError as exc:
            raise ToolNotFoundError(f"Tool not registered: {name}") from exc

    def names(self) -> List[str]:
        return list(self._tools.keys())

    def json_schemas(self, allowed: Optional[List[str]] = None) -> List[Dict[str, Any]]:
        """Render the registry as the OpenAI ``tools=`` payload.

        ``allowed`` (None = all) restricts the rendered set by tool name —
        used by sub-agent runs and per-session tool allowlists.
        """
        out: List[Dict[str, Any]] = []
        allowed_set = None if allowed is None else set(allowed)
        for tool in self._tools.values():
            if allowed_set is not None and tool.name not in allowed_set:
                continue
            out.append(
                {
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                }
            )
        return out

    async def run(
        self, name: str, args: Dict[str, Any], ctx: ToolExecutionContext
    ) -> ToolResult:
        tool = self.get(name)
        return await tool.run(args, ctx)


# Lightweight functional tool helper for tests / inline registrations.
ToolFn = Callable[[Dict[str, Any], ToolExecutionContext], Awaitable[ToolResult]]


@dataclass
class FunctionTool:
    name: str
    description: str
    parameters: Dict[str, Any]
    fn: ToolFn

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        return await self.fn(args, ctx)
