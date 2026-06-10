//! Debug event schema. Wire format keeps the Python contract exactly:
//! `{ id, sessionId, seq, type, ts, source, correlationId, channel, payload }`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Event domains that map to dedicated WS channels (rest fall back to "logs").
pub fn channel_for(domain: &str) -> &'static str {
    match domain {
        "plan" => "plan",
        "agent" => "agent",
        "todo" => "todo",
        "subagent" => "subagent",
        "swarm" => "swarm",
        "provider" => "provider",
        "tool" => "tool",
        _ => "logs",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugEvent {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub seq: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub ts: String,
    /// e.g. {"domain": "agent", "actor": "main"}
    pub source: BTreeMap<String, String>,
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub payload: Value,
}

impl DebugEvent {
    pub fn domain(&self) -> &str {
        self.source.get("domain").map(|s| s.as_str()).unwrap_or("")
    }

    pub fn channel(&self) -> &'static str {
        channel_for(self.domain())
    }

    /// Wire JSON with the derived `channel` tag (matches `ws_gateway._encode_event`).
    pub fn to_wire(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "sessionId": self.session_id,
            "seq": self.seq,
            "type": self.event_type,
            "ts": self.ts,
            "source": self.source,
            "correlationId": self.correlation_id,
            "channel": self.channel(),
            "payload": self.payload,
        })
    }
}

/// Builder for emitting events from domain code.
pub struct EventDraft {
    pub session_id: String,
    pub event_type: String,
    pub domain: String,
    pub actor: String,
    pub payload: Value,
    pub correlation_id: Option<String>,
}

impl EventDraft {
    pub fn new(
        session_id: impl Into<String>,
        event_type: impl Into<String>,
        domain: impl Into<String>,
    ) -> Self {
        EventDraft {
            session_id: session_id.into(),
            event_type: event_type.into(),
            domain: domain.into(),
            actor: "main".to_string(),
            payload: Value::Object(Default::default()),
            correlation_id: None,
        }
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = actor.into();
        self
    }

    pub fn correlation(mut self, id: Option<String>) -> Self {
        self.correlation_id = id;
        self
    }
}
