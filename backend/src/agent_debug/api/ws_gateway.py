"""WebSocket fan-out + per-session subscribe with ``fromSeq`` replay support.

Protocol (client → server):

    {"action": "subscribe", "sessionId": "<sid>", "fromSeq": <int?>, "channels": [..]}

If ``fromSeq`` is provided, the gateway tries to backfill any buffered events
``seq > fromSeq`` from ``EventBus.replay_since``. If the requested seq is older
than the bus retention window, the gateway sends a single
``ws.replay.gap`` frame so the client can fall back to a fresh
``GET /design-snapshot`` and resubscribe.

Live frames are forwarded after the optional backfill. Each forwarded frame is
also tagged with ``channel`` (derived from ``event.source.domain``) so the
client can filter without parsing every payload.

Legacy clients that subscribe without ``fromSeq`` still see only real domain
events — no synthetic ack is emitted, preserving the original
``receive_json`` contract used in older tests.
"""

from __future__ import annotations

import json
import logging
from collections import defaultdict
from typing import Any, Dict, Iterable, List, Optional, Set


# P9 v2 W9 (M9.6, D9-12) — module logger so the previously-silent
# ``_send_text`` drop path surfaces dead-socket reaping at warning level
# (helps diagnose subscriber churn / leaked sockets).
logger = logging.getLogger(__name__)

from src.agent_debug.domain.models import DebugEvent
from src.agent_debug.infra.event_bus import EventBus


def _channel_for(domain: str) -> str:
    if domain in {"plan", "agent", "todo", "subagent", "swarm", "provider", "tool"}:
        return domain
    return "logs"


class _Subscription:
    __slots__ = ("websocket", "channels")

    def __init__(self, websocket: Any, channels: Optional[Iterable[str]]) -> None:
        self.websocket = websocket
        self.channels: Optional[Set[str]] = (
            None if not channels else {str(c) for c in channels}
        )


def _encode_event(event: DebugEvent) -> str:
    return json.dumps(
        {
            "id": event.id,
            "sessionId": event.session_id,
            "seq": event.seq,
            "type": event.type,
            "ts": event.ts,
            "source": event.source,
            "correlationId": event.correlation_id,
            "channel": _channel_for(event.source.get("domain", "")),
            "payload": event.payload,
        }
    )


class AgentDebugWsGateway:
    def __init__(self, event_bus: EventBus) -> None:
        self.event_bus = event_bus
        self.subscribers_by_session: Dict[str, List[_Subscription]] = defaultdict(list)
        self.event_bus.subscribe(self._fan_out)

    async def handle_subscribe(self, websocket: Any, payload: Dict[str, Any]) -> None:
        session_id = payload.get("sessionId")
        if not session_id:
            return
        from_seq_raw = payload.get("fromSeq")
        try:
            from_seq = int(from_seq_raw) if from_seq_raw is not None else None
        except (TypeError, ValueError):
            from_seq = None
        channels = payload.get("channels")

        for bucket in self.subscribers_by_session.values():
            bucket[:] = [s for s in bucket if s.websocket is not websocket]
        self.subscribers_by_session[session_id].append(
            _Subscription(websocket, channels if isinstance(channels, list) else None)
        )

        if from_seq is not None:
            replay, gap = self.event_bus.replay_since(session_id, from_seq)
            if gap:
                await self._send_text(
                    websocket,
                    json.dumps(
                        {
                            "type": "ws.replay.gap",
                            "sessionId": session_id,
                            "requestedFromSeq": from_seq,
                            "latestSeq": self.event_bus.latest_seq(session_id),
                            "code": "WS_REPLAY_GAP_TOO_LARGE",
                            "message": (
                                "Requested fromSeq is older than the buffered window. "
                                "Re-fetch GET /design-snapshot and resubscribe."
                            ),
                        }
                    ),
                )
                return
            for evt in replay:
                await self._send_text(websocket, json.dumps(evt))
            await self._send_text(
                websocket,
                json.dumps(
                    {
                        "type": "ws.subscribed",
                        "sessionId": session_id,
                        "latestSeq": self.event_bus.latest_seq(session_id),
                    }
                ),
            )

    async def disconnect(self, websocket: Any) -> None:
        for bucket in self.subscribers_by_session.values():
            bucket[:] = [s for s in bucket if s.websocket is not websocket]

    async def _fan_out(self, event: DebugEvent) -> None:
        encoded = _encode_event(event)
        channel = _channel_for(event.source.get("domain", ""))
        for sub in list(self.subscribers_by_session.get(event.session_id, [])):
            if sub.channels is not None and channel not in sub.channels:
                continue
            await self._send_text(sub.websocket, encoded)

    async def _send_text(self, websocket: Any, encoded: str) -> None:
        try:
            if hasattr(websocket, "send_text"):
                await websocket.send_text(encoded)
            else:
                await websocket.send(encoded)
        except Exception as exc:
            # P9 v2 W9 (M9.6, D9-12) — surface dead-socket reaping so
            # subscriber churn isn't invisible. Behaviour preserved: the
            # subscriber is still removed from every session bucket.
            logger.warning(
                "agent_debug WS send failed; dropping subscriber: %s", exc
            )
            for bucket in self.subscribers_by_session.values():
                bucket[:] = [s for s in bucket if s.websocket is not websocket]
