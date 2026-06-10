"""Web search / fetch agent tools backed by :class:`WebSearchService`.

重建说明：原文件随 ``backend/`` 目录意外丢失。导出面按
``tools/__init__.py`` 重建：``WebSearchTool`` / ``WebFetchTool`` /
``register_web_tools``。工具名 ``web_search`` / ``web_fetch`` 与
``rest_gateway`` 的 allowed-tools 过滤逻辑保持一致。
"""

from __future__ import annotations

import json
from typing import Any, Dict

from src.agent_debug.domain.tools.base import (
    ToolExecutionContext,
    ToolExecutionError,
    ToolResult,
    WorkspaceToolRegistry,
)
from src.agent_debug.domain.web_search_service import WebSearchError, WebSearchService

WEB_SEARCH_TOOL_NAME = "web_search"
WEB_FETCH_TOOL_NAME = "web_fetch"


class WebSearchTool:
    name = WEB_SEARCH_TOOL_NAME
    description = (
        "Search the web for real-time information. Returns summarized results "
        "with titles, URLs and content snippets."
    )
    parameters: Dict[str, Any] = {
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "The search query."},
            "max_results": {
                "type": "integer",
                "description": "Maximum number of results (1-10, default 5).",
            },
        },
        "required": ["query"],
    }

    def __init__(self, service: WebSearchService) -> None:
        self._service = service

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        query = str(args.get("query") or "").strip()
        if not query:
            raise ToolExecutionError("TOOL_INVALID_ARGS", "web_search 需要非空 query")
        try:
            max_results = int(args.get("max_results") or 5)
        except (TypeError, ValueError):
            max_results = 5
        try:
            data = await self._service.search(query, max_results=max_results)
        except WebSearchError as exc:
            raise ToolExecutionError("TOOL_FAILED", str(exc)) from exc
        lines = []
        if data.get("answer"):
            lines.append(f"Answer: {data['answer']}")
        for item in data.get("results") or []:
            lines.append(f"- {item.get('title')} ({item.get('url')})\n  {item.get('content', '')[:300]}")
        text = "\n".join(lines) or "（无结果）"
        return ToolResult(output=data, text=text)


class WebFetchTool:
    name = WEB_FETCH_TOOL_NAME
    description = "Fetch the readable content of a web page by URL."
    parameters: Dict[str, Any] = {
        "type": "object",
        "properties": {
            "url": {"type": "string", "description": "The URL to fetch."},
        },
        "required": ["url"],
    }

    def __init__(self, service: WebSearchService) -> None:
        self._service = service

    async def run(self, args: Dict[str, Any], ctx: ToolExecutionContext) -> ToolResult:
        url = str(args.get("url") or "").strip()
        if not url.startswith(("http://", "https://")):
            raise ToolExecutionError("TOOL_INVALID_ARGS", "web_fetch 需要合法的 http(s) URL")
        try:
            data = await self._service.extract(url)
        except WebSearchError as exc:
            raise ToolExecutionError("TOOL_FAILED", str(exc)) from exc
        content = str(data.get("content") or "")
        text = content[:8000] if content else "（页面无可读内容）"
        return ToolResult(output={"url": url, "length": len(content)}, text=text)


def register_web_tools(
    registry: WorkspaceToolRegistry,
    *,
    service: WebSearchService,
) -> WorkspaceToolRegistry:
    registry.register(WebSearchTool(service))
    registry.register(WebFetchTool(service))
    return registry
