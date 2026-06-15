//! In-process event bus: `tokio::broadcast` live fan-out + per-session bounded
//! replay ring buffer + monotonic per-session seq + optional JSONL persistence.
//!
//! Optimizations over the Python `EventBus`:
//! - Buffer is **bounded by default** (config `event_buffer_cap`) instead of
//!   growing without limit (fixes the long-session OOM risk).
//! - Live delivery uses a lock-free `broadcast` channel; subscribers filter by
//!   session id, so publishing never iterates a listener list under a lock.
//! - JSONL persistence is offloaded to a dedicated writer thread, keeping
//!   `emit` free of synchronous file IO.
//! - Sessions are lazily rehydrated from JSONL on first access, so replay and
//!   seq continuity survive process restarts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;

use crate::jsonl::JsonlStore;
use agent_protocol::events::{DebugEvent, EventDraft};
use agent_protocol::models::{new_id, now_ts};

enum LogOp {
    Append(String, DebugEvent),
    Delete(String),
    /// Keep only events with `seq <= max_seq` in the durable log.
    Truncate(String, i64),
    Flush(mpsc::Sender<()>),
}

struct Inner {
    per_session: HashMap<String, VecDeque<DebugEvent>>,
    seq_by_session: HashMap<String, i64>,
    hydrated: HashSet<String>,
}

pub struct EventBus {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<DebugEvent>,
    cap: Option<usize>,
    jsonl: Option<Arc<JsonlStore>>,
    log_tx: Option<mpsc::Sender<LogOp>>,
}

impl EventBus {
    pub fn new(cap: Option<usize>, jsonl: Option<Arc<JsonlStore>>) -> Arc<Self> {
        Self::with_broadcast_cap(cap, 4096, jsonl)
    }

    /// `broadcast_cap` bounds the live fan-out channel; slow SSE consumers
    /// past that lag get a `stream.gap` instead of blocking publishers.
    pub fn with_broadcast_cap(
        cap: Option<usize>,
        broadcast_cap: usize,
        jsonl: Option<Arc<JsonlStore>>,
    ) -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(broadcast_cap.max(64));
        let log_tx = jsonl.as_ref().map(|store| {
            let store = store.clone();
            let (log_tx, log_rx) = mpsc::channel::<LogOp>();
            // Dedicated writer thread: the only place that does blocking JSONL
            // file IO, so `emit` never blocks an async worker.
            std::thread::Builder::new()
                .name("event-jsonl-writer".to_string())
                .spawn(move || {
                    while let Ok(op) = log_rx.recv() {
                        match op {
                            LogOp::Append(session_id, event) => {
                                store.append(&session_id, &event);
                            }
                            LogOp::Delete(session_id) => store.delete_session(&session_id),
                            LogOp::Truncate(session_id, max_seq) => {
                                store.truncate_after_seq(&session_id, max_seq)
                            }
                            LogOp::Flush(ack) => {
                                let _ = ack.send(());
                            }
                        }
                    }
                })
                .ok();
            log_tx
        });
        Arc::new(EventBus {
            inner: Mutex::new(Inner {
                per_session: HashMap::new(),
                seq_by_session: HashMap::new(),
                hydrated: HashSet::new(),
            }),
            tx,
            cap,
            jsonl,
            log_tx,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DebugEvent> {
        self.tx.subscribe()
    }

    /// Lazily restore a session's ring buffer + seq counter from the JSONL log
    /// (no-op if the session was already touched in this process).
    fn ensure_hydrated(&self, session_id: &str) {
        let Some(jsonl) = &self.jsonl else { return };
        {
            let inner = self.inner.lock().unwrap();
            if inner.hydrated.contains(session_id) {
                return;
            }
        }
        // Read outside the lock — first touch only.
        let events = jsonl.read_session(session_id);
        let mut inner = self.inner.lock().unwrap();
        if !inner.hydrated.insert(session_id.to_string()) {
            return; // another thread hydrated concurrently
        }
        let bucket_empty = inner
            .per_session
            .get(session_id)
            .map(|b| b.is_empty())
            .unwrap_or(true);
        let cur_seq = *inner.seq_by_session.get(session_id).unwrap_or(&0);
        if events.is_empty() || !bucket_empty || cur_seq != 0 {
            return;
        }
        let max_seq = events.iter().map(|e| e.seq).max().unwrap_or(0);
        let skip = self
            .cap
            .map(|c| events.len().saturating_sub(c))
            .unwrap_or(0);
        let bucket = inner.per_session.entry(session_id.to_string()).or_default();
        for ev in events.into_iter().skip(skip) {
            bucket.push_back(ev);
        }
        inner.seq_by_session.insert(session_id.to_string(), max_seq);
    }

    pub fn next_seq(&self, session_id: &str) -> i64 {
        self.ensure_hydrated(session_id);
        let mut inner = self.inner.lock().unwrap();
        let entry = inner
            .seq_by_session
            .entry(session_id.to_string())
            .or_insert(0);
        *entry += 1;
        *entry
    }

    pub fn latest_seq(&self, session_id: &str) -> i64 {
        self.ensure_hydrated(session_id);
        let inner = self.inner.lock().unwrap();
        *inner.seq_by_session.get(session_id).unwrap_or(&0)
    }

    /// Assign id/seq/ts to a draft and publish it. Returns the stored event.
    pub fn emit(&self, draft: EventDraft) -> DebugEvent {
        self.emit_inner(draft, true)
    }

    /// Like `emit` but skips JSONL persistence — used for high-frequency
    /// streaming token deltas so the durable log isn't flooded (optimization).
    pub fn emit_ephemeral(&self, draft: EventDraft) -> DebugEvent {
        self.emit_inner(draft, false)
    }

    fn emit_inner(&self, draft: EventDraft, persist: bool) -> DebugEvent {
        let seq = self.next_seq(&draft.session_id);
        let mut source = std::collections::BTreeMap::new();
        source.insert("domain".to_string(), draft.domain.clone());
        source.insert("actor".to_string(), draft.actor.clone());
        let event = DebugEvent {
            id: new_id("evt"),
            session_id: draft.session_id.clone(),
            seq,
            event_type: draft.event_type.clone(),
            ts: now_ts(),
            source,
            correlation_id: draft.correlation_id.clone(),
            payload: draft.payload.clone(),
        };
        self.publish_inner(event.clone(), persist);
        event
    }

    pub fn publish(&self, event: DebugEvent) {
        self.publish_inner(event, true)
    }

    fn publish_inner(&self, event: DebugEvent, persist: bool) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.hydrated.insert(event.session_id.clone());
            let bucket = inner
                .per_session
                .entry(event.session_id.clone())
                .or_insert_with(VecDeque::new);
            bucket.push_back(event.clone());
            if let Some(cap) = self.cap {
                while bucket.len() > cap {
                    bucket.pop_front();
                }
            }
            let cur = inner
                .seq_by_session
                .entry(event.session_id.clone())
                .or_insert(0);
            if event.seq > *cur {
                *cur = event.seq;
            }
        }
        if persist {
            if let Some(log_tx) = &self.log_tx {
                if log_tx
                    .send(LogOp::Append(event.session_id.clone(), event.clone()))
                    .is_err()
                {
                    tracing::warn!("event jsonl writer is gone; event not persisted");
                }
            }
        }
        // Ignore send error: it only means there are currently no subscribers.
        let _ = self.tx.send(event);
    }

