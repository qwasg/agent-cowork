pub mod app;
pub mod ui;

use moonlit_core::models::{DebugEvent, DebugSession, DesignSnapshot, TodoItem};
use moonlit_uikit::{
    compute_line_diff, CodeBuffer, CommandPalette, DiffLine, SplitPane, Theme, ToastStack,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposerMode {
    Build,
    Debug,
    Ask,
    Multitask,
    Plan,
}

impl ComposerMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Debug => "debug",
            Self::Ask => "ask",
            Self::Multitask => "multitask",
            Self::Plan => "plan",
        }
    }
}

/// Normalize an `ask:execute` or `GET /plans/{id}` payload into the workbench
/// plan-bundle shape (`{ plan, stages?, tasks? }`).
pub fn plan_response_to_bundle(value: &Value) -> Option<Value> {
    if value.get("plan").is_none() {
        return None;
    }
    if value.get("tasks").is_some() || value.get("stages").is_some() {
        return Some(value.clone());
    }
    let plan = value.get("plan")?;
    let stages = plan.get("stages").cloned().unwrap_or(Value::Null);
    let mut tasks: Vec<Value> = Vec::new();
    if let Some(stage_list) = stages.as_array() {
        for stage in stage_list {
            if let Some(stage_tasks) = stage.get("tasks").and_then(|v| v.as_array()) {
                tasks.extend(stage_tasks.iter().cloned());
            }
        }
    }
    Some(serde_json::json!({
        "plan": plan,
        "stages": stages,
        "tasks": tasks,
    }))
}

/// Extract a plan id from plan lifecycle event payloads.
pub fn plan_id_from_event_payload(payload: &Value) -> Option<String> {
    payload
        .get("planId")
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// The agent profile a session runs as (general / document / coding). Mirrors
/// the backend `DebugSession.agentKind`; fixed at session creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    General,
    Document,
    Coding,
}

impl AgentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Document => "document",
            Self::Coding => "coding",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::General => "通用助手",
            Self::Document => "文档处理",
            Self::Coding => "编码",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "general" => Self::General,
            "document" => Self::Document,
            _ => Self::Coding,
        }
    }

    /// Composer modes this profile exposes. Non-coding agents are
    /// conversational (ask / build only).
    pub fn supports_mode(&self, mode: &ComposerMode) -> bool {
        match self {
            Self::Coding => true,
            _ => matches!(mode, ComposerMode::Build | ComposerMode::Ask),
        }
    }

    pub const ALL: [AgentKind; 3] = [Self::Coding, Self::Document, Self::General];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: ChatRole,
    pub text: String,
    pub reasoning: String,
    pub run_id: Option<String>,
    pub status: MessageStatus,
    /// HH:MM derived from the first event timestamp (legacy `msg.time`).
    #[serde(default)]
    pub time: String,
    /// Ordered action timeline (legacy `AgentBlocks`): text / reasoning /
    /// tool / subagent blocks interleaved in event order.
    #[serde(default)]
    pub blocks: Vec<ChatBlock>,
    /// Raw ISO timestamp of `agent.started` (used for the worked-for duration).
    #[serde(default)]
    pub started_ts: Option<String>,
    /// Raw ISO timestamp of `agent.completed` / `failed` / `cancelled`.
    #[serde(default)]
    pub finished_ts: Option<String>,
}

impl ChatMessage {
    /// Whole-turn duration in seconds, when both boundary timestamps are known.
    pub fn work_duration_secs(&self) -> Option<u64> {
        duration_between_ts(self.started_ts.as_deref(), self.finished_ts.as_deref())
    }
}

/// One entry of the assistant action timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ChatBlock {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Tool {
        id: String,
        name: String,
        args: String,
        result: String,
        status: BlockStatus,
        /// `Some((server, tool))` when the tool name is `mcp__server__tool`.
        mcp: Option<(String, String)>,
    },
    Subagent {
        id: String,
        label: String,
        prompt: String,
        summary: String,
        status: BlockStatus,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
        #[serde(default)]
        work: Vec<ChatBlock>,
        #[serde(default)]
        started_ts: Option<String>,
        #[serde(default)]
        finished_ts: Option<String>,
    },
}

