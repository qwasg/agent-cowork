"""Agent tool registry and built-in workspace tools.

Exposed entry points:

- ``AgentTool`` — abstract base with ``name``, ``description``, JSON-Schema-shaped
  ``parameters`` and an async ``run(args, ctx)``.
- ``WorkspaceToolRegistry`` — lookup + ``json_schemas()`` shaped for the OpenAI
  ``tools=`` parameter.
- ``build_default_workspace_tools(workspace_tree)`` — registers
- ``read_file`` / ``list_dir`` / ``grep`` / ``write_file`` / ``create_document``
  against a :class:`WorkspaceTreeService`.
- ``ToolExecutionError`` / ``ToolNotFoundError`` — surfaced through the runtime
  layer with structured codes (``TOOL_INVALID_ARGS`` / ``TOOL_FAILED`` /
  ``TOOL_NOT_FOUND``).
"""

from src.agent_debug.domain.tools.base import (
    AgentTool,
    ToolExecutionContext,
    ToolExecutionError,
    ToolNotFoundError,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.tools.mcp_tools import (
    McpDemoTool,
    register_mcp_demo_tools,
)
from src.agent_debug.domain.tools.skill_tools import (
    ReadSkillTool,
    register_skill_tools,
)
from src.agent_debug.domain.tools.subagent_tools import (
    TASK_TOOL_NAME,
    TaskTool,
)
from src.agent_debug.domain.tools.todo_tools import (
    WRITE_TODOS_TOOL_NAME,
    TodoWriteTool,
)
from src.agent_debug.domain.tools.workspace_tools import (
    CreateDocumentTool,
    GrepTool,
    ListDirTool,
    ReadFileTool,
    WriteFileTool,
    build_default_workspace_tools,
)
from src.agent_debug.domain.tools.web_tools import (
    WebFetchTool,
    WebSearchTool,
    register_web_tools,
)

__all__ = [
    "AgentTool",
    "CreateDocumentTool",
    "GrepTool",
    "ListDirTool",
    "McpDemoTool",
    "ReadFileTool",
    "ReadSkillTool",
    "TaskTool",
    "TASK_TOOL_NAME",
    "TodoWriteTool",
    "WRITE_TODOS_TOOL_NAME",
    "ToolExecutionContext",
    "ToolExecutionError",
    "ToolNotFoundError",
    "ToolResult",
    "WebFetchTool",
    "WebSearchTool",
    "WriteFileTool",
    "WorkspaceToolRegistry",
    "build_default_workspace_tools",
    "register_skill_tools",
    "register_web_tools",
    "register_mcp_demo_tools",
]
