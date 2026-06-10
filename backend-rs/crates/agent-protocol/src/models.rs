//! Domain models. All serialize as camelCase to match the existing API contract
//! consumed by `apps/agent-ide/public/api-client.jsx` and the Tauri shell.

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugSession {
    pub id: String,
    pub title: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub selected_model_id: Option<String>,
    #[serde(default)]
    pub web_search_enabled: bool,
    #[serde(default)]
    pub active_plan_id: Option<String>,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub title_manually_set: bool,
}

fn default_status() -> String {
    "idle".to_string()
}
fn default_mode() -> String {
    "hybrid".to_string()
}

impl DebugSession {
    pub fn new(
        id: String,
        title: String,
        selected_model_id: Option<String>,
        web_search_enabled: bool,
    ) -> Self {
        let ts = now_iso();
        DebugSession {
            id,
            title,
            status: default_status(),
            mode: default_mode(),
            selected_model_id,
            web_search_enabled,
            active_plan_id: None,
            active_run_id: None,
            created_at: ts.clone(),
            updated_at: ts,
            pinned: false,
            title_manually_set: false,
        }
    }
    pub fn touch(&mut self) {
        self.updated_at = now_iso();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelOption {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub source: String,
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(default)]
    pub supports_reasoning: bool,
    #[serde(default)]
    pub context_window_tokens: Option<i64>,
    #[serde(default = "default_availability")]
    pub availability: String,
    #[serde(default)]
    pub is_default: bool,
}

fn default_tier() -> String {
    "balanced".to_string()
}
fn default_availability() -> String {
    "available".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub session_id: String,
    pub objective: String,
    #[serde(default = "plan_default_status")]
    pub status: String,
    #[serde(default)]
    pub current_version_id: String,
    #[serde(default)]
    pub latest_execution_id: Option<String>,
    #[serde(default)]
    pub stages: Vec<PlanStage>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn plan_default_status() -> String {
    "drafting".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStage {
    pub id: String,
    pub plan_id: String,
    pub title: String,
    pub order: i64,
    #[serde(default = "node_default_status")]
    pub status: String,
    #[serde(default)]
    pub tasks: Vec<PlanTask>,
}

fn node_default_status() -> String {
    "draft".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTask {
    pub id: String,
    pub stage_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "task_default_priority")]
    pub priority: String,
    #[serde(default = "task_default_parallelism")]
    pub parallelism: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "node_default_status")]
    pub status: String,
    #[serde(default = "task_default_owner")]
    pub owner_type: String,
}

fn task_default_priority() -> String {
    "medium".to_string()
}
fn task_default_parallelism() -> String {
    "serial".to_string()
}
fn task_default_owner() -> String {
    "main-agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: String,
    pub session_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// `"explore"` (read-only investigation) or `"edit"` (writes files /
    /// runs commands). Drives tool gating and scheduling in `run_plan`.
    #[serde(default = "todo_default_kind")]
    pub kind: String,
    #[serde(default = "todo_default_source")]
    pub source: String,
    #[serde(default = "todo_default_priority")]
    pub priority: i64,
    #[serde(default = "todo_default_status")]
    pub status: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub related_agent_run_id: Option<String>,
    #[serde(default)]
    pub retry_count: i64,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn todo_default_kind() -> String {
    "edit".to_string()
}
fn todo_default_source() -> String {
    "agent".to_string()
}
fn todo_default_priority() -> i64 {
    3
}
fn todo_default_status() -> String {
    "queued".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default = "run_default_trigger")]
    pub trigger: String,
    #[serde(default = "run_default_status")]
    pub status: String,
    #[serde(default)]
    pub active_todo_ids: Vec<String>,
    #[serde(default)]
    pub completed_todo_ids: Vec<String>,
    #[serde(default)]
    pub failed_todo_ids: Vec<String>,
    #[serde(default = "run_default_parallel")]
    pub parallel_limit: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn run_default_trigger() -> String {
    "manual".to_string()
}
fn run_default_status() -> String {
    "starting".to_string()
}
fn run_default_parallel() -> i64 {
    4
}

/// Minimal chat message used by the runtime conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::simple("system", content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::simple("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::simple("assistant", content)
    }
    pub fn simple(role: &str, content: impl Into<String>) -> Self {
        ChatMessage {
            role: role.to_string(),
            content: content.into(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }
    pub fn tool_result(tool_call_id: String, name: String, content: String) -> Self {
        ChatMessage {
            role: "tool".to_string(),
            content,
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id),
            name: Some(name),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(default = "tool_call_kind")]
    pub kind: String,
    pub name: String,
    /// JSON-encoded arguments string (OpenAI function-calling convention).
    pub arguments: String,
}

fn tool_call_kind() -> String {
    "function".to_string()
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

pub fn now_ts() -> String {
    now_iso()
}
