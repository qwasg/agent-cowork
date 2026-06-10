from __future__ import annotations

import json
import logging
import os
from typing import Any, Dict, Optional

from src.agent_debug.dev_llm_defaults import apply_dev_llm_defaults

apply_dev_llm_defaults()

from fastapi import FastAPI, HTTPException, Request, WebSocket, WebSocketDisconnect
from fastapi.middleware.cors import CORSMiddleware
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import JSONResponse, StreamingResponse
import uvicorn
import asyncio

from src.agent_debug.api.rest_gateway import AgentDebugRestGateway
from src.agent_debug.api.ws_gateway import AgentDebugWsGateway
from src.agent_debug.domain.auth_service import AuthService
from src.agent_debug.service_registry import get_agent_debug_gateway, get_auth_service


logger = logging.getLogger(__name__)


# P9 v2 W9 (M9.6, D9-12) — Bearer-token authentication middleware.
#
# Two credentials are accepted on ``Authorization: Bearer <X>``:
#   1. The historic static service token ``AGENT_DEBUG_AUTH_TOKEN`` (automation).
#   2. A per-account JWT minted by :class:`AuthService` (the login flow).
#
# Whenever a bearer is present it is verified; an invalid/expired one always
# returns 401. Whether a *missing* bearer is rejected depends on enforcement:
# protected paths require a credential only when the static token is set OR
# ``AGENT_DEBUG_REQUIRE_AUTH=1``. The default local-dev workflow (neither set)
# keeps passing token-less requests so existing flows / tests are unaffected.
# ``OPTIONS`` (CORS preflight carries no auth header), ``/health`` and the
# login/register endpoints are always open.
class _AgentDebugAuthMiddleware(BaseHTTPMiddleware):
    _OPEN_PATHS = (
        "/health",
        "/api/agent-debug/auth/login",
        "/api/agent-debug/auth/register",
    )

    def __init__(
        self,
        app,
        token: Optional[str],
        auth: Optional[AuthService] = None,
        require_auth: bool = False,
    ) -> None:
        super().__init__(app)
        self._token = token or ""
        self._auth = auth
        # 强校验：静态服务令牌已配置，或显式开启 REQUIRE_AUTH。
        self._require_auth = bool(require_auth or self._token)

    async def dispatch(self, request: Request, call_next):
        # CORS 预检不携带 Authorization 头，必须放行。
        if request.method == "OPTIONS":
            return await call_next(request)
        if any(request.url.path.startswith(p) for p in self._OPEN_PATHS):
            return await call_next(request)
        header = request.headers.get("authorization", "")
        bearer = header[len("Bearer "):].strip() if header.startswith("Bearer ") else ""
        if bearer:
            # 1) 历史静态服务令牌
            if self._token and bearer == self._token:
                return await call_next(request)
            # 2) 账户 JWT
            user = self._auth.user_from_token(bearer) if self._auth else None
            if user is not None:
                request.state.user = user
                return await call_next(request)
            return JSONResponse(
                status_code=401,
                content={"error": {"code": "AUTH_INVALID", "message": "Invalid or expired token"}},
            )
        if self._require_auth:
            return JSONResponse(
                status_code=401,
                content={"error": {"code": "AUTH_MISSING", "message": "Authentication required"}},
            )
        return await call_next(request)


def _validate_cors_allow_origins(allow_origins_raw: str) -> list[str]:
    """P9 v2 W9 (M9.6, D9-12) — narrow CORS origins.

    Reject the wildcard ``*`` whenever ``allow_credentials=True`` is
    paired with it (browser ignores the response anyway and it is a
    common foot-gun). Empty / commented entries are stripped.
    """
    parsed = [o.strip() for o in allow_origins_raw.split(",") if o.strip()]
    for origin in parsed:
        if origin == "*":
            raise RuntimeError(
                "AGENT_DEBUG_ALLOW_ORIGINS contains '*' which is incompatible with"
                " allow_credentials=True; list explicit origins instead."
            )
    return parsed


