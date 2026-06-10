"""Expose MCP demo-server tools as agent tools.

Bridges the stdio MCP demo (``src/agent_debug/mcp/demo_service.py``) into the
agent ReAct loop so MCP calls flow through ``_dispatch_tool`` and emit the same
``agent.tool.*`` events as built-in tools. Tools are named ``mcp__demo__<tool>``
(mirroring Proma's ``mcp__<server>__<tool>`` convention) so the frontend can
render them with the dedicated MCP (Plug) styling.

Registration is best-effort: when the optional ``mcp`` dependency or the demo
server script is unavailable, ``register_mcp_demo_tools`` silently registers
nothing instead of breaking startup.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Any, Dict

from src.agent_debug.domain.tools.base import (
    AgentTool,
    ToolExecutionContext,
    ToolExecutionError,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.mcp import demo_service

logger = logging.getLogger(__name__)

MCP_DEMO_SERVER = "demo"
_MCP_TOOL_PREFIX = f"mcp__{MCP_DEMO_SERVER}__"


def _open_object_schema() -> Dict[str, Any]:
    return {"type": "object", "additionalProperties": True, "properties": {}}


@dataclass
class McpDemoTool(AgentTool):
    """Single MCP demo-server tool wrapped as an :class:`AgentTool`."""

    remote_name: str
    description: str = ""
    parameters: Dict[str, Any] = field(default_factory=_open_object_schema)

    @property
    def name(self) -> str:  # type: ignore[override]
        return f"{_MCP_TOOL_PREFIX}{self.remote_name}"

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        result = await demo_service.mcp_demo_call(self.remote_name, dict(args or {}))
        if not isinstance(result, dict) or not result.get("ok"):
            error = (result or {}).get("error", {}) if isinstance(result, dict) else {}
            code = str(error.get("code") or "MCP_TOOL_ERROR")
            message = str(error.get("message") or "MCP tool call failed")
            raise ToolExecutionError(code, message)

        payload = result.get("result") or {}
        parts = payload.get("content") if isinstance(payload, dict) else None
        text_chunks = []
        if isinstance(parts, list):
            for block in parts:
                if isinstance(block, dict) and block.get("type") == "text":
                    text_chunks.append(str(block.get("text") or ""))
        text = "\n".join(chunk for chunk in text_chunks if chunk)
        if not text and isinstance(payload, dict) and payload.get("structured") is not None:
            text = str(payload.get("structured"))
        return ToolResult(output=payload, text=text)


def register_mcp_demo_tools(registry: WorkspaceToolRegistry) -> int:
    """Register every available MCP demo tool. Returns the number registered.

    Safe to call at startup: returns 0 (and registers nothing) when MCP is not
    installed or the demo server script is missing.
    """
    try:
        status = demo_service.mcp_demo_status_sync()
    except Exception as exc:  # pragma: no cover - defensive against stdio/platform
        logger.debug("MCP demo status failed, skipping MCP tool registration: %s", exc)
        return 0

    if not isinstance(status, dict) or not status.get("ok"):
        return 0

    registered = 0
    for tool in status.get("tools", []) or []:
        if not isinstance(tool, dict):
            continue
        remote_name = str(tool.get("name") or "").strip()
        if not remote_name:
            continue
        schema = tool.get("inputSchema")
        parameters = schema if isinstance(schema, dict) and schema else _open_object_schema()
        registry.register(
            McpDemoTool(
                remote_name=remote_name,
                description=str(tool.get("description") or f"MCP demo tool: {remote_name}"),
                parameters=parameters,
            )
        )
        registered += 1
    if registered:
        logger.info("Registered %d MCP demo tool(s)", registered)
    return registered