impl ChatBlock {
    pub fn subagent_work_duration_secs(&self) -> Option<u64> {
        match self {
            ChatBlock::Subagent {
                started_ts,
                finished_ts,
                ..
            } => duration_between_ts(started_ts.as_deref(), finished_ts.as_deref()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockStatus {
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Default, Clone)]
pub struct ChatStore {
    pub messages: Vec<ChatMessage>,
    pub active_run_id: Option<String>,
    hidden_subagent_run_ids: std::collections::HashSet<String>,
    seen_event_keys: std::collections::HashSet<String>,
}

impl ChatStore {
    pub fn reset(&mut self) {
        self.messages.clear();
        self.active_run_id = None;
        self.hidden_subagent_run_ids.clear();
        self.seen_event_keys.clear();
    }

    pub fn push_local_user(&mut self, text: String) {
        self.messages.push(ChatMessage {
            id: format!("local-{}", self.messages.len() + 1),
            role: ChatRole::User,
            text,
            reasoning: String::new(),
            run_id: None,
            status: MessageStatus::Completed,
            time: String::new(),
            blocks: Vec::new(),
            started_ts: None,
            finished_ts: None,
        });
    }

    /// Drop `message_id` and everything after it (optimistic local echo of a
    /// backend revert before an edited resend).
    pub fn truncate_at(&mut self, message_id: &str) {
        if let Some(idx) = self.messages.iter().position(|m| m.id == message_id) {
            self.messages.truncate(idx);
            self.active_run_id = None;
            self.rebuild_hidden_subagent_ids();
        }
    }

    pub fn apply_event(&mut self, evt: &DebugEvent) {
        if !self.mark_event_seen(evt) {
            return;
        }
        let time = hhmm_from_ts(evt.ts.as_deref());
        if self.apply_subagent_agent_event(evt) {
            self.normalize_messages();
            return;
        }
        match evt.event_type.as_str() {
            "composer.user.message" => {
                let text = evt.payload_str("text").unwrap_or_default().to_string();
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let user = ChatMessage {
                    id: evt
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("user-{}", evt.seq.unwrap_or(0))),
                    role: ChatRole::User,
                    text,
                    reasoning: String::new(),
                    run_id: run_id.clone(),
                    status: MessageStatus::Completed,
                    time,
                    blocks: Vec::new(),
                    started_ts: None,
                    finished_ts: None,
                };
                self.upsert_user(user, run_id.as_deref());
            }
            "agent.started" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                self.active_run_id = run_id.clone();
                let ts = evt.ts.clone();
                let msg = self.ensure_assistant(run_id);
                if msg.time.is_empty() {
                    msg.time = time;
                }
                if msg.started_ts.is_none() {
                    msg.started_ts = ts;
                }
            }
            "agent.token.stream.delta" => {
                let delta = evt.payload_str("delta").unwrap_or_default().to_string();
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                msg.text.push_str(&delta);
                msg.status = MessageStatus::Streaming;
                match msg.blocks.last_mut() {
                    Some(ChatBlock::Text { text }) => text.push_str(&delta),
                    _ => msg.blocks.push(ChatBlock::Text { text: delta }),
                }
            }
            "agent.reasoning.delta" => {
                let delta = evt.payload_str("delta").unwrap_or_default().to_string();
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                msg.reasoning.push_str(&delta);
                msg.status = MessageStatus::Streaming;
                match msg.blocks.last_mut() {
                    Some(ChatBlock::Reasoning { text }) => text.push_str(&delta),
                    _ => msg.blocks.push(ChatBlock::Reasoning { text: delta }),
                }
            }
            "agent.tool.invoked" => {
                // Sub-agent nested events carry parentToolCallId; bucket those
                // under the Task card by skipping top-level handling.
                if evt.payload.get("parentToolCallId").is_some() {
                    return;
                }
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let name = evt.payload_str("name").unwrap_or("tool").to_string();
                let id = evt
                    .payload_str("toolCallId")
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("tool-{}", evt.seq.unwrap_or(0)));
                let msg = self.ensure_assistant(run_id);
                msg.status = MessageStatus::Streaming;
                if is_subagent_tool_name(&name) {
                    let prompt = evt
                        .payload
                        .get("args")
                        .or_else(|| evt.payload.get("arguments"))
                        .and_then(|a| a.get("prompt"))
                        .and_then(Value::as_str)
                        .or_else(|| evt.payload_str("prompt"))
                        .unwrap_or_default()
                        .to_string();
                    let label = evt
                        .payload
                        .get("args")
                        .or_else(|| evt.payload.get("arguments"))
                        .and_then(|a| a.get("description"))
                        .and_then(Value::as_str)
                        .or_else(|| {
                            evt.payload
                                .get("args")
                                .or_else(|| evt.payload.get("arguments"))
                                .and_then(|a| a.get("title"))
                                .and_then(Value::as_str)
                        })
                        .or_else(|| evt.payload_str("label"))
                        .or_else(|| first_nonempty_line(&prompt))
                        .unwrap_or("子代理任务")
                        .to_string();
                    msg.blocks.push(ChatBlock::Subagent {
                        id: id.clone(),
                        label,
                        prompt,
                        summary: String::new(),
                        status: BlockStatus::Running,
                        parent_tool_call_id: Some(id),
                        work: Vec::new(),
                        started_ts: evt.ts.clone(),
                        finished_ts: None,
                    });
                } else {
                    let args = evt
                        .payload
                        .get("args")
                        .or_else(|| evt.payload.get("arguments"))
                        .map(|a| serde_json::to_string_pretty(a).unwrap_or_default())
                        .unwrap_or_default();
                    let mcp = name.strip_prefix("mcp__").and_then(|rest| {
                        rest.split_once("__")
                            .map(|(server, tool)| (server.to_string(), tool.to_string()))
                    });
                    msg.blocks.push(ChatBlock::Tool {
                        id,
                        name,
                        args,
                        result: String::new(),
                        status: BlockStatus::Running,
                        mcp,
                    });
                }
            }
            "agent.tool.completed" | "agent.tool.failed" | "agent.tool.denied" => {
                if evt.payload.get("parentToolCallId").is_some() {
                    return;
                }
                let failed = evt.event_type != "agent.tool.completed";
                let call_id = evt.payload_str("toolCallId").map(str::to_string);
                let output = if failed {
                    evt.payload_str("message")
                        .or_else(|| evt.payload_str("reason"))
                        .or_else(|| evt.payload_str("code"))
                        .unwrap_or("工具执行失败")
                        .to_string()
                } else {
                    evt.payload_str("output")
                        .or_else(|| evt.payload_str("outputPreview"))
                        .unwrap_or_default()
                        .to_string()
                };
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                if let Some(ChatBlock::Subagent {
                    summary,
                    status,
                    finished_ts,
                    ..
                }) = msg.blocks.iter_mut().rev().find(|b| match b {
                    ChatBlock::Subagent {
                        id,
                        parent_tool_call_id,
                        status,
                        ..
                    } => call_id
                        .as_deref()
                        .map(|c| c == id || parent_tool_call_id.as_deref() == Some(c))
                        .unwrap_or(*status == BlockStatus::Running),
                    _ => false,
                }) {
                    if !output.is_empty() {
                        *summary = output;
                    }
                    *status = if failed {
                        BlockStatus::Error
                    } else {
                        BlockStatus::Done
                    };
                    *finished_ts = evt.ts.clone();
                } else if let Some(block) = msg.blocks.iter_mut().rev().find(|b| match b {
                    ChatBlock::Tool { id, status, .. } => call_id
                        .as_deref()
                        .map(|c| c == id)
                        .unwrap_or(*status == BlockStatus::Running),
                    _ => false,
                }) {
                    if let ChatBlock::Tool { result, status, .. } = block {
                        *result = output;
                        *status = if failed {
                            BlockStatus::Error
                        } else {
                            BlockStatus::Done
                        };
                    }
                }
            }
            "subagent.started" => {
                let Some(sub_id) = subagent_event_id(evt) else {
                    self.normalize_messages();
                    return;
                };
                self.hidden_subagent_run_ids.insert(sub_id.clone());
                let parent_run_id = evt
                    .payload_str("parentRunId")
                    .map(str::to_string)
                    .or_else(|| evt.correlation_id.clone());
                let label = evt
                    .payload_str("description")
                    .or_else(|| evt.payload_str("label"))
                    .unwrap_or("子代理任务")
                    .to_string();
                let prompt = evt.payload_str("prompt").unwrap_or_default().to_string();
                let msg = self.ensure_assistant(parent_run_id);
                if let Some(ChatBlock::Subagent {
                    id,
                    label: existing_label,
                    prompt: existing_prompt,
                    parent_tool_call_id,
                    started_ts,
                    finished_ts,
                    status,
                    ..
                }) = msg.blocks.iter_mut().rev().find(|b| match b {
                    ChatBlock::Subagent { id, status, .. } => {
                        id == &sub_id || *status == BlockStatus::Running
                    }
                    _ => false,
                }) {
                    if parent_tool_call_id.is_none() {
                        *parent_tool_call_id = Some(id.clone());
                    }
                    *id = sub_id;
                    if !label.is_empty() {
                        *existing_label = label;
                    }
                    if !prompt.is_empty() {
                        *existing_prompt = prompt;
                    }
                    if started_ts.is_none() {
                        *started_ts = evt.ts.clone();
                    }
                    *finished_ts = None;
                    *status = BlockStatus::Running;
                } else if !msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ChatBlock::Subagent { id, .. } if *id == sub_id))
                {
                    msg.blocks.push(ChatBlock::Subagent {
                        id: sub_id.clone(),
                        label,
                        prompt,
                        summary: String::new(),
                        status: BlockStatus::Running,
                        parent_tool_call_id: Some(sub_id.clone()),
                        work: Vec::new(),
                        started_ts: evt.ts.clone(),
                        finished_ts: None,
                    });
                }
            }
            "subagent.created" => {
                let run_id = payload_run_id(&evt.payload)
                    .or_else(|| evt.payload_str("parentRunId").map(str::to_string))
                    .or_else(|| evt.correlation_id.clone());
                let parent_id = evt.payload_str("parentToolCallId").map(str::to_string);
                let sub_id = evt
                    .payload_str("subagentRunId")
                    .or_else(|| evt.payload_str("subagentId"))
                    .or_else(|| evt.payload_str("subRunId"))
                    .or(parent_id.as_deref())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("sub-{}", evt.seq.unwrap_or(0)));
                self.hidden_subagent_run_ids.insert(sub_id.clone());
                let label = evt
                    .payload_str("description")
                    .or_else(|| evt.payload_str("label"))
                    .unwrap_or("子代理任务")
                    .to_string();
                let prompt = evt.payload_str("prompt").unwrap_or_default().to_string();
                let msg = self.ensure_assistant(run_id);
                if let Some(ChatBlock::Subagent {
                    id,
                    label: existing_label,
                    prompt: existing_prompt,
                    parent_tool_call_id,
                    started_ts,
                    finished_ts,
                    status,
                    ..
                }) = msg.blocks.iter_mut().rev().find(|b| match b {
                    ChatBlock::Subagent { id, .. } => {
                        Some(id.as_str()) == parent_id.as_deref() || id == &sub_id
                    }
                    _ => false,
                }) {
                    *id = sub_id;
                    if parent_tool_call_id.is_none() {
                        *parent_tool_call_id = parent_id.clone().or_else(|| Some(id.clone()));
                    }
                    if !label.is_empty() {
                        *existing_label = label;
                    }
                    if !prompt.is_empty() {
                        *existing_prompt = prompt;
                    }
                    if started_ts.is_none() {
                        *started_ts = evt.ts.clone();
                    }
                    *finished_ts = None;
                    *status = BlockStatus::Running;
                } else if !msg
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ChatBlock::Subagent { id, .. } if *id == sub_id))
                {
                    msg.blocks.push(ChatBlock::Subagent {
                        id: sub_id.clone(),
                        label,
                        prompt,
                        summary: String::new(),
                        status: BlockStatus::Running,
                        parent_tool_call_id: parent_id.clone().or_else(|| Some(sub_id.clone())),
                        work: Vec::new(),
                        started_ts: evt.ts.clone(),
                        finished_ts: None,
                    });
                }
            }
            "subagent.completed" | "subagent.failed" => {
                let failed = evt.event_type == "subagent.failed";
                let sub_id = evt
                    .payload_str("subagentRunId")
                    .or_else(|| evt.payload_str("subagentId"))
                    .or_else(|| evt.payload_str("subRunId"))
                    .or_else(|| evt.payload_str("parentToolCallId"))
                    .map(str::to_string);
                if let Some(sub_id) = &sub_id {
                    self.hidden_subagent_run_ids.insert(sub_id.clone());
                }
                let new_summary = evt
                    .payload_str("summary")
                    .or_else(|| evt.payload_str("message"))
                    .or_else(|| evt.payload_str("error"))
                    .unwrap_or_default()
                    .to_string();
                let run_id = payload_run_id(&evt.payload)
                    .or_else(|| evt.payload_str("parentRunId").map(str::to_string))
                    .or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                if let Some(ChatBlock::Subagent {
                    summary,
                    status,
                    finished_ts,
                    ..
                }) = msg.blocks.iter_mut().rev().find(|b| match b {
                    ChatBlock::Subagent {
                        id,
                        parent_tool_call_id,
                        status,
                        ..
                    } => sub_id
                        .as_deref()
                        .map(|c| c == id || parent_tool_call_id.as_deref() == Some(c))
                        .unwrap_or(*status == BlockStatus::Running),
                    _ => false,
                }) {
                    *status = if failed {
                        BlockStatus::Error
                    } else {
                        BlockStatus::Done
                    };
                    *finished_ts = evt.ts.clone();
                    if !new_summary.is_empty() {
                        *summary = new_summary;
                    }
                }
            }
            "agent.completed" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let text = evt.payload_str("text").unwrap_or_default();
                let ts = evt.ts.clone();
                let msg = self.ensure_assistant(run_id);
                if !text.is_empty() {
                    msg.text = text.to_string();
                    if !matches!(msg.blocks.last(), Some(ChatBlock::Text { .. })) {
                        msg.blocks.push(ChatBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
                msg.status = MessageStatus::Completed;
                msg.finished_ts = ts;
                self.active_run_id = None;
            }
            "agent.failed" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let ts = evt.ts.clone();
                let msg = self.ensure_assistant(run_id);
                msg.status = MessageStatus::Failed;
                msg.finished_ts = ts;
                self.active_run_id = None;
            }
            "agent.cancelled" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let ts = evt.ts.clone();
                let msg = self.ensure_assistant(run_id);
                msg.status = MessageStatus::Cancelled;
                msg.finished_ts = ts;
                self.active_run_id = None;
            }
            _ => {}
        }
        self.normalize_messages();
    }

    fn apply_subagent_agent_event(&mut self, evt: &DebugEvent) -> bool {
        if !evt.event_type.starts_with("agent.") {
            return false;
        }
        let parent_tool_call_id = evt.payload_str("parentToolCallId").map(str::to_string);
        let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
        let hidden_run_id = run_id
            .as_deref()
            .filter(|run_id| self.hidden_subagent_run_ids.contains(*run_id))
            .map(str::to_string);
        if parent_tool_call_id.is_none() && hidden_run_id.is_none() {
            return false;
        }

        let subagent_id = evt
            .payload_str("subagentRunId")
            .or_else(|| evt.payload_str("subagentId"))
            .or_else(|| evt.payload_str("subRunId"))
            .map(str::to_string)
            .or_else(|| hidden_run_id.clone());

        if self
            .find_subagent_block_mut(
                parent_tool_call_id.as_deref(),
                subagent_id.as_deref(),
                hidden_run_id.as_deref(),
            )
            .is_none()
        {
            let Some(parent_tool_call_id) = parent_tool_call_id.as_deref() else {
                return false;
            };
            let run_id = payload_run_id(&evt.payload)
                .or_else(|| evt.payload_str("parentRunId").map(str::to_string))
                .or_else(|| evt.correlation_id.clone());
            let id = subagent_id
                .clone()
                .unwrap_or_else(|| parent_tool_call_id.to_string());
            let msg = self.ensure_assistant(run_id);
            if !msg.blocks.iter().any(|b| {
                matches!(
                    b,
                    ChatBlock::Subagent {
                        id: existing,
                        parent_tool_call_id: existing_parent,
                        ..
                    } if existing == &id
                        || existing == parent_tool_call_id
                        || existing_parent.as_deref() == Some(parent_tool_call_id)
                )
            }) {
                msg.blocks.push(ChatBlock::Subagent {
                    id,
                    label: "子代理任务".to_string(),
                    prompt: String::new(),
                    summary: String::new(),
                    status: BlockStatus::Running,
                    parent_tool_call_id: Some(parent_tool_call_id.to_string()),
                    work: Vec::new(),
                    started_ts: evt.ts.clone(),
                    finished_ts: None,
                });
            }
        }

        let Some(block) = self.find_subagent_block_mut(
            parent_tool_call_id.as_deref(),
            subagent_id.as_deref(),
            hidden_run_id.as_deref(),
        ) else {
            return parent_tool_call_id.is_some() || hidden_run_id.is_some();
        };
        apply_agent_event_to_subagent_block(block, evt);
        true
    }

    fn find_subagent_block_mut(
        &mut self,
        parent_tool_call_id: Option<&str>,
        subagent_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Option<&mut ChatBlock> {
        self.messages.iter_mut().rev().find_map(|message| {
            message.blocks.iter_mut().rev().find(|block| match block {
                ChatBlock::Subagent {
                    id,
                    parent_tool_call_id: existing_parent,
                    ..
                } => {
                    subagent_id == Some(id.as_str())
                        || run_id == Some(id.as_str())
                        || parent_tool_call_id == Some(id.as_str())
                        || (parent_tool_call_id.is_some()
                            && existing_parent.as_deref() == parent_tool_call_id)
                }
                _ => false,
            })
        })
    }

    fn mark_event_seen(&mut self, evt: &DebugEvent) -> bool {
        self.seen_event_keys.insert(event_dedupe_key(evt))
    }

    fn upsert_user(&mut self, user: ChatMessage, run_id: Option<&str>) {
        if let Some(run_id) = run_id {
            if let Some(idx) = self
                .messages
                .iter()
                .position(|m| m.role == ChatRole::User && m.run_id.as_deref() == Some(run_id))
            {
                self.messages[idx] = user;
                return;
            }
        }
        if let Some(idx) = self
            .messages
            .iter()
            .rposition(|m| m.role == ChatRole::User && m.run_id.is_none() && m.text == user.text)
        {
            self.messages[idx] = user;
            return;
        }
        if let Some(run_id) = run_id {
            if let Some(idx) = self
                .messages
                .iter()
                .position(|m| m.role == ChatRole::Assistant && m.run_id.as_deref() == Some(run_id))
            {
                self.messages.insert(idx, user);
                return;
            }
        }
        self.messages.push(user);
    }

    fn ensure_assistant(&mut self, run_id: Option<String>) -> &mut ChatMessage {
        if let Some(run_id_ref) = run_id.as_deref() {
            if let Some(idx) = self.messages.iter().position(|m| {
                m.role == ChatRole::Assistant && m.run_id.as_deref() == Some(run_id_ref)
            }) {
                return &mut self.messages[idx];
            }
        }
        let assistant = ChatMessage {
            id: format!("assistant-{}", self.messages.len() + 1),
            role: ChatRole::Assistant,
            text: String::new(),
            reasoning: String::new(),
            run_id: run_id.clone(),
            status: MessageStatus::Streaming,
            time: String::new(),
            blocks: Vec::new(),
            started_ts: None,
            finished_ts: None,
        };
        let insert_idx = run_id.as_deref().and_then(|run_id| {
            self.messages
                .iter()
                .rposition(|m| m.role == ChatRole::User && m.run_id.as_deref() == Some(run_id))
                .map(|idx| idx + 1)
        });
        if let Some(idx) = insert_idx {
            self.messages.insert(idx, assistant);
            return &mut self.messages[idx];
        }
        self.messages.push(assistant);
        self.messages.last_mut().unwrap()
    }

    fn normalize_messages(&mut self) {
        let mut idx = 0;
        while idx < self.messages.len() {
            if self.messages[idx].role != ChatRole::User || self.messages[idx].run_id.is_none() {
                idx += 1;
                continue;
            }
            let run_id = self.messages[idx].run_id.clone();
            let text = self.messages[idx].text.clone();
            let mut scan = idx + 1;
            while scan < self.messages.len() {
                let duplicate_run_user = self.messages[scan].role == ChatRole::User
                    && self.messages[scan].run_id == run_id;
                let duplicate_local_user = self.messages[scan].role == ChatRole::User
                    && self.messages[scan].run_id.is_none()
                    && self.messages[scan].text == text;
                if duplicate_run_user || duplicate_local_user {
                    self.messages.remove(scan);
                } else {
                    scan += 1;
                }
            }
            idx += 1;
        }

        let mut assistant_idx = 0;
        while assistant_idx < self.messages.len() {
            let Some(run_id) = (self.messages[assistant_idx].role == ChatRole::Assistant)
                .then(|| self.messages[assistant_idx].run_id.clone())
                .flatten()
            else {
                assistant_idx += 1;
                continue;
            };
            let user_after_idx = self
                .messages
                .iter()
                .enumerate()
                .skip(assistant_idx + 1)
                .find_map(|(idx, m)| {
                    (m.role == ChatRole::User && m.run_id.as_deref() == Some(run_id.as_str()))
                        .then_some(idx)
                });
            if let Some(user_idx) = user_after_idx {
                let user = self.messages.remove(user_idx);
                self.messages.insert(assistant_idx, user);
                assistant_idx += 2;
            } else {
                assistant_idx += 1;
            }
        }
    }

    fn rebuild_hidden_subagent_ids(&mut self) {
        self.hidden_subagent_run_ids.clear();
        for message in &self.messages {
            for block in &message.blocks {
                if let ChatBlock::Subagent { id, .. } = block {
                    self.hidden_subagent_run_ids.insert(id.clone());
                }
            }
        }
    }
}

