//! SSE event stream: replay `fromSeq` backlog, then live events for a session.
//! The Go edge gateway consumes this to drive client WS/SSE fan-out.
//!
//! Gap signalling: if the requested `fromSeq` predates the retained ring
//! buffer, or the broadcast channel lags (events dropped), a synthetic
//! `stream.gap` event is emitted so the client knows to resync via the REST
//! replay endpoint instead of silently missing events.

use std::convert::Infallible;
use std::sync::Arc;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::api::gateway::AppServices;
use crate::contracts::events::DebugEvent;

fn to_sse(ev: &DebugEvent) -> Event {
    let data = serde_json::to_string(&ev.to_wire()).unwrap_or_default();
    Event::default()
        .id(ev.seq.to_string())
        .event(ev.event_type.clone())
        .data(data)
}

fn gap_event(session_id: &str, reason: &str) -> Event {
    Event::default().event("stream.gap").data(
        json!({
            "sessionId": session_id,
            "type": "stream.gap",
            "channel": "logs",
            "payload": { "gap": true, "reason": reason },
        })
        .to_string(),
    )
}

pub fn session_stream(
    app: Arc<AppServices>,
    session_id: String,
    from_seq: i64,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (backlog, gap) = app.bus.replay_since(&session_id, from_seq, None);

    let mut head: Vec<Result<Event, Infallible>> = Vec::with_capacity(backlog.len() + 1);
    if gap {
        head.push(Ok(gap_event(&session_id, "replay-window-exceeded")));
    }
    head.extend(backlog.into_iter().map(|ev| Ok(to_sse(&ev))));
    let backlog_stream = stream::iter(head);

    let rx = app.bus.subscribe();
    let live = stream::unfold((rx, session_id), |(mut rx, sid)| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if ev.session_id == sid {
                        return Some((Ok(to_sse(&ev)), (rx, sid)));
                    }
                }
                // Slow consumer: the broadcast channel dropped events. Tell
                // the client instead of silently skipping them.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("sse subscriber lagged, {n} events dropped");
                    let ev = gap_event(&sid, "subscriber-lagged");
                    return Some((Ok(ev), (rx, sid)));
                }
                Err(RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(backlog_stream.chain(live)).keep_alive(KeepAlive::default())
}