    /// Block until all queued JSONL writes are flushed (used on shutdown).
    pub fn flush(&self, timeout: Duration) {
        if let Some(log_tx) = &self.log_tx {
            let (ack_tx, ack_rx) = mpsc::channel();
            if log_tx.send(LogOp::Flush(ack_tx)).is_ok() {
                let _ = ack_rx.recv_timeout(timeout);
            }
        }
    }

    /// Returns `(events_after_from_seq, gap)` where `gap` is true if `from_seq`
    /// predates the retained ring-buffer window.
    pub fn replay_since(
        &self,
        session_id: &str,
        from_seq: i64,
        limit: Option<usize>,
    ) -> (Vec<DebugEvent>, bool) {
        self.ensure_hydrated(session_id);
        let inner = self.inner.lock().unwrap();
        let Some(bucket) = inner.per_session.get(session_id) else {
            return (Vec::new(), false);
        };
        if bucket.is_empty() {
            return (Vec::new(), false);
        }
        let gap = if self.cap.is_some() {
            let oldest = bucket.front().map(|e| e.seq).unwrap_or(0);
            from_seq + 1 < oldest
        } else {
            false
        };
        let mut out = Vec::new();
        for ev in bucket.iter() {
            if ev.seq <= from_seq {
                continue;
            }
            out.push(ev.clone());
            if let Some(l) = limit {
                if out.len() >= l {
                    break;
                }
            }
        }
        (out, gap)
    }

