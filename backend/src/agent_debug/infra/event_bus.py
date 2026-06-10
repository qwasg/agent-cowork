"""In-process event bus with per-session replay buffer and incremental replay.

Design notes:

- Events are stored in two structures:
  ``_events`` keeps a global flat list (used by snapshot/replay services that
  ignore session id), while ``_per_session`` keeps a deque per session so
  subscribers can resume from a known ``seq`` without paying O(N) over the
  global stream.
- ``next_seq`` is monotonic *per session*. Sequence numbers start at 1.
- ``replay_since(session_id, from_seq, limit=None)`` returns events whose
  ``seq > from_seq``. The boolean ``gap`` flag is True when the requested
  ``from_seq`` is older than the oldest buffered event for that session *and*
  a finite ring buffer is in effect.
- ``subscribe`` takes an async listener fan-out for live events. Returning a
  disposer keeps the legacy contract.
- By default the per-session buffer is **unbounded** (no eviction). Set
  ``AGENT_DEBUG_EVENT_BUFFER`` to a positive integer to enable a ring buffer
  (minimum 64). Use ``0`` or ``unlimited`` for unbounded explicitly.
"""

from __future__ import annotations

import os
from collections import deque
from dataclasses import asdict as _asdict
from typing import Any, Awaitable, Callable, Deque, Dict, List, Optional, Tuple

from src.agent_debug.domain.models import DebugEvent, asdict_safe


EventHandler = Callable[[DebugEvent], Awaitable[None]]

_DEFAULT_BUFFER_CAP: Optional[int] = None


def _resolve_default_cap() -> Optional[int]:
    raw = os.getenv("AGENT_DEBUG_EVENT_BUFFER", "").strip().lower()
    if not raw:
        return _DEFAULT_BUFFER_CAP
    if raw in ("0", "unlimited", "none", "inf", "infinity"):
        return None
    try:
        value = int(raw)
    except ValueError:
        return _DEFAULT_BUFFER_CAP
    if value <= 0:
        return None
    return max(64, value)


def _make_session_bucket(cap: Optional[int]) -> Deque[DebugEvent]:
    if cap is None:
        return deque()
    return deque(maxlen=cap)