fn apply_agent_event_to_subagent_block(block: &mut ChatBlock, evt: &DebugEvent) {
    let ChatBlock::Subagent {
        work,
        status,
        started_ts,
        finished_ts,
        ..
    } = block
    else {
        return;
    };

    match evt.event_type.as_str() {
        "agent.started" => {
            *status = BlockStatus::Running;
            if started_ts.is_none() {
                *started_ts = evt.ts.clone();
            }
            *finished_ts = None;
        }
        "agent.token.stream.delta" => {
            let delta = evt.payload_str("delta").unwrap_or_default();
            if delta.is_empty() {
                return;
            }
            match work.last_mut() {
                Some(ChatBlock::Text { text }) => text.push_str(delta),
                _ => work.push(ChatBlock::Text {
                    text: delta.to_string(),
                }),
            }
        }
        "agent.reasoning.delta" => {
            let delta = evt.payload_str("delta").unwrap_or_default();
            if delta.is_empty() {
                return;
            }
            match work.last_mut() {
                Some(ChatBlock::Reasoning { text }) => text.push_str(delta),
                _ => work.push(ChatBlock::Reasoning {
                    text: delta.to_string(),
                }),
            }
        }
        "agent.tool.invoked" => {
            let name = evt.payload_str("name").unwrap_or("tool").to_string();
            let id = evt
                .payload_str("toolCallId")
                .map(str::to_string)
                .unwrap_or_else(|| format!("tool-{}", evt.seq.unwrap_or(0)));
            let args = payload_args(&evt.payload)
                .map(|a| serde_json::to_string_pretty(a).unwrap_or_default())
                .unwrap_or_default();
            let mcp = name.strip_prefix("mcp__").and_then(|rest| {
                rest.split_once("__")
                    .map(|(server, tool)| (server.to_string(), tool.to_string()))
            });
            work.push(ChatBlock::Tool {
                id,
                name,
                args,
                result: String::new(),
                status: BlockStatus::Running,
                mcp,
            });
        }
        "agent.tool.completed" | "agent.tool.failed" | "agent.tool.denied" => {
            let failed = evt.event_type != "agent.tool.completed";
            let call_id = evt.payload_str("toolCallId").map(str::to_string);
            let output = if failed {
                evt.payload_str("message")
                    .or_else(|| evt.payload_str("reason"))
                    .or_else(|| evt.payload_str("error"))
                    .or_else(|| evt.payload_str("code"))
                    .unwrap_or("工具执行失败")
                    .to_string()
            } else {
                evt.payload_str("output")
                    .or_else(|| evt.payload_str("outputPreview"))
                    .unwrap_or_default()
                    .to_string()
            };
            if let Some(ChatBlock::Tool { result, status, .. }) =
                work.iter_mut().rev().find(|b| match b {
                    ChatBlock::Tool { id, status, .. } => call_id
                        .as_deref()
                        .map(|c| c == id)
                        .unwrap_or(*status == BlockStatus::Running),
                    _ => false,
                })
            {
                if !output.is_empty() {
                    *result = output;
                }
                *status = if failed {
                    BlockStatus::Error
                } else {
                    BlockStatus::Done
                };
            }
        }
        "agent.completed" => {
            let text = evt.payload_str("text").unwrap_or_default();
            if !text.is_empty() {
                match work.last_mut() {
                    Some(ChatBlock::Text { text: existing }) => {
                        *existing = text.to_string();
                    }
                    _ => work.push(ChatBlock::Text {
                        text: text.to_string(),
                    }),
                }
            }
            *status = BlockStatus::Done;
            *finished_ts = evt.ts.clone();
        }
        "agent.failed" | "agent.cancelled" => {
            let text = evt
                .payload_str("error")
                .or_else(|| evt.payload_str("message"))
                .or_else(|| evt.payload_str("text"))
                .unwrap_or_default();
            if !text.is_empty() {
                work.push(ChatBlock::Text {
                    text: text.to_string(),
                });
            }
            *status = BlockStatus::Error;
            *finished_ts = evt.ts.clone();
        }
        _ => {}
    }
}

