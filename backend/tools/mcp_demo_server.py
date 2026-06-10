"""In-repo stdio MCP demo server.

A tiny Model Context Protocol server exposing a few deterministic demo tools so
the agent runtime (and Settings → Tools panel) can exercise the full MCP path:
list tools, call a tool, render the structured result.

Run standalone for debugging:

    python backend/tools/mcp_demo_server.py

It speaks MCP over stdio and is launched as a subprocess by
``src/agent_debug/mcp/demo_service.py``.

NOTE: do not add ``from __future__ import annotations`` here — FastMCP
introspects real parameter annotation classes, and stringized annotations
break its tool registration.
"""

from mcp.server.fastmcp import FastMCP

mcp = FastMCP("agent-debug-demo")


@mcp.tool()
def add(a: float, b: float) -> str:
    """Add two numbers and return the sum."""
    total = a + b
    # Render integers without trailing .0 for nicer display.
    if total == int(total):
        total = int(total)
    return f"{a} + {b} = {total}"


@mcp.tool()
def echo(text: str) -> str:
    """Echo the provided text back to the caller."""
    return text


@mcp.tool()
def reverse(text: str) -> str:
    """Reverse the provided text (useful for verifying tool round-trips)."""
    return text[::-1]


if __name__ == "__main__":
    mcp.run()
