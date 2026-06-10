from __future__ import annotations

from threading import Lock

from src.agent_debug.api.rest_gateway import AgentDebugRestGateway
from src.agent_debug.domain.auth_service import AuthService
from src.agent_debug.provider.base import ProviderRegistry

_gateway_lock = Lock()
_gateway_singleton: AgentDebugRestGateway | None = None

_auth_lock = Lock()
_auth_singleton: AuthService | None = None


def get_auth_service() -> AuthService:
    global _auth_singleton
    with _auth_lock:
        if _auth_singleton is None:
            _auth_singleton = AuthService()
        return _auth_singleton


def set_auth_service(auth: AuthService) -> AuthService:
    global _auth_singleton
    with _auth_lock:
        _auth_singleton = auth
        return _auth_singleton


def create_agent_debug_gateway(
    provider_registry: ProviderRegistry | None = None,
) -> AgentDebugRestGateway:
    return AgentDebugRestGateway(provider_registry=provider_registry)


def get_agent_debug_gateway() -> AgentDebugRestGateway:
    global _gateway_singleton
    with _gateway_lock:
        if _gateway_singleton is None:
            _gateway_singleton = create_agent_debug_gateway()
        return _gateway_singleton


def set_agent_debug_gateway(gateway: AgentDebugRestGateway) -> AgentDebugRestGateway:
    global _gateway_singleton
    with _gateway_lock:
        _gateway_singleton = gateway
        return _gateway_singleton


def reset_agent_debug_gateway(
    provider_registry: ProviderRegistry | None = None,
) -> AgentDebugRestGateway:
    return set_agent_debug_gateway(create_agent_debug_gateway(provider_registry=provider_registry))