/// "2026-06-10T01:24:49…" → "01:24". Empty when absent/malformed.
fn hhmm_from_ts(ts: Option<&str>) -> String {
    ts.and_then(|s| s.get(11..16))
        .unwrap_or_default()
        .to_string()
}

fn duration_between_ts(started_ts: Option<&str>, finished_ts: Option<&str>) -> Option<u64> {
    let start = iso_to_epoch_secs(started_ts?)?;
    let end = iso_to_epoch_secs(finished_ts?)?;
    (end >= start).then(|| (end - start).round() as u64)
}

/// Minimal RFC3339 → epoch seconds parser (no chrono dependency). Handles
/// "YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]"; returns None on malformed input.
pub fn iso_to_epoch_secs(ts: &str) -> Option<f64> {
    let ts = ts.trim();
    if ts.len() < 19 {
        return None;
    }
    let year: i64 = ts.get(0..4)?.parse().ok()?;
    let month: i64 = ts.get(5..7)?.parse().ok()?;
    let day: i64 = ts.get(8..10)?.parse().ok()?;
    let hour: i64 = ts.get(11..13)?.parse().ok()?;
    let min: i64 = ts.get(14..16)?.parse().ok()?;
    let sec: i64 = ts.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut rest = &ts[19..];
    let mut frac = 0.0_f64;
    if rest.starts_with('.') {
        let digits: String = rest[1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            frac = digits.parse::<f64>().ok()? / 10f64.powi(digits.len() as i32);
        }
        rest = &rest[1 + digits.len()..];
    }
    // Timezone offset: "Z", "+HH:MM" / "-HH:MM", or absent (treated as UTC).
    let mut offset_secs = 0i64;
    if let Some(sign) = rest.chars().next() {
        if sign == '+' || sign == '-' {
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            offset_secs = (oh * 3600 + om * 60) * if sign == '-' { -1 } else { 1 };
        }
    }

    // Days-from-civil (Howard Hinnant) — valid for the proleptic Gregorian calendar.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let secs = days * 86_400 + hour * 3_600 + min * 60 + sec - offset_secs;
    Some(secs as f64 + frac)
}

