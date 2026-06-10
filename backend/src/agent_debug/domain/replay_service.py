from __future__ import annotations

from typing import Any, Dict, List

from src.agent_debug.infra.event_bus import EventBus


class ReplayService:
    def __init__(self, event_bus: EventBus) -> None:
        self.event_bus = event_bus

    def get_session_replay(self, session_id: str) -> Dict[str, List[Dict[str, Any]]]:
        return {
            "events": self.event_bus.snapshot(session_id),
        }
