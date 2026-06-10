"""Tavily-backed web search / URL extraction service.

重建说明：原文件随 ``backend/`` 目录意外丢失。接口按调用面重建：
``rest_gateway`` 以 ``WebSearchService(config_resolver=...)`` 构造并交给
``register_web_tools``；工具层（``web_tools.py``）调用 ``search`` /
``extract``。配置（API key、topic、search_depth 等）每次调用时通过
``config_resolver()`` 实时解析，保证设置页改动即时生效。
"""

from __future__ import annotations

from typing import Any, Callable, Dict, List

import httpx

from src.agent_debug.domain.search_config import SearchApiConfig

_TAVILY_BASE = "https://api.tavily.com"
_TIMEOUT = httpx.Timeout(30.0, connect=10.0)


class WebSearchError(RuntimeError):
    """Raised when search is disabled/misconfigured or the upstream fails."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


class WebSearchService:
    def __init__(self, config_resolver: Callable[[], SearchApiConfig]) -> None:
        self._resolve_config = config_resolver

    # ------------------------------------------------------------------ helpers
    def _require_config(self) -> SearchApiConfig:
        config = self._resolve_config()
        if not config.enabled:
            raise WebSearchError(
                "WEB_SEARCH_DISABLED",
                "联网搜索未启用，请先在设置中开启并配置 Tavily API Key。",
            )
        if not (config.api_key or "").strip():
            raise WebSearchError(
                "WEB_SEARCH_NO_KEY",
                "缺少 Tavily API Key，请先在设置中配置。",
            )
        return config

    async def _post(self, path: str, payload: Dict[str, Any]) -> Dict[str, Any]:
        try:
            async with httpx.AsyncClient(timeout=_TIMEOUT) as client:
                response = await client.post(f"{_TAVILY_BASE}{path}", json=payload)
                response.raise_for_status()
                data = response.json()
                return data if isinstance(data, dict) else {}
        except httpx.HTTPStatusError as exc:
            raise WebSearchError(
                "WEB_SEARCH_UPSTREAM",
                f"Tavily 请求失败（HTTP {exc.response.status_code}）：{exc.response.text[:200]}",
            ) from exc
        except httpx.HTTPError as exc:
            raise WebSearchError("WEB_SEARCH_NETWORK", f"Tavily 网络错误：{exc}") from exc

    # ------------------------------------------------------------------- search
    async def search(self, query: str, *, max_results: int = 5) -> Dict[str, Any]:
        """Run a Tavily search; returns ``{"query", "answer", "results": [...]}``."""
        config = self._require_config()
        payload: Dict[str, Any] = {
            "api_key": config.api_key,
            "query": query,
            "topic": config.topic or "general",
            "search_depth": config.search_depth or "basic",
            "max_results": max(1, min(int(max_results or 5), 10)),
            "include_answer": True,
        }
        if config.time_range:
            payload["time_range"] = config.time_range
        data = await self._post("/search", payload)
        results: List[Dict[str, Any]] = []
        for item in data.get("results") or []:
            if not isinstance(item, dict):
                continue
            results.append(
                {
                    "title": str(item.get("title") or ""),
                    "url": str(item.get("url") or ""),
                    "content": str(item.get("content") or ""),
                    "score": item.get("score"),
                }
            )
        return {
            "query": query,
            "answer": str(data.get("answer") or ""),
            "results": results,
        }

    # ------------------------------------------------------------------ extract
    async def extract(self, url: str) -> Dict[str, Any]:
        """Fetch/extract a single URL via Tavily; returns ``{"url", "content"}``."""
        config = self._require_config()
        payload: Dict[str, Any] = {
            "api_key": config.api_key,
            "urls": [url],
            "extract_depth": config.extract_depth or "basic",
        }
        data = await self._post("/extract", payload)
        for item in data.get("results") or []:
            if isinstance(item, dict) and item.get("url"):
                return {
                    "url": str(item.get("url")),
                    "content": str(item.get("raw_content") or item.get("content") or ""),
                }
        failed = data.get("failed_results") or []
        detail = ""
        if failed and isinstance(failed[0], dict):
            detail = str(failed[0].get("error") or "")
        raise WebSearchError("WEB_FETCH_FAILED", f"无法抓取 {url}：{detail or '无结果'}")
