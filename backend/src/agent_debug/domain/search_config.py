"""Web search (Tavily) API configuration domain model.

重建说明：原文件随 ``backend/`` 目录意外丢失，本实现按
``rest_gateway._search_config_to_dict`` / ``set_search_config`` 的字段
约定重建（enabled / provider / api_key / topic / search_depth /
time_range / extract_depth / created_at / updated_at）。
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class SearchApiConfig:
    """Tavily search configuration persisted by ``SearchConfigStore``."""

    enabled: bool = False
    provider: str = "tavily"
    api_key: str = ""
    topic: str = "general"
    search_depth: str = "basic"
    time_range: str = ""
    extract_depth: str = "basic"
    created_at: str = ""
    updated_at: str = ""

    def to_dict(self) -> dict:
        return {
            "enabled": self.enabled,
            "provider": self.provider,
            "api_key": self.api_key,
            "topic": self.topic,
            "search_depth": self.search_depth,
            "time_range": self.time_range,
            "extract_depth": self.extract_depth,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        }

    @staticmethod
    def from_dict(data: dict | None) -> "SearchApiConfig":
        data = data or {}
        return SearchApiConfig(
            enabled=bool(data.get("enabled", False)),
            provider=str(data.get("provider") or "tavily"),
            api_key=str(data.get("api_key") or data.get("apiKey") or ""),
            topic=str(data.get("topic") or "general"),
            search_depth=str(data.get("search_depth") or data.get("searchDepth") or "basic"),
            time_range=str(data.get("time_range") or data.get("timeRange") or ""),
            extract_depth=str(data.get("extract_depth") or data.get("extractDepth") or "basic"),
            created_at=str(data.get("created_at") or data.get("createdAt") or ""),
            updated_at=str(data.get("updated_at") or data.get("updatedAt") or ""),
        )
