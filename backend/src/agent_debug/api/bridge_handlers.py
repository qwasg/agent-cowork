from __future__ import annotations

from typing import Any, Awaitable, Callable, Dict

from src.agent_debug.domain.models import asdict_safe
from src.agent_debug.service_registry import get_agent_debug_gateway


def _resolve_gateway(gateway: Any = None):
    return gateway or get_agent_debug_gateway()


def _bind_gateway(
    handler: Callable[..., Awaitable[Dict[str, Any]]], gateway: Any
) -> Callable[[Dict[str, Any]], Awaitable[Dict[str, Any]]]:
    async def wrapped(params: Dict[str, Any]) -> Dict[str, Any]:
        return await handler(params, gateway=gateway)

    return wrapped


async def handle_agent_debug_session_create(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    title = params.get("title", "Agent Debug Session")
    return gateway.create_session(
        title,
        params.get("selectedModelId"),
        bool(params.get("webSearchEnabled", False)),
    )


async def handle_agent_debug_plan_generate(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    session_id = params.get("sessionId", "")
    user_input = params.get("userInput", "")
    return await gateway.generate_plan(session_id, user_input, params.get("contextWindow"))


async def handle_agent_debug_plan_execute(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    plan_id = params.get("planId", "")
    return await gateway.execute_plan(plan_id)


async def handle_agent_debug_ask_execute(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    session_id = params.get("sessionId", "")
    user_input = params.get("userInput", "")
    return await gateway.ask_execute(
        session_id,
        user_input,
        params.get("contextWindow"),
        params.get("composerMode"),
    )


async def handle_agent_debug_sessions_list(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return gateway.list_sessions()


async def handle_agent_debug_design_snapshot(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.get_design_snapshot(params.get("sessionId"))


async def handle_agent_debug_provider_status(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return gateway.get_provider_status()


async def handle_agent_debug_search_config_get(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return gateway.get_search_config()


async def handle_agent_debug_search_config_set(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.set_search_config(params.get("config") or params)


async def handle_agent_debug_plan_get(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.get_plan(params.get("planId", ""))


async def handle_agent_debug_plan_patch(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.patch_plan(params.get("planId", ""), params.get("patch") or {})


async def handle_agent_debug_plan_confirm(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.confirm_plan(params.get("planId", ""))


async def handle_agent_debug_plan_replan(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return await gateway.replan(
        params.get("planId", ""),
        {"userInput": params.get("userInput", ""), "reason": params.get("reason")},
    )


async def handle_agent_debug_todo_add(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.add_todo(params)


async def handle_agent_debug_todo_patch(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.patch_todo(params.get("todoId", ""), params.get("patch") or {})


async def handle_agent_debug_todo_batch_rerun(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return await gateway.batch_rerun_todos(params)


async def handle_agent_debug_run_logs(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.get_run_logs(params.get("runId", ""))


async def handle_agent_debug_run_metrics(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.get_run_metrics(params.get("runId", ""))


async def handle_agent_debug_proposals_list(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.list_proposals(params.get("sessionId", ""))


async def handle_agent_debug_proposal_apply(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return await gateway.apply_proposal(params.get("proposalId", ""))


async def handle_agent_debug_proposal_discard(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return await gateway.discard_proposal(params.get("proposalId", ""))


async def handle_agent_debug_todos_list(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    session_id = params.get("sessionId", "")
    return gateway.get_todos(session_id)


async def handle_agent_debug_session_get(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    session_id = params.get("sessionId", "")
    return gateway.get_session(session_id)


async def handle_agent_debug_models_list(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return gateway.list_models()


async def handle_agent_debug_model_preferences_get(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return gateway.get_model_preferences()


async def handle_agent_debug_model_preferences_set(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.set_model_preferences(str(params.get("modelId", "")))


async def handle_agent_debug_session_model_set(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return gateway.set_session_model(params.get("sessionId", ""), params.get("modelId"))


async def handle_agent_debug_run_get(params: Dict[str, Any], gateway: Any = None) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    run_id = params.get("runId", "")
    return gateway.get_run(run_id)


async def handle_agent_debug_run_pause(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    run_id = params.get("runId", "")
    return await gateway.pause_run(run_id)


async def handle_agent_debug_run_resume(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    run_id = params.get("runId", "")
    return await gateway.resume_run(run_id)


async def handle_agent_debug_run_cancel(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    run_id = params.get("runId", "")
    return await gateway.cancel_run(run_id)


async def handle_agent_debug_todo_rerun(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return await gateway.rerun_todo(params.get("runId", ""), params.get("todoId", ""))


async def handle_agent_debug_node_rerun(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    return await gateway.rerun_node(params.get("runId", ""), params.get("nodeId", ""))


async def handle_agent_debug_replay_get(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    session_id = params.get("sessionId", "")
    return gateway.get_replay(session_id)


async def handle_agent_debug_swarm_list(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return await gateway.list_swarm_state()


async def handle_agent_debug_swarm_seed_demo(
    params: Dict[str, Any], gateway: Any = None
) -> Dict[str, Any]:
    gateway = _resolve_gateway(gateway)
    del params
    return await gateway.seed_swarm_demo()


def register_agent_debug_handlers(server: Any, gateway: Any = None) -> None:
    gateway = _resolve_gateway(gateway)

    async def forward_event(event: Any) -> None:
        if hasattr(server, "broadcast_event"):
            await server.broadcast_event(event.type, asdict_safe(event))

    gateway.event_bus.subscribe(forward_event)
    server.register_handler(
        "agent_debug.session.create", _bind_gateway(handle_agent_debug_session_create, gateway)
    )
    server.register_handler(
        "agent_debug.session.get", _bind_gateway(handle_agent_debug_session_get, gateway)
    )
    server.register_handler(
        "agent_debug.models.list", _bind_gateway(handle_agent_debug_models_list, gateway)
    )
    server.register_handler(
        "agent_debug.model_preferences.get",
        _bind_gateway(handle_agent_debug_model_preferences_get, gateway),
    )
    server.register_handler(
        "agent_debug.model_preferences.set",
        _bind_gateway(handle_agent_debug_model_preferences_set, gateway),
    )
    server.register_handler(
        "agent_debug.session.model.set",
        _bind_gateway(handle_agent_debug_session_model_set, gateway),
    )
    server.register_handler(
        "agent_debug.plan.generate", _bind_gateway(handle_agent_debug_plan_generate, gateway)
    )
    server.register_handler(
        "agent_debug.plan.execute", _bind_gateway(handle_agent_debug_plan_execute, gateway)
    )
    server.register_handler(
        "agent_debug.ask.execute", _bind_gateway(handle_agent_debug_ask_execute, gateway)
    )
    server.register_handler(
        "agent_debug.todos.list", _bind_gateway(handle_agent_debug_todos_list, gateway)
    )
    server.register_handler("agent_debug.run.get", _bind_gateway(handle_agent_debug_run_get, gateway))
    server.register_handler(
        "agent_debug.run.pause", _bind_gateway(handle_agent_debug_run_pause, gateway)
    )
    server.register_handler(
        "agent_debug.run.resume", _bind_gateway(handle_agent_debug_run_resume, gateway)
    )
    server.register_handler(
        "agent_debug.run.cancel", _bind_gateway(handle_agent_debug_run_cancel, gateway)
    )
    server.register_handler(
        "agent_debug.todo.rerun", _bind_gateway(handle_agent_debug_todo_rerun, gateway)
    )
    server.register_handler(
        "agent_debug.node.rerun", _bind_gateway(handle_agent_debug_node_rerun, gateway)
    )
    server.register_handler(
        "agent_debug.replay.get", _bind_gateway(handle_agent_debug_replay_get, gateway)
    )
    server.register_handler(
        "agent_debug.swarm.list", _bind_gateway(handle_agent_debug_swarm_list, gateway)
    )
    server.register_handler(
        "agent_debug.swarm.seed_demo", _bind_gateway(handle_agent_debug_swarm_seed_demo, gateway)
    )
    server.register_handler(
        "agent_debug.sessions.list", _bind_gateway(handle_agent_debug_sessions_list, gateway)
    )
    server.register_handler(
        "agent_debug.design.snapshot", _bind_gateway(handle_agent_debug_design_snapshot, gateway)
    )
    server.register_handler(
        "agent_debug.provider.status", _bind_gateway(handle_agent_debug_provider_status, gateway)
    )
    server.register_handler(
        "agent_debug.search_config.get",
        _bind_gateway(handle_agent_debug_search_config_get, gateway),
    )
    server.register_handler(
        "agent_debug.search_config.set",
        _bind_gateway(handle_agent_debug_search_config_set, gateway),
    )
    server.register_handler(
        "agent_debug.plan.get", _bind_gateway(handle_agent_debug_plan_get, gateway)
    )
    server.register_handler(
        "agent_debug.plan.patch", _bind_gateway(handle_agent_debug_plan_patch, gateway)
    )
    server.register_handler(
        "agent_debug.plan.confirm", _bind_gateway(handle_agent_debug_plan_confirm, gateway)
    )
    server.register_handler(
        "agent_debug.plan.replan", _bind_gateway(handle_agent_debug_plan_replan, gateway)
    )
    server.register_handler(
        "agent_debug.todo.add", _bind_gateway(handle_agent_debug_todo_add, gateway)
    )
    server.register_handler(
        "agent_debug.todo.patch", _bind_gateway(handle_agent_debug_todo_patch, gateway)
    )
    server.register_handler(
        "agent_debug.todo.batch_rerun",
        _bind_gateway(handle_agent_debug_todo_batch_rerun, gateway),
    )
    server.register_handler(
        "agent_debug.run.logs", _bind_gateway(handle_agent_debug_run_logs, gateway)
    )
    server.register_handler(
        "agent_debug.run.metrics", _bind_gateway(handle_agent_debug_run_metrics, gateway)
    )
    server.register_handler(
        "agent_debug.proposals.list",
        _bind_gateway(handle_agent_debug_proposals_list, gateway),
    )
    server.register_handler(
        "agent_debug.proposal.apply",
        _bind_gateway(handle_agent_debug_proposal_apply, gateway),
    )
    server.register_handler(
        "agent_debug.proposal.discard",
        _bind_gateway(handle_agent_debug_proposal_discard, gateway),
    )