def _maybe_raise(payload: Dict[str, Any]) -> Dict[str, Any]:
    """Translate gateway error envelopes to HTTP exceptions."""
    if not isinstance(payload, dict):
        return payload
    err = payload.get("error")
    if not isinstance(err, dict):
        return payload
    code = err.get("code") or ""
    if code in {
        "SESSION_NOT_FOUND",
        "PLAN_NOT_FOUND",
        "TODO_NOT_FOUND",
        "RUN_NOT_FOUND",
        "PLAN_NODE_NOT_FOUND",
        "PROPOSAL_NOT_FOUND",
        "PATH_NOT_FOUND",
        "PATH_NOT_DIRECTORY",
        "MODEL_NOT_FOUND",
        "AUTH_USER_NOT_FOUND",
        "SKILL_NOT_FOUND",
    }:
        raise HTTPException(status_code=404, detail=payload)
    if code in {
        "INVALID_TITLE",
        "INVALID_PATH",
        "PATH_OUTSIDE_ROOT",
        "PATH_IS_DIRECTORY",
        "TODO_INVALID",
        "PLAN_INVALID_STATE",
        "PROPOSAL_INVALID_STATE",
        "AUTH_INVALID_INPUT",
    }:
        raise HTTPException(status_code=400, detail=payload)
    if code in {"AUTH_BAD_CREDENTIALS", "AUTH_MISSING", "AUTH_INVALID"}:
        raise HTTPException(status_code=401, detail=payload)
    if code in {"PROPOSAL_APPLY_FAILED", "FILESYSTEM_ERROR", "GIT_ERROR"}:
        raise HTTPException(status_code=500, detail=payload)
    if code in {"NOT_A_GIT_REPO", "AUTH_EMAIL_TAKEN"}:
        # 409 Conflict — the request is well-formed but conflicts with the
        # current state (no git repo / email already registered).
        raise HTTPException(status_code=409, detail=payload)
    return payload


def _sse_frame(event: Dict[str, Any]) -> str:
    seq = event.get("seq", "")
    etype = event.get("type", "message")
    data = json.dumps(event, ensure_ascii=False)
    return f"id: {seq}\nevent: {etype}\ndata: {data}\n\n"


async def _sse_event_source(gateway: "AgentDebugRestGateway", session_id: str, from_seq: int):
    """SSE 事件流：先回放 ``fromSeq`` 之后的历史，再订阅实时事件并心跳保活。"""
    from src.agent_debug.domain.models import asdict_safe

    bus = gateway.event_bus
    backlog, _gap = bus.replay_since(session_id, from_seq)
    for ev in backlog:
        yield _sse_frame(ev)

    queue: asyncio.Queue = asyncio.Queue()

    async def _listener(event) -> None:
        if event.session_id == session_id:
            await queue.put(event)

    dispose = bus.subscribe(_listener)
    try:
        while True:
            try:
                event = await asyncio.wait_for(queue.get(), timeout=15.0)
                yield _sse_frame(asdict_safe(event))
            except asyncio.TimeoutError:
                yield ": keep-alive\n\n"
    finally:
        dispose()