fn event_dedupe_key(evt: &DebugEvent) -> String {
    if let Some(id) = evt.id.as_deref().filter(|id| !id.is_empty()) {
        return format!("id:{id}");
    }
    let run_id = payload_run_id(&evt.payload)
        .or_else(|| evt.payload_str("parentRunId").map(str::to_string))
        .or_else(|| evt.correlation_id.clone())
        .unwrap_or_default();
    let payload = serde_json::to_string(&evt.payload).unwrap_or_default();
    format!(
        "fallback:{}:{}:{}:{}",
        evt.seq
            .map(|seq| seq.to_string())
            .unwrap_or_else(|| "none".to_string()),
        evt.event_type,
        run_id,
        payload
    )
}

#[derive(Debug, Clone)]
pub struct AgentIdeState {
    pub sessions: Vec<DebugSession>,
    pub active_session_id: Option<String>,
    pub todos: Vec<TodoItem>,
    pub chat: ChatStore,
    pub workbench: WorkbenchState,
    pub settings: SettingsState,
    pub theme: Theme,
    pub panes: SplitPane,
    pub commands: CommandPalette,
    pub toasts: ToastStack,
}

impl Default for AgentIdeState {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            active_session_id: None,
            todos: Vec::new(),
            chat: ChatStore::default(),
            workbench: WorkbenchState::default(),
            settings: SettingsState::default(),
            theme: Theme::moonlit_dark(),
            panes: SplitPane::default(),
            commands: CommandPalette::default(),
            toasts: ToastStack::default(),
        }
    }
}

impl AgentIdeState {
    pub fn hydrate_snapshot(&mut self, snapshot: DesignSnapshot) {
        self.sessions = snapshot
            .sessions
            .into_iter()
            .filter(|s| !s.is_placeholder())
            .collect();
        self.active_session_id = snapshot.active_session.map(|s| s.id);
        self.todos = snapshot.todos;
        self.chat.reset();
        for evt in &snapshot.events {
            self.chat.apply_event(evt);
        }
    }

    pub fn snapshot_targets_active(&self, snapshot: &DesignSnapshot) -> bool {
        let incoming = snapshot.active_session.as_ref().map(|s| s.id.as_str());
        match (self.active_session_id.as_deref(), incoming) {
            (Some(current), Some(incoming)) => current == incoming,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    pub fn hydrate_snapshot_if_current(&mut self, snapshot: DesignSnapshot) -> bool {
        if !self.snapshot_targets_active(&snapshot) {
            return false;
        }
        self.hydrate_snapshot(snapshot);
        true
    }

    pub fn event_targets_active(&self, evt: &DebugEvent) -> bool {
        match (self.active_session_id.as_deref(), evt.session_id.as_deref()) {
            (Some(current), Some(incoming)) => current == incoming,
            _ => true,
        }
    }

    pub fn apply_event(&mut self, evt: DebugEvent) -> bool {
        if !self.event_targets_active(&evt) {
            return false;
        }
        self.chat.apply_event(&evt);
        match evt.event_type.as_str() {
            "todo.created" | "todo.updated" | "todo.started" | "todo.running"
            | "todo.completed" | "todo.failed" => {
                merge_todo_event(&mut self.todos, &evt.event_type, &evt.payload);
            }
            _ => {}
        }
        true
    }
}

/// Field-wise merge of a `todo.*` event into the todo list: events carry
/// partial payloads, so wholesale replacement would wipe fields (notably
/// `status`) that the payload omits. Unknown todos are inserted; the status
/// falls back to what the event type implies when the payload lacks one.
fn merge_todo_event(todos: &mut Vec<TodoItem>, evt_type: &str, payload: &Value) {
    let Some(id) = payload.get("id").and_then(|v| v.as_str()) else {
        return;
    };
    let status_hint = match evt_type {
        "todo.started" | "todo.running" => Some("running"),
        "todo.completed" => Some("completed"),
        "todo.failed" => Some("failed"),
        _ => None,
    };
    let payload_status = payload
        .get("status")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if let Some(existing) = todos.iter_mut().find(|t| t.id == id) {
        if let Some(title) = payload
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            existing.title = title.to_string();
        }
        if let Some(status) = payload_status.or(status_hint) {
            existing.status = status.to_string();
        }
        if let Some(desc) = payload.get("description").and_then(|v| v.as_str()) {
            if !desc.is_empty() {
                existing.description = Some(desc.to_string());
            }
        }
        if let Some(summary) = payload.get("summary").and_then(|v| v.as_str()) {
            existing.summary = Some(summary.to_string());
        }
        if let Some(err) = payload.get("error").and_then(|v| v.as_str()) {
            existing.last_error = Some(err.to_string());
        }
        if let Some(deps) = payload.get("dependencies").and_then(|v| v.as_array()) {
            existing.dependencies = deps
                .iter()
                .filter_map(|d| d.as_str().map(String::from))
                .collect();
        }
    } else if let Ok(mut todo) = serde_json::from_value::<TodoItem>(payload.clone()) {
        if todo.status.is_empty() {
            todo.status = payload_status
                .or(status_hint)
                .unwrap_or("queued")
                .to_string();
        }
        todos.push(todo);
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkbenchState {
    pub tabs: Vec<WorkbenchTab>,
    pub active_tab: Option<String>,
    pub proposals: Vec<ProposalView>,
    pub workspace_tree: Vec<WorkspaceNode>,
    pub bottom_panel: BottomPanel,
    pub swarm: Option<Value>,
    pub plan_bundle: Option<Value>,
}

impl WorkbenchState {
    pub fn open_builtin(&mut self, kind: BuiltinTab) {
        let id = kind.id().to_string();
        if !self.tabs.iter().any(|tab| tab.id == id) {
            self.tabs.push(WorkbenchTab {
                id: id.clone(),
                title: kind.title().to_string(),
                kind: TabKind::Builtin(kind),
                dirty: false,
            });
        }
        self.active_tab = Some(id);
    }

    pub fn open_file(&mut self, path: impl Into<String>, content: impl Into<String>) {
        let path = path.into();
        let id = format!("file:{path}");
        let title = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path)
            .to_string();
        let buffer = CodeBuffer::new(Some(path), content);
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.kind = TabKind::File(buffer);
            tab.dirty = false;
        } else {
            self.tabs.push(WorkbenchTab {
                id: id.clone(),
                title,
                kind: TabKind::File(buffer),
                dirty: false,
            });
        }
        self.active_tab = Some(id);
    }

    pub fn open_document(&mut self, path: impl Into<String>) {
        let path = path.into();
        let id = format!("doc:{path}");
        let title = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path)
            .to_string();
        if !self.tabs.iter().any(|tab| tab.id == id) {
            self.tabs.push(WorkbenchTab {
                id: id.clone(),
                title,
                kind: TabKind::Document { path: path.clone() },
                dirty: false,
            });
        }
        self.active_tab = Some(id);
    }