    pub fn snapshot(&self, session_id: &str) -> Vec<DebugEvent> {
        self.ensure_hydrated(session_id);
        let inner = self.inner.lock().unwrap();
        inner
            .per_session
            .get(session_id)
            .map(|b| b.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn purge_session(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.per_session.remove(session_id);
        inner.seq_by_session.remove(session_id);
        // Keep the hydrated marker so the (deleted) file isn't re-read.
        inner.hydrated.insert(session_id.to_string());
        drop(inner);
        if let Some(log_tx) = &self.log_tx {
            if log_tx.send(LogOp::Delete(session_id.to_string())).is_err() {
                tracing::warn!("event_bus: log writer gone, session {session_id} file not deleted");
            }
        }
    }

    /// Copy a session's whole event history into another session, re-assigning
    /// ids/seq for the destination (port of Python `EventBus.fork_session`).
    pub fn fork_session(&self, old_session_id: &str, new_session_id: &str) {
        let events = self.snapshot(old_session_id);
        for mut ev in events {
            ev.id = new_id("evt");
            ev.session_id = new_session_id.to_string();
            ev.seq = self.next_seq(new_session_id);
            self.publish_inner(ev, true);
        }
    }

    /// Keep events up to *and including* the one with `event_id` (port of
    /// Python `truncate_session`). No-op when the event isn't buffered.
    pub fn truncate_session(&self, session_id: &str, event_id: &str) {
        let target_seq = {
            self.ensure_hydrated(session_id);
            let inner = self.inner.lock().unwrap();
            inner
                .per_session
                .get(session_id)
                .and_then(|b| b.iter().find(|e| e.id == event_id).map(|e| e.seq))
        };
        if let Some(seq) = target_seq {
            self.truncate_to_seq(session_id, seq);
        }
    }

    /// Truncate to *before* the target event (exclusive; used by "edit &
    /// resend"). When the target belongs to a run, rewind to before that
    /// run's earliest event so leading `agent.started` events go too.
    pub fn truncate_before_event(&self, session_id: &str, event_id: &str) {
        let cutoff = {
            self.ensure_hydrated(session_id);
            let inner = self.inner.lock().unwrap();
            let Some(bucket) = inner.per_session.get(session_id) else {
                return;
            };
            let Some(target) = bucket.iter().find(|e| e.id == event_id) else {
                return;
            };
            let run_id = target
                .payload
                .get("runId")
                .or_else(|| target.payload.get("run_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| target.correlation_id.clone());
            let mut cutoff_seq = target.seq;
            if let Some(rid) = run_id {
                for ev in bucket.iter() {
                    let ev_run = ev
                        .payload
                        .get("runId")
                        .or_else(|| ev.payload.get("run_id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| ev.correlation_id.clone());
                    if ev_run.as_deref() == Some(rid.as_str()) {
                        cutoff_seq = cutoff_seq.min(ev.seq);
                    }
                }
            }
            cutoff_seq - 1
        };
        self.truncate_to_seq(session_id, cutoff);
    }

    /// Keep only events with `seq <= max_seq` (memory + durable log) and
    /// rewind the session's seq counter (used by checkpoint rewind).
    pub fn truncate_to_seq(&self, session_id: &str, max_seq: i64) {
        self.ensure_hydrated(session_id);
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(bucket) = inner.per_session.get_mut(session_id) {
                bucket.retain(|e| e.seq <= max_seq);
            }
            inner
                .seq_by_session
                .insert(session_id.to_string(), max_seq.max(0));
        }
        if let Some(log_tx) = &self.log_tx {
            if log_tx
                .send(LogOp::Truncate(session_id.to_string(), max_seq))
                .is_err()
            {
                tracing::warn!(
                    "event_bus: log writer gone, session {session_id} not truncated on disk"
                );
            }
        }
    }

    /// Rehydrate a session's in-memory buffer + seq from persisted events.
    pub fn hydrate(&self, session_id: &str, events: Vec<DebugEvent>) {
        if events.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        inner.hydrated.insert(session_id.to_string());
        let bucket = inner
            .per_session
            .entry(session_id.to_string())
            .or_insert_with(VecDeque::new);
        let mut max_seq = 0;
        for ev in events {
            max_seq = max_seq.max(ev.seq);
            bucket.push_back(ev);
            if let Some(cap) = self.cap {
                while bucket.len() > cap {
                    bucket.pop_front();
                }
            }
        }
        inner.seq_by_session.insert(session_id.to_string(), max_seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn draft(sid: &str, etype: &str) -> EventDraft {
        EventDraft::new(sid, etype, "agent").payload(json!({ "n": etype }))
    }

    #[test]
    fn detects_gap_when_ring_buffer_overflows() {
        let bus = EventBus::new(Some(64), None);
        let sid = "sess_gap";
        for i in 0..100 {
            bus.emit(draft(sid, &format!("e{i}")));
        }
        let (events, gap) = bus.replay_since(sid, 0, None);
        assert!(gap, "fromSeq=0 predates the retained window");
        assert_eq!(events.len(), 64);
        // Replaying from the newest seq has no gap.
        let latest = bus.latest_seq(sid);
        let (events, gap) = bus.replay_since(sid, latest, None);
        assert!(!gap);
        assert!(events.is_empty());
    }

    #[test]
    fn rehydrates_from_jsonl_and_continues_seq() {
        let dir = std::env::temp_dir().join(format!(
            "agentd_bus_test_{}",
            agent_protocol::models::new_id("d")
        ));
        let sid = "sess_hydrate";
        {
            // First "process": emit persisted events.
            let jsonl = Arc::new(JsonlStore::new(dir.clone()));
            let bus = EventBus::new(Some(4096), Some(jsonl));
            for i in 0..5 {
                bus.emit(draft(sid, &format!("e{i}")));
            }
            bus.flush(Duration::from_secs(5));
        }
        // Second "process": fresh bus over the same log dir.
        let jsonl = Arc::new(JsonlStore::new(dir));
        let bus = EventBus::new(Some(4096), Some(jsonl));
        let (events, gap) = bus.replay_since(sid, 0, None);
        assert!(!gap);
        assert_eq!(events.len(), 5, "replay must survive a restart");
        // Sequence numbering continues instead of restarting at 1.
        let ev = bus.emit(draft(sid, "after-restart"));
        assert_eq!(ev.seq, 6);
    }
}
