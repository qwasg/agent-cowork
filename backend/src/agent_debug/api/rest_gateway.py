from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Any, Dict, Iterable, List

import httpx

from src.agent_debug.domain.checkpoint_service import CheckpointService
from src.agent_debug.domain.code_edit_engine import (
    CodeEditEngine,
    ProposalRegistry,
    proposal_to_dict,
)
from src.agent_debug.domain.context_compactor import ContextCompactor
from src.agent_debug.domain.context_manager import SessionContextManager
from src.agent_debug.domain.permission_service import PermissionService
from src.agent_debug.domain.models import DebugEvent, asdict_safe
from src.agent_debug.domain.plan_engine import PlanEngine
from src.agent_debug.domain.replay_service import ReplayService
from src.agent_debug.domain.runtime import AgentRuntimeService
from src.agent_debug.domain.search_config import SearchApiConfig
from src.agent_debug.domain.session_service import SessionService
from src.agent_debug.domain.subagent_orchestrator import SubagentOrchestrator
from src.agent_debug.domain.summary_manager import SummaryManager
from src.agent_debug.domain.swarm_coordinator import SwarmCoordinator
from src.agent_debug.domain.todo_engine import TodoEngine
from src.agent_debug.domain.tools import (
    build_default_workspace_tools,
    register_mcp_demo_tools,
    register_web_tools,
)
from src.agent_debug.domain.web_search_service import WebSearchService
from src.agent_debug.domain.workspace_tree import (
    NotAGitRepoError,
    WorkspaceTreeService,
    browse_directories,
)
from src.agent_debug.infra.crypto_store import CryptoStore
from src.agent_debug.infra.event_bus import EventBus
from src.agent_debug.infra.jsonl_store import JsonlEventStore
from src.agent_debug.infra.search_config_store import SearchConfigStore
from src.agent_debug.infra.tracing import TraceCollector
from src.agent_debug.infra.utils import make_id, utc_now_iso
from src.agent_debug.prompts import (
    build_chat_system_content,
    discover_skill_records,
    discover_workspace_skills,
    normalize_composer_chat_mode,
    normalize_composer_runtime_mode,
    resolve_composer_profile,
    resolve_skill_path,
)
from src.agent_debug.prompts.agent_project_files import read_agent_md_snippet
from src.agent_debug.provider.base import (
    ProviderRegistry,
)
from src.agent_debug.provider.channel_store import ChannelStore
from src.agent_debug.provider.channels import (
    ALL_PROVIDER_TYPES,
    Channel,
    ChannelModel,
    PROVIDER_DEFAULT_URLS,
    PROVIDER_LABELS,
    default_base_url,
    provider_protocol,
)
from src.agent_debug.provider.package_model_catalog import PackageModelCatalog
from src.agent_debug.provider.service import (
    ProviderExecutionService,
    build_provider_registry,
)


def _normalize_base_url(url: str) -> str:
    return (url or "").strip().rstrip("/")


def _normalize_anthropic_base_url(url: str) -> str:
    base = _normalize_base_url(url)
    return base if base.endswith("/v1") else f"{base}/v1"


def _fetch_models_from_provider(provider: str, base_url: str, api_key: str) -> List[ChannelModel]:
    protocol = provider_protocol(provider)  # type: ignore[arg-type]
    timeout = httpx.Timeout(30.0, connect=10.0)

    if protocol == "google":
        url = f"{_normalize_base_url(base_url)}/v1beta/models"
        response = httpx.get(url, params={"key": api_key}, timeout=timeout)
        response.raise_for_status()
        items = response.json().get("models") or []
        models = []
        for item in items:
            methods = item.get("supportedGenerationMethods") or []
            if "generateContent" not in methods:
                continue
            model_id = str(item.get("name") or "").removeprefix("models/")
            if model_id:
                models.append(
                    ChannelModel(
                        id=model_id,
                        name=str(item.get("displayName") or model_id),
                        enabled=True,
                    )
                )
        return models

    if protocol == "anthropic":
        versioned_base = (
            _normalize_base_url(base_url)
            if provider in ("deepseek", "kimi-api", "kimi-coding")
            else _normalize_anthropic_base_url(base_url)
        )
        headers: Dict[str, str] = {"anthropic-version": "2023-06-01"}
        if provider in ("kimi-coding", "minimax"):
            headers["Authorization"] = f"Bearer {api_key}"
        else:
            headers["x-api-key"] = api_key
            headers["Authorization"] = f"Bearer {api_key}"
        if provider == "kimi-coding":
            headers["User-Agent"] = "KimiCLI/1.3"
        response = httpx.get(f"{versioned_base}/models", headers=headers, timeout=timeout)
        response.raise_for_status()
        items = response.json().get("data") or []
        return [
            ChannelModel(
                id=str(item.get("id")),
                name=str(item.get("display_name") or item.get("id")),
                enabled=True,
            )
            for item in items
            if item.get("id")
        ]

    response = httpx.get(
        f"{_normalize_base_url(base_url)}/models",
        headers={"Authorization": f"Bearer {api_key}"},
        timeout=timeout,
    )
    response.raise_for_status()
    items = response.json().get("data") or []
    models = [
        ChannelModel(id=str(item.get("id")), name=str(item.get("id")), enabled=True)
        for item in items
        if item.get("id")
    ]
    models.sort(key=lambda m: m.id)
    return models