    pub fn update_active_file(&mut self, content: impl Into<String>) -> bool {
        let Some(active) = self.active_tab.clone() else {
            return false;
        };
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == active) {
            if let TabKind::File(buffer) = &mut tab.kind {
                buffer.replace_all(content);
                tab.dirty = true;
                return true;
            }
        }
        false
    }

    pub fn close_tab(&mut self, id: &str) -> bool {
        let before = self.tabs.len();
        self.tabs.retain(|tab| tab.id != id);
        if self.active_tab.as_deref() == Some(id) {
            self.active_tab = self.tabs.last().map(|tab| tab.id.clone());
        }
        self.tabs.len() != before
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchTab {
    pub id: String,
    pub title: String,
    pub kind: TabKind,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TabKind {
    Builtin(BuiltinTab),
    File(CodeBuffer),
    Document { path: String },
    MarkdownPreview { path: String, markdown: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuiltinTab {
    Plan,
    Todo,
    Diff,
    Swarm,
    Readme,
    DocForge,
}

impl BuiltinTab {
    pub fn id(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Todo => "todo",
            Self::Diff => "diff",
            Self::Swarm => "swarm",
            Self::Readme => "readme",
            Self::DocForge => "docforge",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Todo => "Todo",
            Self::Diff => "Diff",
            Self::Swarm => "Swarm",
            Self::Readme => "README",
            Self::DocForge => "文档",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    pub id: String,
    pub path: String,
    pub summary: Option<String>,
    pub diff: Vec<DiffLine>,
}

impl ProposalView {
    pub fn new(
        id: impl Into<String>,
        path: impl Into<String>,
        summary: Option<String>,
        original: &str,
        proposed: &str,
    ) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            summary,
            diff: compute_line_diff(original, proposed),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceNode {
    pub name: String,
    pub path: String,
    pub kind: WorkspaceNodeKind,
    #[serde(default)]
    pub children: Vec<WorkspaceNode>,
    /// Git porcelain status code ("M" / "??" / "A" / "D"), when known.
    #[serde(default)]
    pub git: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceNodeKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BottomPanel {
    pub open: bool,
    pub active: BottomPanelTab,
    pub height: f32,
}

impl Default for BottomPanel {
    fn default() -> Self {
        // Legacy defaults: `moonlit:bottomOpen` is true and the panel opens on
        // the Terminal tab.
        Self {
            open: true,
            active: BottomPanelTab::Terminal,
            height: 260.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BottomPanelTab {
    Problems,
    Output,
    Terminal,
    Logs,
    Metrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsState {
    pub page: SettingsPage,
    pub auto_approve: bool,
    pub web_search_enabled: bool,
    pub submit_with_ctrl_enter: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            // Legacy defaults: autoApprove ?? true, submitCtrlEnter ?? false.
            page: SettingsPage::General,
            auto_approve: true,
            web_search_enabled: false,
            submit_with_ctrl_enter: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsPage {
    General,
    Appearance,
    Plan,
    Agents,
    Tab,
    Models,
    Rules,
    Tools,
    Memory,
}

impl SettingsPage {
    /// Legacy page id (`moonlit:settingsPage` values).
    pub fn as_id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Plan => "plan",
            Self::Agents => "agents",
            Self::Tab => "tab",
            Self::Models => "models",
            Self::Rules => "rules",
            Self::Tools => "tools",
            Self::Memory => "memory",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "appearance" => Self::Appearance,
            "plan" => Self::Plan,
            "agents" => Self::Agents,
            "tab" => Self::Tab,
            "models" => Self::Models,
            "rules" => Self::Rules,
            "tools" => Self::Tools,
            "memory" => Self::Memory,
            _ => Self::General,
        }
    }
}

pub struct NativeShell {
    pub workspace_root: PathBuf,
    backend_child: Option<Child>,
}

impl NativeShell {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            backend_child: None,
        }
    }

    pub fn set_workspace_root(&mut self, root: impl Into<PathBuf>) {
        self.workspace_root = root.into();
    }

    pub fn read_text_file(&self, relative: impl AsRef<Path>) -> std::io::Result<String> {
        std::fs::read_to_string(self.workspace_root.join(relative))
    }

    pub fn write_text_file(
        &self,
        relative: impl AsRef<Path>,
        content: &str,
    ) -> std::io::Result<usize> {
        let path = self.workspace_root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(content.len())
    }

    pub fn backend_command(python: impl AsRef<Path>, backend_script: impl AsRef<Path>) -> Command {
        let mut cmd = Command::new(python.as_ref());
        cmd.arg(backend_script.as_ref())
            .env("AGENT_DEBUG_HTTP_PORT", "8002")
            .env("AGENT_DEBUG_TRANSPORT_MODE", "dedicated-http-ws")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    pub fn attach_backend_child(&mut self, child: Child) {
        self.backend_child = Some(child);
    }

    pub fn stop_backend(&mut self) {
        if let Some(mut child) = self.backend_child.take() {
            let _ = child.kill();
        }
    }
}

impl Drop for NativeShell {
    fn drop(&mut self) {
        self.stop_backend();
    }
}

fn payload_run_id(payload: &Value) -> Option<String> {
    payload
        .get("runId")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn payload_args(payload: &Value) -> Option<&Value> {
    payload.get("args").or_else(|| payload.get("arguments"))
}

fn is_subagent_tool_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("task")
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn subagent_event_id(evt: &DebugEvent) -> Option<String> {
    evt.payload_str("subRunId")
        .or_else(|| evt.payload_str("subagentRunId"))
        .or_else(|| evt.payload_str("subagentId"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn evt(kind: &str, payload: Value) -> DebugEvent {
        DebugEvent {
            event_type: kind.to_string(),
            payload,
            seq: Some(1),
            ..DebugEvent::default()
        }
    }

    #[test]
    fn todo_events_merge_fields_instead_of_replacing() {
        let mut todos: Vec<TodoItem> = Vec::new();

        // todo.created inserts with the payload status.
        merge_todo_event(
            &mut todos,
            "todo.created",
            &json!({ "id": "t1", "title": "调研", "kind": "explore", "status": "queued" }),
        );
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].status, "queued");

        // todo.started without an explicit status falls back to running and
        // keeps the existing title.
        merge_todo_event(&mut todos, "todo.started", &json!({ "id": "t1" }));
        assert_eq!(todos[0].status, "running");
        assert_eq!(todos[0].title, "调研");

        // Partial todo.completed payload must not wipe other fields.
        merge_todo_event(
            &mut todos,
            "todo.completed",
            &json!({ "id": "t1", "summary": "结论" }),
        );
        assert_eq!(todos[0].status, "completed");
        assert_eq!(todos[0].title, "调研");
        assert_eq!(todos[0].summary.as_deref(), Some("结论"));

        // Unknown todos from a status event are inserted with a derived status.
        merge_todo_event(
            &mut todos,
            "todo.failed",
            &json!({ "id": "t2", "title": "实施" }),
        );
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[1].status, "failed");
    }

    #[test]
    fn records_turn_timestamps_and_duration() {
        let mut chat = ChatStore::default();
        let mut start = evt("agent.started", json!({ "runId": "r1" }));
        start.ts = Some("2026-06-10T01:24:49.000Z".into());
        chat.apply_event(&start);
        let mut done = evt("agent.completed", json!({ "runId": "r1", "text": "完成" }));
        done.seq = Some(2);
        done.ts = Some("2026-06-10T01:26:35.000Z".into());
        chat.apply_event(&done);

        let msg = &chat.messages[0];
        assert_eq!(msg.started_ts.as_deref(), Some("2026-06-10T01:24:49.000Z"));
        assert_eq!(msg.finished_ts.as_deref(), Some("2026-06-10T01:26:35.000Z"));
        assert_eq!(msg.work_duration_secs(), Some(106));
    }

    #[test]
    fn iso_parse_handles_offsets_fractions_and_garbage() {
        let utc = iso_to_epoch_secs("2026-06-10T00:00:00Z").unwrap();
        let offset = iso_to_epoch_secs("2026-06-10T02:00:00+02:00").unwrap();
        assert_eq!(utc, offset);
        let frac = iso_to_epoch_secs("2026-06-10T00:00:00.500Z").unwrap();
        assert!((frac - utc - 0.5).abs() < 1e-9);
        assert!(iso_to_epoch_secs("not-a-timestamp").is_none());
        assert!(iso_to_epoch_secs("").is_none());
    }

    #[test]
    fn chat_streams_token_and_completes() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt(
            "agent.token.stream.delta",
            json!({ "runId": "r1", "delta": "hel" }),
        ));
        chat.apply_event(&evt(
            "agent.token.stream.delta",
            json!({ "runId": "r1", "delta": "lo" }),
        ));
        chat.apply_event(&evt("agent.completed", json!({ "runId": "r1" })));
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].text, "hello");
        assert_eq!(chat.messages[0].status, MessageStatus::Completed);
    }

    #[test]
    fn duplicate_stream_events_are_ignored() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        let token = evt(
            "agent.token.stream.delta",
            json!({ "runId": "r1", "delta": "hi" }),
        );
        let reasoning = evt(
            "agent.reasoning.delta",
            json!({ "runId": "r1", "delta": "think" }),
        );

        chat.apply_event(&token);
        chat.apply_event(&token);
        chat.apply_event(&reasoning);
        chat.apply_event(&reasoning);

        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].text, "hi");
        assert_eq!(chat.messages[0].reasoning, "think");
        assert_eq!(chat.messages[0].blocks.len(), 2);
        match &chat.messages[0].blocks[0] {
            ChatBlock::Text { text } => assert_eq!(text, "hi"),
            other => panic!("expected text block, got {other:?}"),
        }
        match &chat.messages[0].blocks[1] {
            ChatBlock::Reasoning { text } => assert_eq!(text, "think"),
            other => panic!("expected reasoning block, got {other:?}"),
        }
    }

    #[test]
    fn user_message_is_replayed() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt(
            "composer.user.message",
            json!({ "runId": "r1", "text": "build it" }),
        ));
        assert_eq!(chat.messages[0].role, ChatRole::User);
        assert_eq!(chat.messages[0].text, "build it");
    }

    #[test]
    fn replayed_user_replaces_local_echo() {
        let mut chat = ChatStore::default();
        chat.push_local_user("你好".into());
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt(
            "composer.user.message",
            json!({ "runId": "r1", "text": "你好" }),
        ));
        chat.apply_event(&evt(
            "agent.completed",
            json!({ "runId": "r1", "text": "你好！" }),
        ));

        assert_eq!(
            chat.messages
                .iter()
                .filter(|m| m.role == ChatRole::User)
                .count(),
            1
        );
        assert_eq!(chat.messages[0].role, ChatRole::User);
        assert_eq!(chat.messages[1].role, ChatRole::Assistant);
    }

    #[test]
    fn replayed_user_is_inserted_before_existing_agent() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt(
            "composer.user.message",
            json!({ "runId": "r1", "text": "你好" }),
        ));

        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, ChatRole::User);
        assert_eq!(chat.messages[1].role, ChatRole::Assistant);
    }

    #[test]
    fn task_tool_invocation_creates_running_subagent_block() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "Task",
                "args": {
                    "description": "探索后端架构",
                    "prompt": "请探索 backend 目录。"
                }
            }),
        ));

        assert_eq!(chat.messages.len(), 1);
        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                id,
                label,
                prompt,
                status,
                ..
            } => {
                assert_eq!(id, "task-1");
                assert_eq!(label, "探索后端架构");
                assert_eq!(prompt, "请探索 backend 目录。");
                assert_eq!(*status, BlockStatus::Running);
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn lowercase_task_tool_invocation_creates_subagent_block() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "task",
                "args": {
                    "prompt": "探索项目中的 backend-rs 目录。\n请完成以下任务。"
                }
            }),
        ));

        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                id,
                label,
                prompt,
                status,
                ..
            } => {
                assert_eq!(id, "task-1");
                assert_eq!(label, "探索项目中的 backend-rs 目录。");
                assert_eq!(prompt, "探索项目中的 backend-rs 目录。\n请完成以下任务。");
                assert_eq!(*status, BlockStatus::Running);
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn subagent_completed_updates_summary_and_status() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "Task",
                "args": { "description": "探索后端架构", "prompt": "请探索 backend 目录。" }
            }),
        ));
        chat.apply_event(&evt(
            "subagent.completed",
            json!({
                "runId": "r1",
                "subagentRunId": "task-1",
                "summary": "backend 使用 FastAPI。"
            }),
        ));

        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                summary, status, ..
            } => {
                assert_eq!(summary, "backend 使用 FastAPI。");
                assert_eq!(*status, BlockStatus::Done);
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn task_tool_completed_updates_subagent_block() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "task",
                "args": { "prompt": "探索 backend-rs。" }
            }),
        ));
        chat.apply_event(&evt(
            "agent.tool.completed",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "output": "backend-rs 是 Rust 后端。"
            }),
        ));

        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                summary, status, ..
            } => {
                assert_eq!(summary, "backend-rs 是 Rust 后端。");
                assert_eq!(*status, BlockStatus::Done);
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn subagent_run_agent_events_are_hidden_from_chat_stream() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "name": "task",
                "args": { "description": "全面探索项目结构", "prompt": "探索项目。" }
            }),
        ));
        chat.apply_event(&evt(
            "subagent.started",
            json!({
                "subRunId": "subrun-1",
                "parentRunId": "r1",
                "description": "全面探索项目结构"
            }),
        ));
        chat.apply_event(&evt("agent.started", json!({ "runId": "subrun-1" })));
        chat.apply_event(&evt(
            "agent.reasoning.delta",
            json!({ "runId": "subrun-1", "delta": "子 agent 内部思考" }),
        ));
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({ "runId": "subrun-1", "name": "list_dir", "args": { "path": "." } }),
        ));
        chat.apply_event(&evt(
            "agent.completed",
            json!({ "runId": "subrun-1", "text": "子 agent 内部最终回答" }),
        ));
        chat.apply_event(&evt(
            "subagent.completed",
            json!({
                "subRunId": "subrun-1",
                "parentRunId": "r1",
                "summary": "结构摘要"
            }),
        ));

        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].run_id.as_deref(), Some("r1"));
        assert_eq!(chat.messages[0].blocks.len(), 1);
        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                id,
                label,
                summary,
                status,
                work,
                ..
            } => {
                assert_eq!(id, "subrun-1");
                assert_eq!(label, "全面探索项目结构");
                assert_eq!(summary, "结构摘要");
                assert_eq!(*status, BlockStatus::Done);
                assert_eq!(work.len(), 3);
                assert!(
                    matches!(&work[0], ChatBlock::Reasoning { text } if text == "子 agent 内部思考")
                );
                assert!(matches!(
                    &work[1],
                    ChatBlock::Tool {
                        name,
                        status: BlockStatus::Running,
                        ..
                    } if name == "list_dir"
                ));
                assert!(
                    matches!(&work[2], ChatBlock::Text { text } if text == "子 agent 内部最终回答")
                );
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn parent_tagged_subagent_agent_events_do_not_duplicate_parent_stream() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "Task",
                "args": { "description": "全面探索项目结构", "prompt": "探索项目。" }
            }),
        ));
        chat.apply_event(&evt(
            "subagent.created",
            json!({
                "runId": "r1",
                "subagentId": "sub-1",
                "parentRunId": "r1",
                "parentToolCallId": "task-1",
                "description": "全面探索项目结构",
                "prompt": "探索项目。"
            }),
        ));
        chat.apply_event(&evt(
            "agent.reasoning.delta",
            json!({
                "runId": "r1",
                "parentToolCallId": "task-1",
                "subagentId": "sub-1",
                "delta": "子代理内部思考"
            }),
        ));
        chat.apply_event(&evt(
            "agent.token.stream.delta",
            json!({
                "runId": "r1",
                "parentToolCallId": "task-1",
                "subagentId": "sub-1",
                "delta": "子代理内部回答"
            }),
        ));
        chat.apply_event(&evt(
            "agent.completed",
            json!({
                "runId": "r1",
                "parentToolCallId": "task-1",
                "subagentId": "sub-1",
                "text": "子代理内部最终回答"
            }),
        ));
        chat.apply_event(&evt(
            "subagent.completed",
            json!({
                "runId": "r1",
                "subagentId": "sub-1",
                "parentRunId": "r1",
                "parentToolCallId": "task-1",
                "summary": "结构摘要"
            }),
        ));

        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].text, "");
        assert_eq!(chat.messages[0].reasoning, "");
        assert_eq!(chat.messages[0].blocks.len(), 1);
        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                id,
                summary,
                status,
                work,
                ..
            } => {
                assert_eq!(id, "sub-1");
                assert_eq!(summary, "结构摘要");
                assert_eq!(*status, BlockStatus::Done);
                assert_eq!(work.len(), 2);
                assert!(
                    matches!(&work[0], ChatBlock::Reasoning { text } if text == "子代理内部思考")
                );
                assert!(
                    matches!(&work[1], ChatBlock::Text { text } if text == "子代理内部最终回答")
                );
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn parent_tagged_subagent_tools_are_recorded_inside_work() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "Task",
                "arguments": { "description": "搜索入口", "prompt": "找入口。" }
            }),
        ));
        let mut created = evt(
            "subagent.created",
            json!({
                "runId": "r1",
                "subagentId": "sub-1",
                "parentRunId": "r1",
                "parentToolCallId": "task-1",
                "description": "搜索入口"
            }),
        );
        created.ts = Some("2026-06-10T01:00:00.000Z".into());
        chat.apply_event(&created);
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "parentToolCallId": "task-1",
                "subagentId": "sub-1",
                "toolCallId": "grep-1",
                "name": "grep",
                "arguments": { "pattern": "main", "path": "backend" }
            }),
        ));
        chat.apply_event(&evt(
            "agent.tool.completed",
            json!({
                "runId": "r1",
                "parentToolCallId": "task-1",
                "subagentId": "sub-1",
                "toolCallId": "grep-1",
                "output": "backend/src/main.py"
            }),
        ));
        let mut task_done = evt(
            "agent.tool.completed",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "subagentId": "sub-1",
                "output": "找到入口。"
            }),
        );
        task_done.ts = Some("2026-06-10T01:00:12.000Z".into());
        chat.apply_event(&task_done);

        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                summary,
                status,
                work,
                ..
            } => {
                assert_eq!(summary, "找到入口。");
                assert_eq!(*status, BlockStatus::Done);
                assert_eq!(
                    chat.messages[0].blocks[0].subagent_work_duration_secs(),
                    Some(12)
                );
                assert_eq!(work.len(), 1);
                match &work[0] {
                    ChatBlock::Tool {
                        name,
                        args,
                        result,
                        status,
                        ..
                    } => {
                        assert_eq!(name, "grep");
                        assert!(args.contains("\"pattern\": \"main\""));
                        assert_eq!(result, "backend/src/main.py");
                        assert_eq!(*status, BlockStatus::Done);
                    }
                    other => panic!("expected nested tool block, got {other:?}"),
                }
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_replay_events_are_not_appended_twice() {
        let mut delta = evt(
            "agent.token.stream.delta",
            json!({ "runId": "r1", "delta": "hello" }),
        );
        delta.id = Some("evt-token-1".into());
        delta.seq = Some(2);
        let mut snapshot = DesignSnapshot::default();
        snapshot.events = vec![
            evt("agent.started", json!({ "runId": "r1" })),
            delta.clone(),
        ];

        let mut state = AgentIdeState::default();
        state.hydrate_snapshot(snapshot);
        state.apply_event(delta);

        assert_eq!(state.chat.messages.len(), 1);
        assert_eq!(state.chat.messages[0].text, "hello");
        assert_eq!(state.chat.messages[0].blocks.len(), 1);
    }

    #[test]
    fn stale_snapshot_does_not_replace_current_session_history() {
        let mut state = AgentIdeState::default();
        state.active_session_id = Some("current".into());
        state.chat.push_local_user("保留当前历史".into());

        let mut snapshot = DesignSnapshot::default();
        snapshot.active_session = Some(DebugSession {
            id: "previous".into(),
            title: "旧会话".into(),
            ..DebugSession::default()
        });
        snapshot.events = vec![evt(
            "composer.user.message",
            json!({ "runId": "old-run", "text": "旧历史" }),
        )];

        assert!(!state.hydrate_snapshot_if_current(snapshot));
        assert_eq!(state.active_session_id.as_deref(), Some("current"));
        assert_eq!(state.chat.messages.len(), 1);
        assert_eq!(state.chat.messages[0].text, "保留当前历史");
    }

    #[test]
    fn stale_subscription_event_is_ignored_for_current_session() {
        let mut state = AgentIdeState::default();
        state.active_session_id = Some("current".into());
        let mut event = evt(
            "composer.user.message",
            json!({ "runId": "old-run", "text": "旧订阅事件" }),
        );
        event.session_id = Some("previous".into());

        assert!(!state.apply_event(event));
        assert!(state.chat.messages.is_empty());
    }

    #[test]
    fn subagent_failed_marks_block_as_error() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt(
            "agent.tool.invoked",
            json!({
                "runId": "r1",
                "toolCallId": "task-1",
                "name": "Task",
                "args": { "description": "探索后端架构", "prompt": "请探索 backend 目录。" }
            }),
        ));
        chat.apply_event(&evt(
            "subagent.failed",
            json!({
                "runId": "r1",
                "subagentRunId": "task-1",
                "message": "子 agent 执行失败"
            }),
        ));

        match &chat.messages[0].blocks[0] {
            ChatBlock::Subagent {
                summary, status, ..
            } => {
                assert_eq!(summary, "子 agent 执行失败");
                assert_eq!(*status, BlockStatus::Error);
            }
            other => panic!("expected subagent block, got {other:?}"),
        }
    }

    #[test]
    fn truncate_at_drops_message_and_tail() {
        let mut chat = ChatStore::default();
        let mut user2 = evt(
            "composer.user.message",
            json!({ "runId": "r2", "text": "第二轮" }),
        );
        user2.id = Some("evt-user-2".into());
        chat.apply_event(&evt(
            "composer.user.message",
            json!({ "runId": "r1", "text": "第一轮" }),
        ));
        chat.apply_event(&evt(
            "agent.completed",
            json!({ "runId": "r1", "text": "好的" }),
        ));
        chat.apply_event(&user2);
        chat.apply_event(&evt(
            "agent.completed",
            json!({ "runId": "r2", "text": "收到" }),
        ));
        assert_eq!(chat.messages.len(), 4);

        chat.truncate_at("evt-user-2");

        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].text, "第一轮");
        assert!(chat.active_run_id.is_none());
    }

    #[test]
    fn workbench_opens_and_updates_file_tab() {
        let mut wb = WorkbenchState::default();
        wb.open_builtin(BuiltinTab::Plan);
        wb.open_file("src/main.rs", "fn main() {}\n");
        assert_eq!(wb.active_tab.as_deref(), Some("file:src/main.rs"));
        assert!(wb.update_active_file("fn main(){println!(\"hi\");}\n"));
        assert!(wb.tabs.iter().any(|tab| tab.dirty));
        assert!(wb.close_tab("plan"));
    }

    #[test]
    fn proposal_view_computes_diff() {
        let proposal = ProposalView::new("p1", "a.txt", None, "a\nb\n", "a\nc\n");
        assert!(proposal
            .diff
            .iter()
            .any(|line| line.tag == moonlit_uikit::DiffTag::Insert));
    }

    #[test]
    fn native_shell_reads_and_writes_workspace_file() {
        let root = std::env::temp_dir().join(format!("moonlit-shell-{}", std::process::id()));
        let shell = NativeShell::new(&root);
        shell.write_text_file("nested/a.txt", "hello").unwrap();
        assert_eq!(shell.read_text_file("nested/a.txt").unwrap(), "hello");
    }
}