class EventBus:
    """Async-fan-out event bus with per-session replay buffers (bounded or not)."""

    def __init__(self, *, buffer_cap: Optional[int] = None, persistence: Any = None) -> None:
        self._listeners: List[EventHandler] = []
        self._events: List[DebugEvent] = []
        self._per_session: Dict[str, Deque[DebugEvent]] = {}
        self._seq_by_session: Dict[str, int] = {}
        if buffer_cap is not None:
            cap = int(buffer_cap)
            self._buffer_cap = None if cap <= 0 else cap
        else:
            self._buffer_cap = _resolve_default_cap()
        self._persistence = persistence

    def _session_bucket(self, session_id: str) -> Deque[DebugEvent]:
        bucket = self._per_session.get(session_id)
        if bucket is None:
            bucket = _make_session_bucket(self._buffer_cap)
            self._per_session[session_id] = bucket
        return bucket

    def attach_persistence(self, persistence: Any) -> None:
        self._persistence = persistence

    def hydrate_session(self, session_id: str, events: List[Dict[str, Any]]) -> None:
        """用持久化的事件（snake_case dict）重建会话缓冲与序号。"""
        if not events:
            return
        bucket = self._session_bucket(session_id)
        max_seq = self._seq_by_session.get(session_id, 0)
        for raw in events:
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
            except (KeyError, TypeError):
                continue
            self._events.append(event)
            bucket.append(event)
            max_seq = max(max_seq, event.seq)
        self._seq_by_session[session_id] = max_seq

    def subscribe(self, listener: EventHandler) -> Callable[[], None]:
        self._listeners.append(listener)

        def dispose() -> None:
            if listener in self._listeners:
                self._listeners.remove(listener)

        return dispose

    async def publish(self, event: DebugEvent) -> None:
        self._events.append(event)
        self._session_bucket(event.session_id).append(event)
        if self._persistence is not None:
            try:
                self._persistence.append(event.session_id, _asdict(event))
            except Exception:  # pragma: no cover - 持久化失败不应阻断事件流
                pass
        for listener in list(self._listeners):
            await listener(event)

    def next_seq(self, session_id: str) -> int:
        next_value = self._seq_by_session.get(session_id, 0) + 1
        self._seq_by_session[session_id] = next_value
        return next_value

    def latest_seq(self, session_id: str) -> int:
        return self._seq_by_session.get(session_id, 0)

    def snapshot(self, session_id: str) -> List[Dict[str, Any]]:
        bucket = self._per_session.get(session_id)
        if bucket is None:
            return [
                asdict_safe(event)
                for event in self._events
                if event.session_id == session_id
            ]
        return [asdict_safe(event) for event in bucket]

    def purge_session(self, session_id: str) -> None:
        """彻底清除某会话的事件缓冲与序号（用于会话删除级联清理）。

        与 ``truncate_*`` 不同，这里连同序号一并丢弃，确保被删会话不再残留
        任何内存事件，避免重启前的同进程内回放到「幽灵历史」。持久化文件由
        调用方（``JsonlEventStore.delete_session``）单独删除。
        """
        self._per_session.pop(session_id, None)
        self._seq_by_session.pop(session_id, None)
        if self._events:
            self._events = [ev for ev in self._events if ev.session_id != session_id]

    def fork_session(self, old_session_id: str, new_session_id: str) -> None:
        bucket = self._per_session.get(old_session_id)
        events_to_copy = bucket if bucket is not None else [
            ev for ev in self._events if ev.session_id == old_session_id
        ]

        new_bucket = self._session_bucket(new_session_id)
        for ev in events_to_copy:
            new_ev_dict = asdict_safe(ev)
            new_ev_dict["session_id"] = new_session_id
            new_ev_dict["seq"] = self.next_seq(new_session_id)
            new_ev = DebugEvent(**new_ev_dict)
            self._events.append(new_ev)
            new_bucket.append(new_ev)
            if self._persistence is not None:
                try:
                    self._persistence.append(new_session_id, _asdict(new_ev))
                except Exception:  # pragma: no cover
                    pass

    def truncate_session(self, session_id: str, event_id: str) -> None:
        bucket = self._per_session.get(session_id)
        if not bucket:
            return

        target_seq = -1
        for ev in bucket:
            if ev.id == event_id:
                target_seq = ev.seq
                break

        if target_seq == -1:
            return

        new_bucket = _make_session_bucket(self._buffer_cap)
        for ev in bucket:
            new_bucket.append(ev)
            if ev.seq == target_seq:
                break
        self._per_session[session_id] = new_bucket

        self._events = [
            ev for ev in self._events
            if ev.session_id != session_id or ev.seq <= target_seq
        ]

        self._seq_by_session[session_id] = target_seq

        if self._persistence is not None:
            try:
                self._persistence.truncate_after_seq(session_id, target_seq)
            except Exception:  # pragma: no cover
                pass

    def truncate_before_event(self, session_id: str, event_id: str) -> None:
        """截断到目标事件 *之前*（exclusive，用于「编辑并重发」）。

        若目标事件归属某个 run（``payload.runId`` / ``correlation_id``），则
        回退到该 run 在流中最早事件之前，从而把先于
        ``composer.user.message`` 发出的 ``agent.started`` 一并删除，避免
        留下悬空的空 assistant 消息。
        """
        bucket = self._per_session.get(session_id)
        if not bucket:
            return

        target: Optional[DebugEvent] = None
        for ev in bucket:
            if ev.id == event_id:
                target = ev
                break
        if target is None:
            return

        run_id = None
        payload = target.payload or {}
        if isinstance(payload, dict):
            run_id = payload.get("runId") or payload.get("run_id")
        run_id = run_id or target.correlation_id

        cutoff_seq = target.seq
        if run_id:
            for ev in bucket:
                ev_payload = ev.payload if isinstance(ev.payload, dict) else {}
                ev_run = (
                    ev_payload.get("runId")
                    or ev_payload.get("run_id")
                    or ev.correlation_id
                )
                if ev_run == run_id:
                    cutoff_seq = min(cutoff_seq, ev.seq)

        self.truncate_to_seq(session_id, cutoff_seq - 1)

    def truncate_to_seq(self, session_id: str, max_seq: int) -> None:
        """保留 ``seq <= max_seq`` 的事件（用于 checkpoint rewind）。"""
        bucket = self._per_session.get(session_id)
        if bucket is not None:
            new_bucket = _make_session_bucket(self._buffer_cap)
            for ev in bucket:
                if ev.seq <= max_seq:
                    new_bucket.append(ev)
            self._per_session[session_id] = new_bucket
        self._events = [
            ev for ev in self._events if ev.session_id != session_id or ev.seq <= max_seq
        ]
        self._seq_by_session[session_id] = max_seq
        if self._persistence is not None:
            try:
                self._persistence.truncate_after_seq(session_id, max_seq)
            except Exception:  # pragma: no cover
                pass

    def replay_since(
        self,
        session_id: str,
        from_seq: int,
        *,
        limit: Optional[int] = None,
    ) -> Tuple[List[Dict[str, Any]], bool]:
        """Return (events_after_from_seq, gap).

        ``gap`` is True iff ``from_seq + 1`` was already evicted from the
        per-session ring buffer (only when a finite cap is configured).
        """
        bucket = self._per_session.get(session_id)
        if bucket is None or not bucket:
            return [], False
        gap = False
        if self._buffer_cap is not None:
            oldest_seq = bucket[0].seq
            gap = from_seq + 1 < oldest_seq
        out: List[Dict[str, Any]] = []
        for event in bucket:
            if event.seq <= from_seq:
                continue
            out.append(asdict_safe(event))
            if limit is not None and len(out) >= limit:
                break
        return out, gap

    def buffer_capacity(self) -> Optional[int]:
        return self._buffer_cap

    def session_buffer_len(self, session_id: str) -> int:
        return len(self._per_session.get(session_id, ()))