class AgentDebugRestGateway:
    def __init__(self, provider_registry: ProviderRegistry | None = None) -> None:
        self.event_bus = EventBus()
        self.sessions = SessionService()
        self._wire_event_persistence()
        self.crypto_store = CryptoStore()
        self.channel_store = ChannelStore(crypto=self.crypto_store)
        self.search_config_store = SearchConfigStore(crypto=self.crypto_store)
        self.model_catalog = PackageModelCatalog(channel_store=self.channel_store)
        self.trace_collector = TraceCollector()
        self.provider_registry = provider_registry or build_provider_registry(
            channel_store=self.channel_store
        )
        self.provider_service = ProviderExecutionService(
            self.provider_registry, self.event_bus, trace_collector=self.trace_collector
        )
        self.provider_service.model_channel_resolver = self._channel_provider_for_model
        self.plan_engine = PlanEngine(self.provider_service, model_resolver=self.resolve_model_for_session)
        self.todo_engine = TodoEngine()
        self.summary_manager = SummaryManager(
            self.provider_service,
            model_resolver=self.resolve_model_for_session,
        )
        self.subagents = SubagentOrchestrator()
        self.swarm = SwarmCoordinator()
        self.context_manager = SessionContextManager()
        self.code_edit_engine = CodeEditEngine()
        self.proposals = ProposalRegistry()
        self.workspace_tree = WorkspaceTreeService()
        self.tool_registry = build_default_workspace_tools(self.workspace_tree)
        self.web_search_service = WebSearchService(config_resolver=self.search_config_store.get_config)
        register_web_tools(self.tool_registry, service=self.web_search_service)
        # 把仓库内 MCP demo 暴露为 agent 工具（mcp__demo__*）；MCP 依赖/脚本缺失时静默跳过。
        # pytest 下默认关闭（避免每个 gateway 实例都拉起 stdio 子进程拖慢测试）。
        import sys as _sys

        _mcp_disabled = (
            "pytest" in _sys.modules
            or os.getenv("AGENT_DEBUG_DISABLE_MCP_TOOLS", "").strip().lower() in ("1", "true", "yes", "on")
        )
        if not _mcp_disabled:
            try:
                register_mcp_demo_tools(self.tool_registry)
            except Exception:  # pragma: no cover - never block startup on MCP
                pass
        self.checkpoints = CheckpointService(self.workspace_tree, self.event_bus)
        self.permission_service = PermissionService()
        self.context_compactor = ContextCompactor(
            self.provider_service,
            model_resolver=self.resolve_model_for_session,
            context_budget_resolver=self.context_budget_for_session,
        )
        self.session_context_windows: dict[str, dict[str, Any]] = {}
        self.runtime = AgentRuntimeService(
            todo_engine=self.todo_engine,
            subagent_orchestrator=self.subagents,
            summary_manager=self.summary_manager,
            event_bus=self.event_bus,
            context_manager=self.context_manager,
            provider_service=self.provider_service,
            model_resolver=self.resolve_model_for_session,
            tool_registry=self.tool_registry,
            tool_allowlist_resolver=self.allowed_tool_names_for_session,
            trace_collector=self.trace_collector,
            permission_service=self.permission_service,
            context_compactor=self.context_compactor,
        )
        self.replay = ReplayService(self.event_bus)

    def _wire_event_persistence(self) -> None:
        """挂载 JSONL 事件持久化并在启动时回放历史（pytest 下默认关闭）。"""
        import sys

        disabled = "pytest" in sys.modules or os.getenv("AGENT_DEBUG_PERSIST_EVENTS", "1") in (
            "0",
            "false",
            "no",
        )
        if disabled:
            self.event_store = None
            return
        try:
            self.event_store = JsonlEventStore()
            self.event_bus.attach_persistence(self.event_store)
            known_ids = {session.id for session in self.sessions.sessions.list_all()}
            for session_id in known_ids:
                self.event_bus.hydrate_session(
                    session_id, self.event_store.read_session(session_id)
                )
            # 清理不在会话索引内的孤儿 JSONL（历史删除残留 / 幽灵会话历史），
            # 防止旧历史在后续 fork/回放中复活。
            try:
                for orphan in self.event_store.list_sessions():
                    if orphan not in known_ids:
                        self.event_store.delete_session(orphan)
            except Exception:  # pragma: no cover - 清理失败不影响启动
                pass
        except Exception:  # pragma: no cover - 持久化不可用时降级为纯内存
            self.event_store = None

    # ============================================================ sessions

    def create_session(
        self,
        title: str,
        selected_model_id: str | None = None,
        web_search_enabled: bool = False,
    ) -> Dict[str, Any]:
        normalized_model_id = self.model_catalog.normalize_model_id(selected_model_id)
        session = self.sessions.create(
            title,
            selected_model_id=normalized_model_id,
            web_search_enabled=web_search_enabled,
        )
        return {"session": asdict_safe(session)}

    def get_session(self, session_id: str) -> Dict[str, Any]:
        session = self.sessions.get(session_id)
        if session is None:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        return {"session": asdict_safe(session)}

    def list_sessions(self) -> Dict[str, Any]:
        sessions = sorted(
            self.sessions.sessions.list_all(),
            key=lambda s: ((s.updated_at or s.created_at or ""), s.id),
            reverse=True,
        )
        sessions.sort(key=lambda s: 0 if getattr(s, "pinned", False) else 1)
        return {"sessions": [asdict_safe(s) for s in sessions]}

    def patch_session(self, session_id: str, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        if self.sessions.get(session_id) is None:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        session = None
        if "title" in data:
            title = data.get("title")
            if not isinstance(title, str) or not title.strip():
                return {"error": {"code": "INVALID_TITLE", "message": "title must be non-empty string"}}
            session = self.sessions.update_title(session_id, title)
        if "pinned" in data:
            session = self.sessions.set_pinned(session_id, bool(data.get("pinned")))
        if "webSearchEnabled" in data:
            session = self.sessions.set_web_search_enabled(
                session_id, bool(data.get("webSearchEnabled"))
            )
        if session is None:
            session = self.sessions.get(session_id)
        return {"session": asdict_safe(session)}

    def delete_session(self, session_id: str) -> Dict[str, Any]:
        if self.sessions.get(session_id) is None:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        deleted = self.sessions.delete(session_id)
        if deleted:
            self._purge_session_state(session_id)
        return {"deleted": bool(deleted), "id": session_id}

    def _purge_session_state(self, session_id: str) -> None:
        """会话删除后级联清理历史事件与内存状态。

        否则：JSONL 历史会在重启时被 ``hydrate_session`` 灌回（历史记忆复活），
        而 Plan/Todo/contextWindow 等内存状态残留会造成「创建/历史紊乱」。
        每一步独立 try/except，确保单点失败不影响其余清理。
        """
        if getattr(self, "event_store", None) is not None:
            try:
                self.event_store.delete_session(session_id)
            except Exception:  # pragma: no cover - 持久化删除失败不应阻断
                pass
        try:
            self.event_bus.purge_session(session_id)
        except Exception:  # pragma: no cover
            pass
        try:
            self.todo_engine.delete_session_todos(session_id)
        except Exception:  # pragma: no cover
            pass
        try:
            self.plan_engine.delete_session_plans(session_id)
        except Exception:  # pragma: no cover
            pass
        self.session_context_windows.pop(session_id, None)

    def fork_session(self, session_id: str) -> Dict[str, Any]:
        original = self.sessions.get(session_id)
        if not original:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        
        new_session = self.sessions.create(
            title=f"分支 · {original.title}",
            selected_model_id=original.selected_model_id,
            web_search_enabled=original.web_search_enabled,
        )
        plan_node_id_map: Dict[str, str] = {}
        if original.active_plan_id:
            cloned = self.plan_engine.clone_plan_to_session(original.active_plan_id, new_session.id)
            if cloned is not None:
                bundle, plan_node_id_map = cloned
                self.sessions.update_active_plan(
                    new_session.id,
                    bundle["plan"].id,
                    status=new_session.status,
                )
                new_session = self.sessions.get(new_session.id) or new_session
        self.todo_engine.clone_session_todos(
            session_id,
            new_session.id,
            plan_node_id_map=plan_node_id_map,
        )
        self.event_bus.fork_session(session_id, new_session.id)
        return {"session": asdict_safe(new_session)}

    def revert_session(
        self,
        session_id: str,
        message_id: str | None,
        mode: str | None = None,
    ) -> Dict[str, Any]:
        original = self.sessions.get(session_id)
        if not original:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        
        if message_id:
            if mode == "before":
                # exclusive：删除目标消息（及其所属 run 的先导事件）本身，
                # 用于「编辑历史消息后回退重发」。
                self.event_bus.truncate_before_event(session_id, message_id)
            else:
                self.event_bus.truncate_session(session_id, message_id)
            self.runtime.invalidate_session_history(session_id)
            self.sessions.update_active_plan(session_id, None, status="idle")
            self.sessions.update_active_run(session_id, None, status="idle")
            
        return {"session": asdict_safe(self.sessions.get(session_id) or original)}

    def _full_session_events(self, session_id: str) -> list[dict[str, Any]]:
        """Merge in-memory bus events with JSONL persistence for complete session history."""
        by_id: dict[str, dict[str, Any]] = {}
        for item in self.event_bus.snapshot(session_id):
            event_id = item.get("id")
            if event_id:
                by_id[event_id] = item
        store = getattr(self, "event_store", None)
        if store is not None:
            for raw in store.read_session(session_id):
                try:
                    event = DebugEvent(
                        id=raw["id"],
                        session_id=raw.get("session_id", session_id),
                        seq=int(raw.get("seq", 0)),
                        type=raw.get("type", ""),
                        ts=raw.get("ts", ""),
                        source=raw.get("source", {}),
                        payload=raw.get("payload", {}),
                        correlation_id=raw.get("correlation_id"),
                    )
                except (KeyError, TypeError, ValueError):
                    continue
                camel = asdict_safe(event)
                event_id = camel.get("id")
                if not event_id:
                    continue
                existing = by_id.get(event_id)
                if existing is None or int(camel.get("seq") or 0) >= int(existing.get("seq") or 0):
                    by_id[event_id] = camel
        return sorted(by_id.values(), key=lambda e: int(e.get("seq") or 0))

    def _replay_gap_for_from_seq(self, session_id: str, from_seq: int) -> bool:
        full = self._full_session_events(session_id)
        if not full:
            return from_seq > 0
        oldest = min(int(e.get("seq") or 0) for e in full)
        return from_seq + 1 < oldest

    # ============================================================ snapshot

    def get_design_snapshot(self, session_id: str | None = None) -> Dict[str, Any]:
        sessions = self.sessions.sessions.list_all()
        active_session = self.sessions.get(session_id or "") if session_id else (sessions[0] if sessions else None)
        active_session_dict = asdict_safe(active_session) if active_session is not None else None
        plan_bundle = None
        todos: list[dict[str, Any]] = []
        events: list[dict[str, Any]] = []
        run = None
        context_window = None
        proposals: list[dict[str, Any]] = []

        if active_session is not None:
            context_window = self.session_context_windows.get(active_session.id)
            if active_session.active_plan_id:
                snapshot = self.plan_engine.get(active_session.active_plan_id)
                if snapshot is not None:
                    plan_bundle = {
                        "plan": asdict_safe(snapshot["plan"]),
                        "stages": [asdict_safe(item) for item in snapshot["stages"]],
                        "tasks": [asdict_safe(item) for item in snapshot["tasks"]],
                        "steps": [asdict_safe(item) for item in snapshot["steps"]],
                        "versions": [asdict_safe(item) for item in snapshot["versions"]],
                    }
            todos = [asdict_safe(item) for item in self.todo_engine.list_default_visible_by_session(active_session.id)]
            events = self._full_session_events(active_session.id)
            if active_session.active_run_id:
                active_run = self.runtime.get_run(active_session.active_run_id)
                if active_run is None:
                    active_session = self.sessions.update_active_run(active_session.id, None, status="idle")
                    active_session_dict = asdict_safe(active_session) if active_session is not None else None
                else:
                    run = asdict_safe(active_run)
            proposals = [proposal_to_dict(p) for p in self.proposals.list_for_session(active_session.id)]

        swarm_nodes = [asdict_safe(node) for node in self.swarm.list_nodes()]
        swarm_shards = [asdict_safe(shard) for shard in self.swarm.list_shards()]
        metrics = self._build_design_metrics(plan_bundle, todos, events, swarm_nodes, context_window)
        diffs = self._extract_design_diffs(events)
        return {
            "sessions": [asdict_safe(s) for s in sessions],
            "activeSession": active_session_dict,
            "planBundle": plan_bundle,
            "todos": todos,
            "events": events,
            "run": run,
            "swarm": {"nodes": swarm_nodes, "shards": swarm_shards},
            "diffs": diffs,
            "proposals": proposals,
            "metrics": metrics,
            "contextWindow": context_window,
            "models": self.list_models(),
            "latestSeq": self.event_bus.latest_seq(active_session.id) if active_session else 0,
        }

    # ============================================================ models / provider

    def list_models(self) -> Dict[str, Any]:
        models = self.model_catalog.list_models()
        return {
            "items": [asdict_safe(item) for item in models],
            "preferences": asdict_safe(self.model_catalog.get_preferences()),
        }

    def get_provider_status(self) -> Dict[str, Any]:
        registered = list(self.provider_registry.names())
        chain_env = os.getenv("AGENT_DEBUG_PROVIDER_CHAIN", "").strip()
        configured_chain = [name.strip() for name in chain_env.split(",") if name.strip()]
        default_model = self.model_catalog.get_default_model_id()
        enabled_channels = self.channel_store.enabled_channels()
        active_channel = self.channel_store.find_channel_for_model(default_model)
        if active_channel is None and enabled_channels:
            active_channel = enabled_channels[0]
        api_key_set = bool(active_channel and (active_channel.api_key or "").strip())
        base_url = active_channel.base_url if active_channel is not None else None

        live_chain = [name for name in configured_chain if name in registered]
        if not live_chain:
            live_chain = list(registered)

        if api_key_set:
            mode = "live"
        elif "mock" in registered or "fake" in registered:
            mode = "mock"
        else:
            mode = "offline"

        return {
            "mode": mode,
            "registeredProviders": registered,
            "configuredChain": configured_chain,
            "effectiveChain": live_chain,
            "openaiCompatible": {
                "configured": api_key_set,
                "baseUrl": base_url,
                "model": default_model,
            },
            "defaultModelId": default_model,
        }

    @staticmethod
    def _search_config_to_dict(config: SearchApiConfig) -> Dict[str, Any]:
        return {
            "enabled": config.enabled,
            "provider": config.provider or "tavily",
            "apiKeySet": bool((config.api_key or "").strip()),
            "topic": config.topic,
            "searchDepth": config.search_depth,
            "timeRange": config.time_range,
            "extractDepth": config.extract_depth,
            "updatedAt": config.updated_at,
            "createdAt": config.created_at,
        }

    def get_search_config(self) -> Dict[str, Any]:
        return {"config": self._search_config_to_dict(self.search_config_store.get_config())}

    def set_search_config(self, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        existing = self.search_config_store.get_config()
        api_key = data.get("apiKey")
        topic = str(data.get("topic", existing.topic) or existing.topic or "general").strip().lower() or "general"
        if topic not in {"general", "news"}:
            topic = existing.topic or "general"
        search_depth = (
            str(data.get("searchDepth", existing.search_depth) or existing.search_depth or "basic").strip().lower()
            or "basic"
        )
        if search_depth not in {"basic", "advanced"}:
            search_depth = existing.search_depth or "basic"
        time_range = (
            str(data.get("timeRange", existing.time_range) or existing.time_range or "").strip().lower()
        )
        if time_range not in {"", "day", "week", "month", "year"}:
            time_range = existing.time_range or ""
        extract_depth = (
            str(data.get("extractDepth", existing.extract_depth) or existing.extract_depth or "basic")
            .strip()
            .lower()
            or "basic"
        )
        if extract_depth not in {"basic", "advanced"}:
            extract_depth = existing.extract_depth or "basic"
        saved = self.search_config_store.save_config(
            SearchApiConfig(
                enabled=bool(data.get("enabled", existing.enabled)),
                provider="tavily",
                api_key=existing.api_key if api_key in (None, "") else str(api_key).strip(),
                topic=topic,
                search_depth=search_depth,
                time_range=time_range,
                extract_depth=extract_depth,
                created_at=existing.created_at,
            )
        )
        return {"config": self._search_config_to_dict(saved)}

    def _rebuild_provider_registry(self) -> None:
        """重建供应商 registry（含渠道），并刷新缓存的渠道视图。"""
        # 渠道缓存可能已变更，强制重读。
        self.channel_store = ChannelStore(crypto=self.crypto_store)
        self.model_catalog = PackageModelCatalog(channel_store=self.channel_store)
        self.provider_registry = build_provider_registry(channel_store=self.channel_store)
        self.provider_service.registry = self.provider_registry
        self.provider_service.model_channel_resolver = self._channel_provider_for_model

    def _channel_provider_for_model(self, model_id: str | None) -> str | None:
        """把模型 id 解析为应优先使用的 ``channel:{id}`` provider 名称。

        读取当前 ``channel_store``（渠道增删改后会被替换），保证按所选模型
        路由到对应渠道的 API Key。
        """
        channel = self.channel_store.find_channel_for_model(model_id)
        return f"channel:{channel.id}" if channel else None

    def allowed_tool_names_for_session(self, session_id: str) -> List[str]:
        names = list(self.tool_registry.names())
        session = self.sessions.get(session_id)
        if session and session.web_search_enabled:
            return names
        return [name for name in names if name not in {"web_search", "web_fetch"}]

    # ============================================================ channels

    @staticmethod
    def _channel_to_dict(channel: Channel) -> Dict[str, Any]:
        return {
            "id": channel.id,
            "name": channel.name,
            "provider": channel.provider,
            "providerLabel": PROVIDER_LABELS.get(channel.provider, channel.provider),
            "baseUrl": channel.base_url,
            "apiKeySet": bool((channel.api_key or "").strip()),
            "protocol": channel.protocol,
            "isChina": channel.is_china,
            "models": [
                {"id": m.id, "name": m.name, "enabled": m.enabled} for m in channel.models
            ],
            "enabled": channel.enabled,
            "createdAt": channel.created_at,
            "updatedAt": channel.updated_at,
        }

    def list_provider_types(self) -> Dict[str, Any]:
        items = [
            {
                "provider": ptype,
                "label": PROVIDER_LABELS.get(ptype, ptype),
                "defaultBaseUrl": PROVIDER_DEFAULT_URLS.get(ptype, ""),
            }
            for ptype in ALL_PROVIDER_TYPES
        ]
        return {"providerTypes": items}

    def list_channels(self) -> Dict[str, Any]:
        return {"channels": [self._channel_to_dict(c) for c in self.channel_store.list_channels()]}

    def fetch_channel_models(self, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        provider = data.get("provider")
        if provider not in ALL_PROVIDER_TYPES:
            return {"error": {"code": "INVALID_PROVIDER", "message": f"unknown provider: {provider}"}}

        channel = self.channel_store.get_channel(str(data.get("channelId") or ""))
        api_key = str(data.get("apiKey") or "").strip()
        if not api_key and channel is not None:
            api_key = channel.api_key
        if not api_key:
            return {
                "success": False,
                "message": "请先输入 API Key，或保存渠道后再获取模型列表",
                "models": [],
            }

        base_url = str(
            data.get("baseUrl")
            or (channel.base_url if channel is not None else "")
            or default_base_url(provider)
        )
        try:
            models = _fetch_models_from_provider(str(provider), base_url, api_key)
        except httpx.HTTPStatusError as exc:
            body = exc.response.text[:200] if exc.response is not None else ""
            if exc.response is not None and exc.response.status_code in (400, 401, 403):
                message = f"API Key 无效或无权限{f': {body}' if body else ''}"
            else:
                status = exc.response.status_code if exc.response is not None else "?"
                message = f"请求失败 ({status}){f': {body}' if body else ''}"
            return {"success": False, "message": message, "models": []}
        except Exception as exc:  # noqa: BLE001 - surface provider/network errors to UI
            return {"success": False, "message": f"拉取模型失败: {exc}", "models": []}

        return {
            "success": True,
            "message": f"成功获取 {len(models)} 个模型",
            "models": [{"id": m.id, "name": m.name, "enabled": m.enabled} for m in models],
        }

    def upsert_channel(self, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        provider = data.get("provider")
        if provider not in ALL_PROVIDER_TYPES:
            return {"error": {"code": "INVALID_PROVIDER", "message": f"unknown provider: {provider}"}}
        existing = self.channel_store.get_channel(data.get("id")) if data.get("id") else None
        models = [
            ChannelModel(
                id=str(m.get("id")),
                name=str(m.get("name", "")),
                enabled=bool(m.get("enabled", True)),
            )
            for m in (data.get("models") or [])
            if isinstance(m, dict) and m.get("id")
        ]
        # apiKey: 空字符串表示「不修改」（保留旧值）。
        api_key = data.get("apiKey")
        if not api_key and existing is not None:
            api_key = existing.api_key
        channel = Channel(
            id=data.get("id") or make_id("chan"),
            name=str(data.get("name") or provider),
            provider=provider,
            base_url=str(data.get("baseUrl") or default_base_url(provider)),
            api_key=str(api_key or ""),
            models=models,
            enabled=bool(data.get("enabled", True)),
            created_at=existing.created_at if existing else "",
        )
        saved = self.channel_store.upsert_channel(channel)
        self._rebuild_provider_registry()
        return {"channel": self._channel_to_dict(saved)}

    def delete_channel(self, channel_id: str) -> Dict[str, Any]:
        ok = self.channel_store.delete_channel(channel_id)
        if not ok:
            return {"error": {"code": "CHANNEL_NOT_FOUND", "message": "channel not found"}}
        self._rebuild_provider_registry()
        return {"ok": True}

    # ============================================================ permissions

    def set_permission_mode(self, session_id: str, mode: str) -> Dict[str, Any]:
        if mode not in ("bypass", "plan", "auto"):
            return {"error": {"code": "INVALID_MODE", "message": f"unknown permission mode: {mode}"}}
        self.permission_service.set_mode(session_id, mode)  # type: ignore[arg-type]
        return {"sessionId": session_id, "mode": mode}

    def get_permission_mode(self, session_id: str) -> Dict[str, Any]:
        return {"sessionId": session_id, "mode": self.permission_service.get_mode(session_id)}

    # ============================================================ checkpoints

    def create_checkpoint(
        self, session_id: str, paths: List[str] | None = None, label: str = ""
    ) -> Dict[str, Any]:
        checkpoint = self.checkpoints.create_checkpoint(
            session_id, paths=paths or [], label=label
        )
        return {"checkpoint": asdict_safe(checkpoint)}

    def list_checkpoints(self, session_id: str) -> Dict[str, Any]:
        return {
            "checkpoints": [asdict_safe(c) for c in self.checkpoints.list_checkpoints(session_id)]
        }

    def rewind_checkpoint(self, checkpoint_id: str) -> Dict[str, Any]:
        try:
            return self.checkpoints.rewind(checkpoint_id)
        except KeyError:
            return {"error": {"code": "CHECKPOINT_NOT_FOUND", "message": "checkpoint not found"}}

    def list_builtin_subagents(self) -> Dict[str, Any]:
        from src.agent_debug.prompts import subagents_as_dicts

        return {"subagents": subagents_as_dicts()}

    def get_model_preferences(self) -> Dict[str, Any]:
        return {"preferences": asdict_safe(self.model_catalog.get_preferences())}

    def set_model_preferences(self, model_id: str) -> Dict[str, Any]:
        try:
            preferences = self.model_catalog.set_default_model_id(model_id)
        except ValueError:
            return {"error": {"code": "MODEL_NOT_FOUND", "message": "model not found"}}
        return {"preferences": asdict_safe(preferences)}

    def set_session_model(self, session_id: str, model_id: str | None) -> Dict[str, Any]:
        normalized_model_id = self.model_catalog.normalize_model_id(model_id)
        if model_id is not None and normalized_model_id is None:
            return {"error": {"code": "MODEL_NOT_FOUND", "message": "model not found"}}
        session = self.sessions.update_selected_model(session_id, normalized_model_id)
        if session is None:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        return {"session": asdict_safe(session)}

    # ============================================================ plan / run

    async def generate_plan(
        self,
        session_id: str,
        user_input: str,
        context_window: Dict[str, Any] | None = None,
    ) -> Dict[str, Any]:
        self.sessions.auto_title_from_input(session_id, user_input)
        if isinstance(context_window, dict):
            self.session_context_windows[session_id] = context_window
        result = await self.plan_engine.generate(session_id, user_input)
        self.todo_engine.archive_plan_todos_except(
            session_id,
            [task.id for task in result.get("tasks", [])],
        )
        self.sessions.update_active_plan(session_id, result["plan"].id, status="planning")
        await self.runtime.publish(
            session_id,
            "plan.created",
            "plan",
            result["plan"].id,
            asdict_safe(result["plan"]),
        )
        return self._render_plan_bundle(result)

    def get_plan(self, plan_id: str) -> Dict[str, Any]:
        snapshot = self.plan_engine.get(plan_id)
        if snapshot is None:
            return {"error": {"code": "PLAN_NOT_FOUND", "message": "plan not found"}}
        return self._render_plan_snapshot(snapshot)

    def patch_plan(self, plan_id: str, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        snapshot = self.plan_engine.patch(
            plan_id,
            objective=data.get("objective") if isinstance(data.get("objective"), str) else None,
            status=data.get("status") if isinstance(data.get("status"), str) else None,
        )
        if snapshot is None:
            return {"error": {"code": "PLAN_NOT_FOUND", "message": "plan not found"}}
        return self._render_plan_snapshot(snapshot)

    def confirm_plan(self, plan_id: str) -> Dict[str, Any]:
        snapshot = self.plan_engine.confirm(plan_id)
        if snapshot is None:
            return {"error": {"code": "PLAN_NOT_FOUND", "message": "plan not found"}}
        return self._render_plan_snapshot(snapshot)

    async def replan(self, plan_id: str, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        new_input = str(data.get("userInput") or "").strip()
        if not new_input:
            return {"error": {"code": "PLAN_INVALID_STATE", "message": "userInput is required for replan"}}
        bundle = await self.plan_engine.replan(
            plan_id,
            new_user_input=new_input,
            reason=str(data.get("reason") or "user_replan"),
        )
        if bundle is None:
            return {"error": {"code": "PLAN_NOT_FOUND", "message": "plan not found"}}
        new_plan = bundle["plan"]
        self.todo_engine.archive_plan_todos_except(
            new_plan.session_id,
            [task.id for task in bundle.get("tasks", [])],
        )
        self.sessions.update_active_plan(new_plan.session_id, new_plan.id, status="planning")
        await self.runtime.publish(
            new_plan.session_id,
            "plan.replanned",
            "plan",
            new_plan.id,
            {"newPlanId": new_plan.id, "reason": str(data.get("reason") or "user_replan"), "previousPlanId": plan_id},
        )
        return self._render_plan_bundle(bundle)

    async def execute_plan(self, plan_id: str) -> Dict[str, Any]:
        snapshot = self.plan_engine.get(plan_id)
        if snapshot is None:
            return {"error": {"code": "PLAN_NOT_FOUND", "message": "plan not found"}}

        plan = snapshot["plan"]
        tasks = snapshot["tasks"]
        existing = [
            t for t in self.todo_engine.list_by_session(plan.session_id)
            if any(node in (t.related_plan_node_ids or []) for node in [task.id for task in tasks])
            and not t.archived_at
        ]
        if existing:
            todos = existing
            if all(t.status in {"completed", "rolled_up", "failed", "cancelled", "skipped"} for t in todos):
                todos = self.todo_engine.reset_plan_todos(todos)
        else:
            todos = self.todo_engine.materialize_from_tasks(plan.session_id, tasks)

        run = await self.runtime.start(plan.session_id, plan.id, plan.objective)
        self.sessions.update_active_run(plan.session_id, run.id, status="running")

        for todo in todos:
            await self.runtime.publish(
                plan.session_id,
                "todo.created",
                "todo",
                todo.id,
                asdict_safe(todo),
                run.id,
            )

        outcome = await self.runtime.run_plan(run, tasks, todos)
        self._sync_plan_status_from_todos(plan.id, tasks, todos, run.status)

        proposal_payload = (
            await self._maybe_propose_code_edit(plan.session_id, plan.objective)
            if run.status == "completed"
            else None
        )
        if proposal_payload is not None:
            self.proposals.record(proposal_payload)
            await self.runtime.publish(
                plan.session_id,
                "agent.code_edit.proposed",
                "agent",
                run.id,
                proposal_payload,
                run.id,
            )

        if run.status in {"failed", "cancelled", "paused"}:
            self.sessions.update_active_run(plan.session_id, run.id, status=run.status)
        else:
            self.sessions.update_active_run(plan.session_id, None, status="idle")

        return {
            "run": asdict_safe(run),
            "outcome": outcome,
        }

    def _sync_plan_status_from_todos(
        self,
        plan_id: str,
        tasks: List[Any],
        todos: List[Any],
        run_status: str,
    ) -> None:
        todo_by_task_id: Dict[str, Any] = {}
        for todo in todos:
            for node_id in todo.related_plan_node_ids or []:
                todo_by_task_id[node_id] = todo
        for task in tasks:
            todo = todo_by_task_id.get(task.id)
            if todo is None:
                continue
            if todo.status in {"completed", "rolled_up"}:
                self.plan_engine.mark_task_status(task.id, "completed")
            elif todo.status in {"failed", "cancelled", "skipped"}:
                self.plan_engine.mark_task_status(task.id, todo.status)
            elif todo.status == "running":
                self.plan_engine.mark_task_status(task.id, "running")
        if run_status == "completed":
            self.plan_engine.mark_plan_status(plan_id, "completed")
        elif run_status in {"failed", "cancelled"}:
            self.plan_engine.mark_plan_status(plan_id, run_status)

    async def _maybe_propose_code_edit(
        self,
        session_id: str,
        objective: str,
    ) -> Dict[str, Any] | None:
        context_window = self.session_context_windows.get(session_id)
        heuristic = self.code_edit_engine.maybe_generate(session_id, objective, context_window)
        if heuristic is not None:
            return heuristic
        if not isinstance(context_window, dict) or not isinstance(context_window.get("activeFile"), dict):
            return None
        return await self.code_edit_engine.propose_with_llm(
            self.provider_service,
            session_id=session_id,
            user_input=objective,
            context_window=context_window,
            model_resolver=self.resolve_model_for_session,
        )

    def get_todos(self, session_id: str) -> Dict[str, Any]:
        items = self.todo_engine.list_default_visible_by_session(session_id)
        return {"items": [asdict_safe(item) for item in items]}

    def add_todo(self, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        session_id = str(data.get("sessionId") or "")
        title = str(data.get("title") or "").strip()
        if not session_id or not title:
            return {"error": {"code": "TODO_INVALID", "message": "sessionId and title are required"}}
        if self.sessions.get(session_id) is None:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        deps = data.get("dependencies") or []
        deps_list = [str(d) for d in deps] if isinstance(deps, list) else []
        priority_value = data.get("priority", 50)
        try:
            priority_int = int(priority_value)
        except (TypeError, ValueError):
            priority_int = 50
        todo = self.todo_engine.add_manual(
            session_id,
            title,
            description=str(data.get("description") or ""),
            dependencies=deps_list,
            priority=priority_int,
            owner_type=str(data.get("ownerType") or "user"),
        )
        return {"todo": asdict_safe(todo)}

    def patch_todo(self, todo_id: str, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        if self.todo_engine.get(todo_id) is None:
            return {"error": {"code": "TODO_NOT_FOUND", "message": "todo not found"}}
        todo = self.todo_engine.update(todo_id, data)
        return {"todo": asdict_safe(todo)}

    async def batch_rerun_todos(self, payload: Dict[str, Any] | None) -> Dict[str, Any]:
        data = payload if isinstance(payload, dict) else {}
        ids = data.get("todoIds")
        if not isinstance(ids, list) or not ids:
            return {"error": {"code": "TODO_INVALID", "message": "todoIds is required"}}
        affected = self.todo_engine.batch_rerun([str(x) for x in ids])
        for todo in affected:
            await self.runtime.publish(
                todo.session_id,
                "todo.updated",
                "todo",
                todo.id,
                asdict_safe(todo),
            )
        return {"items": [asdict_safe(t) for t in affected]}

    # ============================================================ runs

    def get_run(self, run_id: str) -> Dict[str, Any]:
        run = self.runtime.get_run(run_id)
        if run is None:
            return {"error": {"code": "RUN_NOT_FOUND", "message": "run not found"}}
        return {"run": asdict_safe(run)}

    def get_run_logs(self, run_id: str) -> Dict[str, Any]:
        if self.runtime.get_run(run_id) is None:
            return {"error": {"code": "RUN_NOT_FOUND", "message": "run not found"}}
        return {
            "runId": run_id,
            "entries": self.runtime.get_run_logs(run_id),
            "summary": self.runtime.get_run_log_summary(run_id),
        }

    def get_run_metrics(self, run_id: str) -> Dict[str, Any]:
        run = self.runtime.get_run(run_id)
        if run is None:
            return {"error": {"code": "RUN_NOT_FOUND", "message": "run not found"}}
        events = self.event_bus.snapshot(run.session_id)
        related = [e for e in events if e.get("correlationId") == run.id]
        token_total = sum(
            int((p.get("payload", {}) or {}).get("tokenUsage", {}).get("output", 0) or 0)
            for p in related
        )
        tool_calls = sum(1 for e in related if e.get("type") == "agent.tool.invoked")
        tool_failures = sum(1 for e in related if e.get("type") == "agent.tool.failed")
        return {
            "runId": run_id,
            "status": run.status,
            "completedTodoIds": list(run.completed_todo_ids),
            "failedTodoIds": list(run.failed_todo_ids),
            "tokensTotal": token_total,
            "toolCallsTotal": tool_calls,
            "toolFailuresTotal": tool_failures,
            "events": len(related),
        }

    async def pause_run(self, run_id: str) -> Dict[str, Any]:
        existing = self.runtime.get_run(run_id)
        if existing is None:
            return {"accepted": False}
        if existing.status not in {"starting", "running"}:
            return {"accepted": False, "status": existing.status}
        run = self.runtime.update_run_status(run_id, "paused")
        if run is not None:
            self.sessions.update_active_run(run.session_id, run.id, status="paused")
            await self.runtime.publish(
                run.session_id,
                "agent.paused",
                "agent",
                run.id,
                {"runId": run.id, "status": "paused"},
                run.id,
            )
        return {"accepted": run is not None}

    async def resume_run(self, run_id: str) -> Dict[str, Any]:
        existing = self.runtime.get_run(run_id)
        if existing is None:
            return {"accepted": False}
        if existing.status != "paused":
            return {"accepted": False, "status": existing.status}
        run = self.runtime.update_run_status(run_id, "running")
        if run is not None:
            self.sessions.update_active_run(run.session_id, run.id, status="running")
            await self.runtime.publish(
                run.session_id,
                "agent.resumed",
                "agent",
                run.id,
                {"runId": run.id, "status": "running"},
                run.id,
            )
        return {"accepted": run is not None}

    async def cancel_run(self, run_id: str) -> Dict[str, Any]:
        run = self.runtime.get_run(run_id)
        if run is None:
            return {"accepted": False}
        if run.status not in {"starting", "running", "paused"}:
            return {"accepted": False, "status": run.status}
        self.runtime.request_cancel(run_id)
        self.runtime.update_run_status(run_id, "cancelled")
        self.sessions.update_active_run(run.session_id, run.id, status="cancelled")
        return {"accepted": True}

    async def rerun_todo(self, run_id: str, todo_id: str) -> Dict[str, Any]:
        run = self.runtime.get_run(run_id)
        if not run:
            return {"error": {"code": "RUN_NOT_FOUND", "message": "run not found"}}
        todo = self.todo_engine.get(todo_id)
        if todo is None:
            return {"error": {"code": "TODO_NOT_FOUND", "message": "todo not found"}}
        self.todo_engine.batch_rerun([todo_id])
        await self.runtime.publish(
            run.session_id,
            "todo.updated",
            "todo",
            todo_id,
            asdict_safe(self.todo_engine.get(todo_id)),
            run.id,
        )
        plan_id = run.plan_id or ""
        snapshot = self.plan_engine.get(plan_id) if plan_id else None
        if snapshot is None:
            return {"accepted": False, "warning": "PLAN_NOT_FOUND"}
        tasks = snapshot["tasks"]
        target_tasks = [
            t for t in tasks if any(node in (todo.related_plan_node_ids or []) for node in [t.id])
        ]
        if not target_tasks:
            return {"accepted": False, "warning": "TASK_NOT_FOUND"}
        outcome = await self.runtime.run_plan(run, target_tasks, [self.todo_engine.get(todo_id)])
        return {"accepted": True, "outcome": outcome}

    async def rerun_node(self, run_id: str, node_id: str) -> Dict[str, Any]:
        run = self.runtime.get_run(run_id)
        if not run:
            return {"error": {"code": "RUN_NOT_FOUND", "message": "run not found"}}
        plan_id = run.plan_id or ""
        snapshot = self.plan_engine.get(plan_id) if plan_id else None
        if snapshot is None:
            return {"error": {"code": "PLAN_NOT_FOUND", "message": "plan not found"}}
        tasks = [t for t in snapshot["tasks"] if t.id == node_id]
        if not tasks:
            return {"error": {"code": "PLAN_NODE_NOT_FOUND", "message": "plan node not found"}}
        related_todos = [
            t for t in self.todo_engine.list_by_session(run.session_id)
            if node_id in (t.related_plan_node_ids or [])
        ]
        if related_todos:
            self.todo_engine.batch_rerun([t.id for t in related_todos])
        outcome = await self.runtime.run_plan(run, tasks, related_todos or [])
        return {"accepted": True, "outcome": outcome}

    # ============================================================ proposals (apply / discard)

    def list_proposals(self, session_id: str) -> Dict[str, Any]:
        if session_id and self.sessions.get(session_id) is None:
            return {"error": {"code": "SESSION_NOT_FOUND", "message": "session not found"}}
        records: Iterable = (
            self.proposals.list_for_session(session_id) if session_id else self.proposals.all()
        )
        return {"items": [proposal_to_dict(r) for r in records]}

    async def apply_proposal(self, proposal_id: str) -> Dict[str, Any]:
        record = self.proposals.get(proposal_id)
        if record is None:
            return {"error": {"code": "PROPOSAL_NOT_FOUND", "message": "proposal not found"}}
        if record.status != "pending":
            return {"error": {"code": "PROPOSAL_INVALID_STATE", "message": f"proposal already {record.status}"}}

        applied: List[str] = []
        try:
            for change in record.changes:
                if not isinstance(change, dict):
                    continue
                path = str(change.get("path") or "").strip()
                content = str(change.get("proposedContent") or "")
                if not path:
                    continue
                self.workspace_tree.write_text(path, content)
                applied.append(path)
        except (ValueError, OSError, IsADirectoryError) as exc:
            self.proposals.mark_apply_failed(proposal_id, str(exc))
            await self.runtime.publish(
                record.session_id,
                "agent.code_edit.failed",
                "agent",
                proposal_id,
                {"proposalId": proposal_id, "code": "PROPOSAL_APPLY_FAILED", "message": str(exc)},
            )
            return {"error": {"code": "PROPOSAL_APPLY_FAILED", "message": str(exc)}}

        updated = self.proposals.mark_applied(proposal_id, applied)
        await self.runtime.publish(
            record.session_id,
            "agent.code_edit.applied",
            "agent",
            proposal_id,
            {
                "proposalId": proposal_id,
                "appliedAt": utc_now_iso(),
                "appliedFiles": applied,
            },
        )
        return {"proposal": proposal_to_dict(updated) if updated is not None else None}

    async def discard_proposal(self, proposal_id: str) -> Dict[str, Any]:
        record = self.proposals.get(proposal_id)
        if record is None:
            return {"error": {"code": "PROPOSAL_NOT_FOUND", "message": "proposal not found"}}
        updated = self.proposals.mark_discarded(proposal_id)
        await self.runtime.publish(
            record.session_id,
            "agent.code_edit.discarded",
            "agent",
            proposal_id,
            {"proposalId": proposal_id, "discardedAt": utc_now_iso()},
        )
        return {"proposal": proposal_to_dict(updated) if updated is not None else None}

    # ============================================================ replay

    def get_replay(self, session_id: str) -> Dict[str, Any]:
        return {"events": self._full_session_events(session_id)}

    def replay_since(self, session_id: str, from_seq: int, limit: int | None = None) -> Dict[str, Any]:
        full = self._full_session_events(session_id)
        out = [e for e in full if int(e.get("seq") or 0) > from_seq]
        if limit is not None:
            out = out[:limit]
        return {
            "events": out,
            "gap": self._replay_gap_for_from_seq(session_id, from_seq),
            "latestSeq": self.event_bus.latest_seq(session_id),
        }

    # ============================================================ swarm / workspace / mcp / tools

    async def list_swarm_state(self) -> Dict[str, Any]:
        return {
            "nodes": [asdict_safe(node) for node in self.swarm.list_nodes()],
            "shards": [asdict_safe(shard) for shard in self.swarm.list_shards()],
        }

    async def seed_swarm_demo(self) -> Dict[str, Any]:
        data = self.swarm.seed_demo()
        for node in data["nodes"]:
            await self.runtime.publish(
                "swarm_demo",
                "swarm.node.joined",
                "swarm",
                node.node_id,
                asdict_safe(node),
            )
        for shard in data["shards"]:
            await self.runtime.publish(
                "swarm_demo",
                "swarm.shard.assigned" if shard.status == "assigned" else "swarm.shard.reassigned",
                "swarm",
                shard.id,
                asdict_safe(shard),
            )
        return {
            "nodes": [asdict_safe(node) for node in data["nodes"]],
            "shards": [asdict_safe(shard) for shard in data["shards"]],
        }

    def get_workspace_info(self) -> Dict[str, Any]:
        return self.workspace_tree.info()

    def set_workspace_root(self, path: str) -> Dict[str, Any]:
        if not path or not isinstance(path, str):
            return {"error": {"code": "INVALID_PATH", "message": "path 必须是非空字符串"}}
        try:
            return self.workspace_tree.set_root(path)
        except ValueError as exc:
            return {"error": {"code": "INVALID_PATH", "message": str(exc)}}
        except OSError as exc:
            return {"error": {"code": "FILESYSTEM_ERROR", "message": str(exc)}}

    def get_workspace_tree(self, rel_path: str = "", show_heavy: bool = False) -> Dict[str, Any]:
        try:
            return self.workspace_tree.list(rel_path or "", show_heavy=show_heavy)
        except ValueError as exc:
            return {"error": {"code": "PATH_OUTSIDE_ROOT", "message": str(exc)}}
        except FileNotFoundError as exc:
            return {"error": {"code": "PATH_NOT_FOUND", "message": str(exc)}}
        except NotADirectoryError as exc:
            return {"error": {"code": "PATH_NOT_DIRECTORY", "message": str(exc)}}

    def browse_directories(self, path: str = "") -> Dict[str, Any]:
        """List sub-directories of an arbitrary local path for the workspace
        picker. Not bound to the current workspace root (see
        ``workspace_tree.browse_directories``)."""
        try:
            return browse_directories(path or "")
        except ValueError as exc:
            return {"error": {"code": "INVALID_PATH", "message": str(exc)}}
        except FileNotFoundError as exc:
            return {"error": {"code": "PATH_NOT_FOUND", "message": str(exc)}}
        except NotADirectoryError as exc:
            return {"error": {"code": "PATH_NOT_DIRECTORY", "message": str(exc)}}

    def read_workspace_file(self, path: str) -> Dict[str, Any]:
        if not path:
            return {"error": {"code": "INVALID_PATH", "message": "path 必须是非空字符串"}}
        try:
            return self.workspace_tree.read_text(path)
        except ValueError as exc:
            return {"error": {"code": "PATH_OUTSIDE_ROOT", "message": str(exc)}}
        except FileNotFoundError as exc:
            return {"error": {"code": "PATH_NOT_FOUND", "message": str(exc)}}
        except IsADirectoryError as exc:
            return {"error": {"code": "PATH_IS_DIRECTORY", "message": str(exc)}}
        except OSError as exc:
            return {"error": {"code": "FILESYSTEM_ERROR", "message": str(exc)}}

    def read_local_file(self, path: str) -> Dict[str, Any]:
        if not path:
            return {"error": {"code": "INVALID_PATH", "message": "path 必须是非空字符串"}}
        try:
            return self.workspace_tree.read_local_text(path)
        except ValueError as exc:
            return {"error": {"code": "INVALID_PATH", "message": str(exc)}}
        except FileNotFoundError as exc:
            return {"error": {"code": "PATH_NOT_FOUND", "message": str(exc)}}
        except IsADirectoryError as exc:
            return {"error": {"code": "PATH_IS_DIRECTORY", "message": str(exc)}}
        except OSError as exc:
            return {"error": {"code": "FILESYSTEM_ERROR", "message": str(exc)}}

    def write_workspace_file(self, path: str, content: str) -> Dict[str, Any]:
        if not path:
            return {"error": {"code": "INVALID_PATH", "message": "path 必须是非空字符串"}}
        try:
            return self.workspace_tree.write_text(path, content or "")
        except ValueError as exc:
            return {"error": {"code": "PATH_OUTSIDE_ROOT", "message": str(exc)}}
        except IsADirectoryError as exc:
            return {"error": {"code": "PATH_IS_DIRECTORY", "message": str(exc)}}
        except OSError as exc:
            return {"error": {"code": "FILESYSTEM_ERROR", "message": str(exc)}}

    def write_local_file(self, path: str, content: str) -> Dict[str, Any]:
        if not path:
            return {"error": {"code": "INVALID_PATH", "message": "path 必须是非空字符串"}}
        try:
            return self.workspace_tree.write_local_text(path, content or "")
        except ValueError as exc:
            return {"error": {"code": "INVALID_PATH", "message": str(exc)}}
        except IsADirectoryError as exc:
            return {"error": {"code": "PATH_IS_DIRECTORY", "message": str(exc)}}
        except OSError as exc:
            return {"error": {"code": "FILESYSTEM_ERROR", "message": str(exc)}}

    def revert_workspace_file(self, path: str) -> Dict[str, Any]:
        """Revert ``path`` in the workspace to its git ``HEAD`` content.

        Wraps `WorkspaceTree.checkout_head` and maps domain exceptions to the
        REST error envelope used elsewhere in this gateway.
        """
        if not path:
            return {"error": {"code": "INVALID_PATH", "message": "path 必须是非空字符串"}}
        try:
            return self.workspace_tree.checkout_head(path)
        except ValueError as exc:
            return {"error": {"code": "PATH_OUTSIDE_ROOT", "message": str(exc)}}
        except IsADirectoryError as exc:
            return {"error": {"code": "PATH_IS_DIRECTORY", "message": str(exc)}}
        except NotAGitRepoError as exc:
            return {"error": {"code": "NOT_A_GIT_REPO", "message": str(exc)}}
        except OSError as exc:
            return {"error": {"code": "GIT_ERROR", "message": str(exc)}}

    def list_tools(self) -> Dict[str, Any]:
        registry = self.tool_registry
        return {
            "items": [
                {
                    "name": name,
                    "description": registry.get(name).description,
                    "parameters": registry.get(name).parameters,
                }
                for name in registry.names()
            ],
        }

    @staticmethod
    def _tool_choice_for_user_input(user_input: str, allowed_tools: List[str]) -> str | None:
        if "web_search" not in set(allowed_tools):
            return None
        text = (user_input or "").strip().lower()
        if not text:
            return None
        explicit_web_patterns = (
            r"联网搜索",
            r"上网搜索",
            r"search the web",
            r"\bweb search\b",
            r"搜索一下",
            r"搜一下",
            r"查一下",
            r"查官网",
            r"官网地址",
            r"官方文档",
            r"最新信息",
            r"最新消息",
        )
        if any(re.search(pattern, text) for pattern in explicit_web_patterns):
            return "web_search"
        return None

    def list_skills(self) -> Dict[str, Any]:
        root = self.workspace_tree.root
        records = discover_skill_records(root)
        return {
            "items": [
                {
                    "name": record.name,
                    "summary": record.summary,
                    "scope": record.scope,
                    "path": record.path,
                }
                for record in records
            ],
        }

    def read_skill_content(self, name: str) -> Dict[str, Any]:
        root = self.workspace_tree.root
        path = resolve_skill_path(root, name)
        if path is None:
            return {"error": {"code": "SKILL_NOT_FOUND", "message": f"unknown skill: {name}"}}
        try:
            content = Path(path).read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            return {"error": {"code": "FILESYSTEM_ERROR", "message": str(exc)}}
        return {"skill": {"name": name, "path": path, "content": content}}

    async def mcp_demo_status(self) -> Dict[str, Any]:
        from src.agent_debug.mcp import demo_service

        return await demo_service.mcp_demo_status()

    async def mcp_demo_call(self, name: str, arguments: Dict[str, Any] | None) -> Dict[str, Any]:
        from src.agent_debug.mcp import demo_service

        return await demo_service.mcp_demo_call(name, arguments)

    # ============================================================ chat / composer

    async def _ask_composer_message(
        self,
        session_id: str,
        user_input: str,
        context_window: Dict[str, Any] | None = None,
        composer_mode: str | None = None,
    ) -> Dict[str, Any]:
        if isinstance(context_window, dict):
            self.session_context_windows[session_id] = context_window

        runtime_mode = normalize_composer_runtime_mode(composer_mode)
        chat_mode = normalize_composer_chat_mode(composer_mode)
        profile = resolve_composer_profile(runtime_mode)
        session_allowed_tools = self.allowed_tool_names_for_session(session_id)
        allowed_tools = [name for name in profile.allowed_tools if name in session_allowed_tools]
        tool_choice_override = self._tool_choice_for_user_input(user_input, allowed_tools)
        root = self.workspace_tree.root
        agent_md = read_agent_md_snippet(root)
        skill_items = discover_workspace_skills(root)
        system_content = build_chat_system_content(
            chat_mode,
            context_window,
            workspace_root=root,
            agent_md_text=agent_md,
            workspace_info=self.workspace_tree.info(),
            skill_items=skill_items,
        )
        runtime_tool_lines = [
            "## Runtime tool access",
            "Treat this list as the source of truth for which tools are directly available in this run.",
        ]
        if allowed_tools:
            rendered_tools = ", ".join(f"`{name}`" for name in allowed_tools)
            runtime_tool_lines.append(f"- Direct tools available now: {rendered_tools}")
        else:
            runtime_tool_lines.append("- No tools are available in this run.")
        runtime_tool_lines.append("- If the user asks whether a tool is available, answer strictly from this list.")
        if "web_search" in allowed_tools or "web_fetch" in allowed_tools:
            runtime_tool_lines.append(
                "- `web_search` and `web_fetch` are directly available in this run; do not claim that web search is unavailable or only accessible through `Task` / a researcher sub-agent."
            )
            runtime_tool_lines.append(
                "- When the user asks to search the web, fetch a URL, check current information, or find official docs, prefer `web_search` / `web_fetch` before exploring local workspace files."
            )
        system_content = f"{system_content}\n\n" + "\n".join(runtime_tool_lines)

        # 日常对话（build/debug/ask）进入会话级带工具的流式 ReAct 循环：
        # 复用 runtime._run_react_loop，从而产出 agent.token.stream.delta /
        # agent.reasoning.delta / agent.tool.* / agent.completed 等事件，让对话栏
        # 能像 Proma 一样实时渲染流式文本、思考与工具/MCP 卡片。
        out = await self.runtime.run_composer_chat(
            session_id=session_id,
            user_message=user_input,
            system_message=system_content,
            timeout_ms=profile.timeout_ms,
            allowed_tools_override=allowed_tools,
            tool_choice_override=tool_choice_override,
        )
        run_id = str((out.get("run") or {}).get("id") or "")
        run = self.runtime.get_run(run_id) if run_id else None
        run_status = getattr(run, "status", "")
        if run_status in {"paused", "failed", "cancelled"} and run_id:
            self.sessions.update_active_run(session_id, run_id, status=run_status)
        else:
            self.sessions.update_active_run(session_id, None, status="idle")
        return out

    async def ask_execute(
        self,
        session_id: str,
        user_input: str,
        context_window: Dict[str, Any] | None = None,
        composer_mode: str | None = None,
    ) -> Dict[str, Any]:
        self.sessions.auto_title_from_input(session_id, user_input)
        runtime_mode = normalize_composer_runtime_mode(composer_mode)
        if composer_mode is not None and runtime_mode != "plan":
            out = await self._ask_composer_message(
                session_id,
                user_input,
                context_window=context_window,
                composer_mode=composer_mode,
            )
            return {**out, "composerMode": runtime_mode}

        # Composer plan mode: generate a reviewable plan and wait for the user
        # to call ``plan:execute``. Legacy callers that omit ``composerMode``
        # still get the historical plan+auto-execute behaviour.
        plan_bundle = await self.generate_plan(session_id, user_input, context_window=context_window)
        rendered = self._render_plan_bundle(plan_bundle)
        if composer_mode is not None and runtime_mode == "plan":
            return {**rendered, "composerMode": "plan"}

        plan_id = str(plan_bundle.get("plan", {}).get("id", ""))
        run_result = await self.execute_plan(plan_id)
        return {
            **rendered,
            "run": run_result.get("run"),
            "outcome": run_result.get("outcome"),
            "composerMode": "plan",
        }

    # ============================================================ helpers

    def resolve_model_for_session(self, session_id: str) -> str:
        session = self.sessions.get(session_id)
        session_override = session.selected_model_id if session else None
        return self.model_catalog.resolve_model(session_override)

    def context_budget_for_session(self, session_id: str) -> int | None:
        session = self.sessions.get(session_id)
        session_override = session.selected_model_id if session else None
        model_id = self.model_catalog.resolve_model(session_override)
        return self.model_catalog.context_window_tokens(model_id)

    def _render_plan_bundle(self, bundle: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "plan": asdict_safe(bundle["plan"]),
            "version": asdict_safe(bundle["version"]),
            "stages": [asdict_safe(item) for item in bundle["stages"]],
            "tasks": [asdict_safe(item) for item in bundle["tasks"]],
            "steps": [asdict_safe(item) for item in bundle["steps"]],
            "assumptions": bundle.get("assumptions", []),
            "risks": bundle.get("risks", []),
            "openQuestions": bundle.get("openQuestions", []),
        }

    def _render_plan_snapshot(self, snapshot: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "plan": asdict_safe(snapshot["plan"]),
            "stages": [asdict_safe(item) for item in snapshot["stages"]],
            "tasks": [asdict_safe(item) for item in snapshot["tasks"]],
            "steps": [asdict_safe(item) for item in snapshot["steps"]],
            "versions": [asdict_safe(item) for item in snapshot["versions"]],
        }

    def _build_design_metrics(
        self,
        plan_bundle: Dict[str, Any] | None,
        todos: list[dict[str, Any]],
        events: list[dict[str, Any]],
        swarm_nodes: list[dict[str, Any]],
        context_window: Dict[str, Any] | None,
    ) -> Dict[str, Any]:
        token_events = [
            event.get("payload", {})
            for event in events
            if isinstance(event.get("payload"), dict)
        ]
        token_total = sum(
            int(payload.get("tokenUsage", {}).get("total", 0))
            for payload in token_events
            if isinstance(payload.get("tokenUsage"), dict)
        )
        completed_todos = [item for item in todos if item.get("status") in {"completed", "rolledUp", "rolled_up"}]
        total_plan_nodes = len(plan_bundle.get("tasks", [])) if plan_bundle else 0
        completed_plan_nodes = len(
            [item for item in plan_bundle.get("tasks", []) if item.get("status") in {"completed", "summarized"}]
        ) if plan_bundle else 0
        return {
            "totalTokens": token_total,
            "toolCalls": len([event for event in events if event.get("source", {}).get("domain") in {"tool", "agent"}]),
            "filesTouched": len(self._extract_design_diffs(events)),
            "avgLatencyMs": 0,
            "planProgress": {
                "completed": completed_plan_nodes,
                "total": total_plan_nodes,
            },
            "todos": {
                "completed": len(completed_todos),
                "total": len(todos),
            },
            "subagents": len(swarm_nodes),
            "contextFillPct": self._estimate_context_fill_pct(context_window),
        }

    def _estimate_context_fill_pct(self, context_window: Dict[str, Any] | None) -> int:
        if not isinstance(context_window, dict):
            return 0

        text_parts: list[str] = []
        active_file = context_window.get("activeFile")
        if isinstance(active_file, dict):
            text_parts.append(str(active_file.get("content") or ""))

        selection = context_window.get("selection")
        if isinstance(selection, dict):
            text_parts.append(str(selection.get("selectedText") or ""))

        terminal_output = context_window.get("terminalRecentOutput")
        if terminal_output:
            text_parts.append(str(terminal_output))

        relevant_logs = context_window.get("relevantLogs")
        if isinstance(relevant_logs, list):
            text_parts.extend(str(item) for item in relevant_logs)

        open_files = context_window.get("openFiles")
        if isinstance(open_files, list):
            text_parts.extend(str(item) for item in open_files)

        estimated_tokens = max(0, sum(len(part) for part in text_parts) // 4)
        if estimated_tokens == 0:
            return 0
        return max(1, min(100, round((estimated_tokens / 32000) * 100)))

    def _extract_design_diffs(self, events: list[dict[str, Any]]) -> list[dict[str, Any]]:
        diffs: list[dict[str, Any]] = []
        for event in events:
            if event.get("type") != "agent.code_edit.proposed":
                continue
            payload = event.get("payload", {})
            if not isinstance(payload, dict):
                continue
            for change in payload.get("changes", []):
                if not isinstance(change, dict):
                    continue
                original = str(change.get("originalContent") or "")
                proposed = str(change.get("proposedContent") or "")
                diffs.append(
                    {
                        "id": change.get("changeId") or payload.get("proposalId"),
                        "proposalId": payload.get("proposalId"),
                        "path": change.get("path", "unknown"),
                        "description": change.get("description", payload.get("summary", "")),
                        "plus": max(0, len(proposed.splitlines()) - len(original.splitlines())),
                        "minus": max(0, len(original.splitlines()) - len(proposed.splitlines())),
                        "state": "pending",
                        "at": event.get("ts", ""),
                        "by": "agent",
                        "originalContent": original,
                        "proposedContent": proposed,
                    }
                )
        return diffs
