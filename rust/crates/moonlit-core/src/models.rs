//! Domain models exchanged with the Python backend (`backend/src/agent_debug`).
//!
//! All objects are serialized to camelCase JSON by the backend's
//! `asdict_safe()`, so we mirror that here. Free-form / rapidly-evolving
//! payloads are kept as [`serde_json::Value`] to stay resilient to backend
//! schema drift; the strongly-typed fields cover the stable contract the UI
//! depends on.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn is_false(b: &bool) -> bool {
    !*b
}

/// Account profile returned by the auth endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PublicUser {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub plan: Option<Value>,
    #[serde(default)]
    pub monthly_cap_rmb: Option<f64>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub user: PublicUser,
    pub token: String,
}

/// A debug session (top-level conversation/workspace unit).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DebugSession {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub selected_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub web_search_enabled: bool,
    #[serde(default)]
    pub active_plan_id: Option<String>,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub title_manually_set: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl DebugSession {
    /// Mirrors backend `is_placeholder_session`: hides historical demo/test rows.
    pub fn is_placeholder(&self) -> bool {
        if self.title_manually_set {
            return false;
        }
        let title = self.title.trim().to_lowercase();
        let mode = self.mode.as_deref().unwrap_or("").trim().to_lowercase();
        if matches!(title.as_str(), "demo" | "ask demo" | "shared" | "ask flow") {
            return true;
        }
        if title == "demo" && (mode == "hybrid" || self.active_plan_id.is_some()) {
            return true;
        }
        if title == "新会话" {
            return true;
        }
        if matches!(title.as_str(), "" | "agent debug session" | "新的 agent debug 会话") {
            return true;
        }
        if title.starts_with("分支 · demo") {
            return true;
        }
        if matches!(title.as_str(), "e2e??" | "plan??") {
            return true;
        }
        false
    }
}

/// Todo item. `status` is one of the backend enum values; kept as String to
/// avoid breaking on new states.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Any extra fields not modeled above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// An agent run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub active_todo_ids: Vec<String>,
    #[serde(default)]
    pub completed_todo_ids: Vec<String>,
    #[serde(default)]
    pub failed_todo_ids: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A single file change inside a [`Proposal`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProposalChange {
    #[serde(default)]
    pub change_id: Option<String>,
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub original_content: String,
    #[serde(default)]
    pub proposed_content: String,
}

/// A code-edit proposal awaiting apply/discard.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub changes: Vec<ProposalChange>,
    #[serde(default)]
    pub applied_files: Vec<String>,
    #[serde(default)]
    pub apply_error: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A model option for the picker.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelOption {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_reasoning: bool,
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
    #[serde(default)]
    pub availability: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_default: bool,
}

/// An LLM provider channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub provider_label: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub api_key_set: bool,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_china: bool,
    #[serde(default)]
    pub models: Vec<Value>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Aggregate metrics for the status bar / plan inspector.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesignMetrics {
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub files_touched: u64,
    #[serde(default)]
    pub avg_latency_ms: f64,
    #[serde(default)]
    pub plan_progress: Option<Value>,
    #[serde(default)]
    pub todos: Option<Value>,
    #[serde(default)]
    pub subagents: u64,
    #[serde(default)]
    pub context_fill_pct: f64,
}

/// A derived diff entry shown in the Diff tab.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesignDiff {
    pub id: String,
    #[serde(default)]
    pub proposal_id: Option<String>,
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub plus: u64,
    #[serde(default)]
    pub minus: u64,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub original_content: String,
    #[serde(default)]
    pub proposed_content: String,
}

/// The generic event envelope delivered over WS/SSE/replay.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DebugEvent {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub seq: Option<u64>,
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub source: Option<Value>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

impl DebugEvent {
    /// Convenience: read a string field from the payload.
    pub fn payload_str(&self, key: &str) -> Option<&str> {
        self.payload.get(key).and_then(Value::as_str)
    }
}

/// The big bootstrap blob from `GET /design-snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesignSnapshot {
    #[serde(default)]
    pub sessions: Vec<DebugSession>,
    #[serde(default)]
    pub active_session: Option<DebugSession>,
    #[serde(default)]
    pub plan_bundle: Option<Value>,
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    #[serde(default)]
    pub events: Vec<DebugEvent>,
    #[serde(default)]
    pub run: Option<AgentRun>,
    #[serde(default)]
    pub swarm: Option<Value>,
    #[serde(default)]
    pub diffs: Vec<DesignDiff>,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    #[serde(default)]
    pub metrics: DesignMetrics,
    #[serde(default)]
    pub context_window: Option<Value>,
    #[serde(default)]
    pub models: Option<Value>,
    #[serde(default)]
    pub latest_seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_camel_case() {
        let json = r#"{"id":"sess_1","title":"hi","webSearchEnabled":true,"pinned":false}"#;
        let s: DebugSession = serde_json::from_str(json).unwrap();
        assert_eq!(s.id, "sess_1");
        assert!(s.web_search_enabled);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["webSearchEnabled"], serde_json::json!(true));
    }

    #[test]
    fn event_decodes_type_and_payload() {
        let json = r#"{"seq":42,"type":"agent.token.stream.delta","payload":{"delta":"hello"}}"#;
        let e: DebugEvent = serde_json::from_str(json).unwrap();
        assert_eq!(e.event_type, "agent.token.stream.delta");
        assert_eq!(e.seq, Some(42));
        assert_eq!(e.payload_str("delta"), Some("hello"));
    }

    #[test]
    fn snapshot_tolerates_missing_fields() {
        let s: DesignSnapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(s.latest_seq, 0);
        assert!(s.sessions.is_empty());
    }

    #[test]
    fn placeholder_sessions_are_detected() {
        let demo = DebugSession {
            id: "sess_demo".into(),
            title: "demo".into(),
            mode: Some("hybrid".into()),
            ..Default::default()
        };
        assert!(demo.is_placeholder());

        let real = DebugSession {
            id: "sess_real".into(),
            title: "你好".into(),
            ..Default::default()
        };
        assert!(!real.is_placeholder());
    }
}
