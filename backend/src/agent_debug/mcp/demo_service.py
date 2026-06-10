"""Connect to the in-repo stdio MCP demo (tools/mcp_demo_server.py) using the official SDK."""

from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path
from typing import Any, Awaitable, Callable, Dict, List

# backend/src/agent_debug/mcp/demo_service.py -> parents[3] == backend
_BACKEND_ROOT = Path(__file__).resolve().parents[3]
_MCP_DEMO_SERVER = _BACKEND_ROOT / "tools" / "mcp_demo_server.py"


def _mcp_unavailable() -> Dict[str, Any]:
    return {
        "ok": False,
        "error": {
            "code": "MCP_NOT_INSTALLED",
            "message": "Install optional dependency: pip install -e .[agent-mcp]",
        },
    }


def _server_missing() -> Dict[str, Any]:
    return {
        "ok": False,
        "error": {
            "code": "MCP_DEMO_SERVER_MISSING",
            "message": f"Demo server script not found: {_MCP_DEMO_SERVER}",
        },
    }


def _mcp_error(message: str, code: str = "MCP_ERROR") -> Dict[str, Any]:
    return {"ok": False, "error": {"code": code, "message": message}}


async def mcp_demo_status() -> Dict[str, Any]:
    """List tools from the demo MCP server (B1)."""
    try:
        from mcp.shared.exceptions import McpError
    except ImportError:
        return _mcp_unavailable()

    if not _MCP_DEMO_SERVER.is_file():
        return _server_missing()

    try:
        tools = await _with_demo_session(_list_tools_session_impl)
    except McpError as exc:  # type: ignore[misc]
        return _mcp_error(str(exc), "MCP_PROTOCOL_ERROR")
    except Exception as exc:  # pragma: no cover - stdio / platform
        return _mcp_error(str(exc), "MCP_DEMO_FAILED")

    return {
        "ok": True,
        "tools": tools,
    }


async def _list_tools_session_impl(session: Any) -> List[Dict[str, Any]]:
    from mcp import ClientSession

    assert isinstance(session, ClientSession)
    await session.initialize()
    out = await session.list_tools()
    return [
        {
            "name": t.name,
            "description": getattr(t, "description", None) or "",
            "inputSchema": getattr(t, "inputSchema", None) or {},
        }
        for t in out.tools
    ]


async def mcp_demo_call(name: str, arguments: Dict[str, Any] | None) -> Dict[str, Any]:
    """Call a tool on the demo MCP server (B2, B3)."""
    try:
        from mcp.shared.exceptions import McpError
    except ImportError:
        return _mcp_unavailable()

    if not _MCP_DEMO_SERVER.is_file():
        return _server_missing()

    if not name or not str(name).strip():
        return _mcp_error("tool name is required", "MCP_INVALID_REQUEST")

    try:

        async def _do_call(session: Any) -> Dict[str, Any]:
            return await _call_tool_session(
                session, str(name).strip(), dict(arguments or {})
            )

        return await _with_demo_session(_do_call)
    except McpError as exc:  # type: ignore[misc]
        # e.g. unknown tool
        return {
            "ok": False,
            "error": {"code": "MCP_TOOL_ERROR", "message": str(exc)},
        }
    except Exception as exc:
        return _mcp_error(str(exc), "MCP_DEMO_FAILED")


async def _call_tool_session(session: Any, name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
    from mcp import ClientSession

    assert isinstance(session, ClientSession)
    await session.initialize()
    result = await session.call_tool(name, arguments)
    # flatten text / structured for JSON response
    parts: List[Dict[str, Any]] = []
    for block in result.content:
        btype = type(block).__name__
        if btype == "TextContent" and hasattr(block, "text"):
            parts.append({"type": "text", "text": block.text})
        else:
            parts.append({"type": btype, "repr": str(block)})
    if result.isError:
        msg = parts[0].get("text", "tool error") if parts else "tool error"
        return {
            "ok": False,
            "error": {"code": "MCP_TOOL_ERROR", "message": msg},
        }
    payload: Dict[str, Any] = {
        "ok": True,
        "result": {"content": parts},
    }
    if result.structuredContent is not None:
        payload["result"]["structured"] = result.structuredContent
    return payload


def _stdio_params() -> Any:
    from mcp import StdioServerParameters

    return StdioServerParameters(
        command=sys.executable,
        args=[str(_MCP_DEMO_SERVER)],
        env={**os.environ, "PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8"},
    )


async def _with_demo_session(
    work: Callable[[Any], Awaitable[Any]],
) -> Any:
    from mcp import ClientSession
    from mcp.client.stdio import stdio_client

    params = _stdio_params()
    async with stdio_client(params) as streams:
        read, write = streams
        async with ClientSession(read, write) as session:
            return await work(session)


def mcp_demo_status_sync() -> Dict[str, Any]:
    return asyncio.run(mcp_demo_status())


def mcp_demo_call_sync(name: str, arguments: Dict[str, Any] | None) -> Dict[str, Any]:
    return asyncio.run(mcp_demo_call(name, arguments))
