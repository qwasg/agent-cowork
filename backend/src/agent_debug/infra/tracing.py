from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List

from src.agent_debug.infra.utils import make_id, utc_now_iso


@dataclass
class TraceSpan:
    id: str
    name: str
    trace_id: str
    started_at: str
    ended_at: str | None = None
    attrs: Dict[str, str] = field(default_factory=dict)


class TraceCollector:
    def __init__(self) -> None:
        self.spans: List[TraceSpan] = []

    def start_span(self, trace_id: str, name: str, attrs: Dict[str, str] | None = None) -> TraceSpan:
        span = TraceSpan(
            id=make_id("span"),
            name=name,
            trace_id=trace_id,
            started_at=utc_now_iso(),
            attrs=attrs or {},
        )
        self.spans.append(span)
        return span

    def finish_span(self, span: TraceSpan) -> TraceSpan:
        span.ended_at = utc_now_iso()
        return span