class AgentDebugService:
    def __init__(
        self,
        gateway: AgentDebugRestGateway | None = None,
        auth: AuthService | None = None,
    ) -> None:
        self.gateway = gateway or get_agent_debug_gateway()
        self.auth = auth or get_auth_service()
        self.ws_gateway = AgentDebugWsGateway(self.gateway.event_bus)
        self.app = FastAPI(title="Agent Debug Service")
        allow_origins = os.getenv(
            "AGENT_DEBUG_ALLOW_ORIGINS",
            ",".join(
                [
                    # Document Compiler copy (avoid upstream IDE defaults 3000 / 8001).
                    "http://127.0.0.1:8030",
                    "http://localhost:8030",
                    "tauri://localhost",
                    "https://tauri.localhost",
                ]
            ),
        )
        # P9 v2 W9 (M9.6, D9-12) — CORS hardening:
        # 1. Reject wildcard origin when paired with allow_credentials.
        # 2. Replace allow_methods=["*"] with the actual method set the
        #    REST gateway exposes (GET / POST / DELETE / OPTIONS).
        # 3. Replace allow_headers=["*"] with the explicit content-type +
        #    authorization header that the IDE / Tauri client uses.
        validated_origins = _validate_cors_allow_origins(allow_origins)
        self.app.add_middleware(
            CORSMiddleware,
            allow_origins=validated_origins,
            allow_credentials=True,
            allow_methods=["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
            allow_headers=["Content-Type", "Authorization"],
        )
        # P9 v2 W9 (M9.6, D9-12) — bearer-token middleware. The env var
        # ``AGENT_DEBUG_AUTH_TOKEN`` is read here so changes at request
        # time would require a service restart; this matches the rest of
        # the agent-debug start-up env contract.
        self._auth_token = os.getenv("AGENT_DEBUG_AUTH_TOKEN", "").strip()
        # ``AGENT_DEBUG_REQUIRE_AUTH=1`` 强制账户登录（受保护路由必须携带有效 JWT）；
        # 默认关闭，缺令牌的请求仍放行以兼容既有本地流程 / 测试。
        require_auth = os.getenv("AGENT_DEBUG_REQUIRE_AUTH", "").strip() in {"1", "true", "True"}
        self.app.add_middleware(
            _AgentDebugAuthMiddleware,
            token=self._auth_token,
            auth=self.auth,
            require_auth=require_auth,
        )
        self._register_routes()

    def _current_user(self, request: Request) -> Optional[Dict[str, Any]]:
        """从中间件注入的 ``request.state.user`` 或 Authorization 头解析当前用户。"""
        user = getattr(request.state, "user", None)
        if user:
            return user
        header = request.headers.get("authorization", "")
        if header.startswith("Bearer "):
            return self.auth.user_from_token(header[len("Bearer "):].strip())
        return None

    def _register_routes(self) -> None:
        app = self.app
        gateway = self.gateway
        ws_gateway = self.ws_gateway

        @app.get("/health")
        async def health() -> Dict[str, Any]:
            return {"ok": True}

        # ============================================== auth / accounts

        @app.post("/api/agent-debug/auth/register")
        async def auth_register(payload: Dict[str, Any]) -> Dict[str, Any]:
            data = payload or {}
            return _maybe_raise(self.auth.register(
                str(data.get("email", "") or ""),
                str(data.get("password", "") or ""),
                str(data.get("displayName", "") or ""),
                str(data.get("workspace", "") or ""),
            ))

        @app.post("/api/agent-debug/auth/login")
        async def auth_login(payload: Dict[str, Any]) -> Dict[str, Any]:
            data = payload or {}
            return _maybe_raise(self.auth.login(
                str(data.get("email", "") or ""),
                str(data.get("password", "") or ""),
            ))

        @app.get("/api/agent-debug/auth/me")
        async def auth_me(request: Request) -> Dict[str, Any]:
            user = self._current_user(request)
            if not user:
                return _maybe_raise({"error": {"code": "AUTH_INVALID", "message": "Invalid or expired token"}})
            return {"user": self.auth.public_user(user)}

        @app.patch("/api/agent-debug/auth/profile")
        async def auth_update_profile(request: Request, payload: Dict[str, Any]) -> Dict[str, Any]:
            user = self._current_user(request)
            if not user:
                return _maybe_raise({"error": {"code": "AUTH_INVALID", "message": "Invalid or expired token"}})
            return _maybe_raise(self.auth.update_profile(str(user.get("id") or ""), payload or {}))

        @app.post("/api/agent-debug/auth/logout")
        async def auth_logout() -> Dict[str, Any]:
            # 无状态 JWT：服务端无需失效，客户端丢弃令牌即可。
            return {"ok": True}

        # ============================================== sessions

        @app.get("/api/agent-debug/sessions")
        async def list_sessions() -> Dict[str, Any]:
            return gateway.list_sessions()

        @app.get("/api/agent-debug/design-snapshot")
        async def get_design_snapshot(sessionId: str | None = None) -> Dict[str, Any]:
            return gateway.get_design_snapshot(sessionId)

        @app.post("/api/agent-debug/sessions")
        async def create_session(payload: Dict[str, Any]) -> Dict[str, Any]:
            return gateway.create_session(
                payload.get("title", "Agent Debug Session"),
                payload.get("selectedModelId"),
                bool(payload.get("webSearchEnabled", False)),
            )

        @app.get("/api/agent-debug/sessions/{session_id}")
        async def get_session(session_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.get_session(session_id))

        @app.patch("/api/agent-debug/sessions/{session_id}")
        async def patch_session(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.patch_session(session_id, payload))

        @app.delete("/api/agent-debug/sessions/{session_id}")
        async def delete_session(session_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.delete_session(session_id))

        @app.post("/api/agent-debug/sessions/{session_id}:fork")
        async def fork_session(session_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.fork_session(session_id))

        @app.post("/api/agent-debug/sessions/{session_id}:revert")
        async def revert_session(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(
                gateway.revert_session(
                    session_id,
                    payload.get("messageId"),
                    mode=payload.get("mode"),
                )
            )

        @app.put("/api/agent-debug/sessions/{session_id}/model")
        async def set_session_model(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.set_session_model(session_id, payload.get("modelId")))

        @app.get("/api/agent-debug/models")
        async def list_models() -> Dict[str, Any]:
            return gateway.list_models()

        @app.get("/api/agent-debug/provider-status")
        async def get_provider_status() -> Dict[str, Any]:
            return gateway.get_provider_status()

        @app.get("/api/agent-debug/search-config")
        async def get_search_config() -> Dict[str, Any]:
            return gateway.get_search_config()

        @app.put("/api/agent-debug/search-config")
        async def set_search_config(payload: Dict[str, Any]) -> Dict[str, Any]:
            return gateway.set_search_config(payload)

        @app.get("/api/agent-debug/model-preferences")
        async def get_model_preferences() -> Dict[str, Any]:
            return gateway.get_model_preferences()

        @app.put("/api/agent-debug/model-preferences")
        async def set_model_preferences(payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.set_model_preferences(str(payload.get("modelId", ""))))

        # ============================================== channels (多渠道 / 中国大模型)

        @app.get("/api/agent-debug/provider-types")
        async def list_provider_types() -> Dict[str, Any]:
            return gateway.list_provider_types()

        @app.get("/api/agent-debug/channels")
        async def list_channels() -> Dict[str, Any]:
            return gateway.list_channels()

        @app.post("/api/agent-debug/channels:fetch-models")
        async def fetch_channel_models(payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.fetch_channel_models(payload))

        @app.post("/api/agent-debug/channels")
        async def create_channel(payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.upsert_channel(payload))

        @app.put("/api/agent-debug/channels/{channel_id}")
        async def update_channel(channel_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            data = dict(payload or {})
            data["id"] = channel_id
            return _maybe_raise(gateway.upsert_channel(data))

        @app.delete("/api/agent-debug/channels/{channel_id}")
        async def delete_channel(channel_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.delete_channel(channel_id))

        # ============================================== permissions / subagents

        @app.get("/api/agent-debug/sessions/{session_id}/permission-mode")
        async def get_permission_mode(session_id: str) -> Dict[str, Any]:
            return gateway.get_permission_mode(session_id)

        @app.put("/api/agent-debug/sessions/{session_id}/permission-mode")
        async def set_permission_mode(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.set_permission_mode(session_id, str(payload.get("mode", ""))))

        @app.get("/api/agent-debug/subagents")
        async def list_builtin_subagents() -> Dict[str, Any]:
            return gateway.list_builtin_subagents()

        @app.get("/api/agent-debug/skills")
        async def list_skills() -> Dict[str, Any]:
            return gateway.list_skills()

        @app.get("/api/agent-debug/skills/{name}")
        async def read_skill_content(name: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.read_skill_content(name))

        # ============================================== checkpoints / rewind

        @app.get("/api/agent-debug/sessions/{session_id}/checkpoints")
        async def list_checkpoints(session_id: str) -> Dict[str, Any]:
            return gateway.list_checkpoints(session_id)

        @app.post("/api/agent-debug/sessions/{session_id}/checkpoints")
        async def create_checkpoint(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return gateway.create_checkpoint(
                session_id,
                payload.get("paths") if isinstance(payload.get("paths"), list) else None,
                str(payload.get("label", "")),
            )

        @app.post("/api/agent-debug/checkpoints/{checkpoint_id}:rewind")
        async def rewind_checkpoint(checkpoint_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.rewind_checkpoint(checkpoint_id))

        # ============================================== SSE 实时事件流

        @app.get("/api/agent-debug/sessions/{session_id}/events/stream")
        async def stream_events(session_id: str, fromSeq: int = 0) -> StreamingResponse:
            return StreamingResponse(
                _sse_event_source(gateway, session_id, fromSeq),
                media_type="text/event-stream",
                headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
            )

        # ============================================== mcp demo

        @app.get("/api/agent-debug/mcp/demo/status")
        async def mcp_demo_status() -> Dict[str, Any]:
            out = await gateway.mcp_demo_status()
            err = out.get("error") or {}
            if err.get("code") in ("MCP_NOT_INSTALLED", "MCP_DEMO_SERVER_MISSING"):
                raise HTTPException(status_code=503, detail=out)
            if not out.get("ok", False):
                raise HTTPException(status_code=500, detail=out)
            return out

        @app.post("/api/agent-debug/mcp/demo/call")
        async def mcp_demo_call(payload: Dict[str, Any]) -> Dict[str, Any]:
            out = await gateway.mcp_demo_call(
                str(payload.get("name", "") or ""),
                payload.get("arguments") if isinstance(payload.get("arguments"), dict) else None,
            )
            err = out.get("error") or {}
            if err.get("code") in ("MCP_NOT_INSTALLED", "MCP_DEMO_SERVER_MISSING"):
                raise HTTPException(status_code=503, detail=out)
            if err.get("code") == "MCP_INVALID_REQUEST":
                raise HTTPException(status_code=400, detail=out)
            if not out.get("ok", False):
                if "error" in out:
                    raise HTTPException(status_code=400, detail=out)
                raise HTTPException(status_code=500, detail=out)
            return out

        # ============================================== chat / plan generation

        @app.post("/api/agent-debug/sessions/{session_id}/plan:generate")
        async def generate_plan(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return await gateway.generate_plan(
                session_id,
                payload.get("userInput", ""),
                payload.get("contextWindow"),
            )

        @app.post("/api/agent-debug/sessions/{session_id}/ask:execute")
        async def ask_execute(session_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return await gateway.ask_execute(
                session_id,
                payload.get("userInput", ""),
                payload.get("contextWindow"),
                payload.get("composerMode"),
            )

        # ============================================== plans

        @app.get("/api/agent-debug/plans/{plan_id}")
        async def get_plan(plan_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.get_plan(plan_id))

        @app.patch("/api/agent-debug/plans/{plan_id}")
        async def patch_plan(plan_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.patch_plan(plan_id, payload))

        @app.post("/api/agent-debug/plans/{plan_id}:confirm")
        async def confirm_plan(plan_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.confirm_plan(plan_id))

        @app.post("/api/agent-debug/plans/{plan_id}:replan")
        async def replan(plan_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(await gateway.replan(plan_id, payload))

        @app.post("/api/agent-debug/plans/{plan_id}:execute")
        async def execute_plan(plan_id: str) -> Dict[str, Any]:
            return _maybe_raise(await gateway.execute_plan(plan_id))

        # ============================================== todos

        @app.get("/api/agent-debug/sessions/{session_id}/todos")
        async def get_todos(session_id: str) -> Dict[str, Any]:
            return gateway.get_todos(session_id)

        @app.post("/api/agent-debug/todos")
        async def add_todo(payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.add_todo(payload))

        @app.patch("/api/agent-debug/todos/{todo_id}")
        async def patch_todo(todo_id: str, payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(gateway.patch_todo(todo_id, payload))

        @app.post("/api/agent-debug/todos:batch-rerun")
        async def batch_rerun_todos(payload: Dict[str, Any]) -> Dict[str, Any]:
            return _maybe_raise(await gateway.batch_rerun_todos(payload))

        # ============================================== runs

        @app.get("/api/agent-debug/runs/{run_id}")
        async def get_run(run_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.get_run(run_id))

        @app.get("/api/agent-debug/runs/{run_id}/logs")
        async def get_run_logs(run_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.get_run_logs(run_id))

        @app.get("/api/agent-debug/runs/{run_id}/metrics")
        async def get_run_metrics(run_id: str) -> Dict[str, Any]:
            return _maybe_raise(gateway.get_run_metrics(run_id))

        @app.post("/api/agent-debug/runs/{run_id}:pause")
        async def pause_run(run_id: str) -> Dict[str, Any]:
            return await gateway.pause_run(run_id)

        @app.post("/api/agent-debug/runs/{run_id}:resume")
        async def resume_run(run_id: str) -> Dict[str, Any]:
            return await gateway.resume_run(run_id)

        @app.post("/api/agent-debug/runs/{run_id}:cancel")
        async def cancel_run(run_id: str) -> Dict[str, Any]:
            return await gateway.cancel_run(run_id)

        @app.post("/api/agent-debug/runs/{run_id}/todos/{todo_id}:rerun")
        async def rerun_todo(run_id: str, todo_id: str) -> Dict[str, Any]:
            return _maybe_raise(await gateway.rerun_todo(run_id, todo_id))

        @app.post("/api/agent-debug/runs/{run_id}/nodes/{node_id}:rerun")
        async def rerun_node(run_id: str, node_id: str) -> Dict[str, Any]:
            return _maybe_raise(await gateway.rerun_node(run_id, node_id))

        # ============================================== proposals (apply / discard)

        @app.get("/api/agent-debug/proposals")
        async def list_proposals(sessionId: str = "") -> Dict[str, Any]:
            return _maybe_raise(gateway.list_proposals(sessionId))

        @app.post("/api/agent-debug/proposals/{proposal_id}:apply")
        async def apply_proposal(proposal_id: str) -> Dict[str, Any]:
            return _maybe_raise(await gateway.apply_proposal(proposal_id))

        @app.post("/api/agent-debug/proposals/{proposal_id}:discard")
        async def discard_proposal(proposal_id: str) -> Dict[str, Any]:
            return _maybe_raise(await gateway.discard_proposal(proposal_id))

        # ============================================== replay / swarm / workspace / tools

        @app.get("/api/agent-debug/replay/{session_id}")
        async def get_replay(session_id: str) -> Dict[str, Any]:
            return gateway.get_replay(session_id)

        @app.get("/api/agent-debug/replay/{session_id}/since")
        async def replay_since(session_id: str, fromSeq: int = 0, limit: int | None = None) -> Dict[str, Any]:
            return gateway.replay_since(session_id, int(fromSeq), limit)

        @app.get("/api/agent-debug/swarm/state")
        async def get_swarm_state() -> Dict[str, Any]:
            return await gateway.list_swarm_state()

        @app.post("/api/agent-debug/swarm/seed-demo")
        async def seed_swarm_demo() -> Dict[str, Any]:
            return await gateway.seed_swarm_demo()

        @app.get("/api/agent-debug/workspace/info")
        async def get_workspace_info() -> Dict[str, Any]:
            return gateway.get_workspace_info()

        @app.get("/api/agent-debug/workspace/tree")
        async def get_workspace_tree(path: str = "", showHeavy: bool = False) -> Dict[str, Any]:
            return _maybe_raise(gateway.get_workspace_tree(path, showHeavy))

        @app.get("/api/agent-debug/workspace/browse")
        async def browse_workspace_directories(path: str = "") -> Dict[str, Any]:
            return _maybe_raise(gateway.browse_directories(path))

        @app.post("/api/agent-debug/workspace/root")
        async def set_workspace_root(payload: Dict[str, Any]) -> Dict[str, Any]:
            path = (payload or {}).get("path") if isinstance(payload, dict) else None
            return _maybe_raise(gateway.set_workspace_root(path or ""))

        @app.get("/api/agent-debug/workspace/file")
        async def read_workspace_file(path: str = "") -> Dict[str, Any]:
            return _maybe_raise(gateway.read_workspace_file(path))

        @app.post("/api/agent-debug/workspace/file")
        async def write_workspace_file(payload: Dict[str, Any]) -> Dict[str, Any]:
            data = payload or {}
            return _maybe_raise(
                gateway.write_workspace_file(
                    str(data.get("path") or ""),
                    str(data.get("content") or ""),
                )
            )

        @app.get("/api/agent-debug/local-file")
        async def read_local_file(path: str = "") -> Dict[str, Any]:
            return _maybe_raise(gateway.read_local_file(path))

        @app.post("/api/agent-debug/local-file")
        async def write_local_file(payload: Dict[str, Any]) -> Dict[str, Any]:
            data = payload or {}
            return _maybe_raise(
                gateway.write_local_file(
                    str(data.get("path") or ""),
                    str(data.get("content") or ""),
                )
            )

        @app.post("/api/agent-debug/workspace/revert")
        async def revert_workspace_file(payload: Dict[str, Any]) -> Dict[str, Any]:
            data = payload or {}
            return _maybe_raise(
                gateway.revert_workspace_file(str(data.get("path") or ""))
            )

        @app.get("/api/agent-debug/tools")
        async def list_tools() -> Dict[str, Any]:
            return gateway.list_tools()

        # ============================================== websocket

        @app.websocket("/ws/agent-debug")
        async def agent_debug_ws(websocket: WebSocket) -> None:
            # P9 v2 W9 (M9.6, D9-12) — token-gated WS handshake. When the
            # auth token is non-empty the client must include it in the
            # subscribe payload (``token`` field) to bind the socket to a
            # principal; otherwise the connection is closed with code
            # 1008 (policy violation). Default empty-token behaviour
            # remains permissive for backward-compatible local dev.
            await websocket.accept()
            auth_token = self._auth_token
            try:
                while True:
                    raw_message = await websocket.receive_text()
                    try:
                        payload = json.loads(raw_message)
                    except json.JSONDecodeError:
                        continue
                    if not isinstance(payload, dict):
                        continue
                    if payload.get("action") == "subscribe":
                        if auth_token:
                            provided = str(payload.get("token", "")).strip()
                            if provided != auth_token:
                                logger.warning(
                                    "agent_debug WS subscribe rejected: bad/missing token"
                                )
                                await websocket.close(code=1008)
                                return
                        await ws_gateway.handle_subscribe(websocket, payload)
            except WebSocketDisconnect:
                await ws_gateway.disconnect(websocket)
            except Exception:
                await ws_gateway.disconnect(websocket)


service = AgentDebugService(gateway=get_agent_debug_gateway())
app = service.app


def _validate_bind_settings(host: str, auth_token: str, bind_public_env: str) -> None:
    """P9 v2 W9 (M9.6, D9-12) — fail-closed when binding beyond localhost.

    The historic default ``AGENT_DEBUG_HOST=127.0.0.1`` keeps working with
    no other env vars. When the operator deliberately exposes the service
    (e.g. ``AGENT_DEBUG_HOST=0.0.0.0``) BOTH of these must be set:

    - ``AGENT_DEBUG_BIND_PUBLIC=1`` (explicit acknowledgement)
    - ``AGENT_DEBUG_AUTH_TOKEN`` non-empty (the REST + WS surfaces are
      then token-gated by the middleware above).

    Otherwise startup raises so the service cannot silently drift to a
    LAN-reachable, unauthenticated state.
    """
    safe_local_hosts = {"127.0.0.1", "::1", "localhost"}
    if host in safe_local_hosts:
        return
    if bind_public_env != "1":
        raise RuntimeError(
            f"AGENT_DEBUG_HOST={host!r} is not localhost; set AGENT_DEBUG_BIND_PUBLIC=1"
            " to acknowledge the public bind."
        )
    if not auth_token:
        raise RuntimeError(
            f"AGENT_DEBUG_HOST={host!r} requires AGENT_DEBUG_AUTH_TOKEN to be set"
            " (public bind without authentication is forbidden)."
        )


def main() -> None:
    host = os.getenv("AGENT_DEBUG_HOST", "127.0.0.1")
    # Document Compiler copy defaults to 8002 (upstream Agent Engine IDE uses 8001).
    port = int(os.getenv("AGENT_DEBUG_HTTP_PORT", "8002"))
    transport_mode = os.getenv("AGENT_DEBUG_TRANSPORT_MODE", "dedicated-http-ws")
    if transport_mode != "dedicated-http-ws":
        raise RuntimeError("AGENT_DEBUG_TRANSPORT_MODE must be dedicated-http-ws")
    _validate_bind_settings(
        host,
        os.getenv("AGENT_DEBUG_AUTH_TOKEN", "").strip(),
        os.getenv("AGENT_DEBUG_BIND_PUBLIC", "").strip(),
    )
    uvicorn.run(app, host=host, port=port)


if __name__ == "__main__":
    main()
