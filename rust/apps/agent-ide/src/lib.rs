pub mod app;
pub mod ui;

use moonlit_core::models::{DebugEvent, DebugSession, DesignSnapshot, TodoItem};
use moonlit_uikit::{compute_line_diff, CodeBuffer, CommandPalette, DiffLine, SplitPane, Theme, ToastStack};
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
}

/// One entry of the assistant action timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ChatBlock {
    Text { text: String },
    Reasoning { text: String },
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
    },
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
}

impl ChatStore {
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
        });
    }

    /// Drop `message_id` and everything after it (optimistic local echo of a
    /// backend revert before an edited resend).
    pub fn truncate_at(&mut self, message_id: &str) {
        if let Some(idx) = self.messages.iter().position(|m| m.id == message_id) {
            self.messages.truncate(idx);
            self.active_run_id = None;
        }
    }

    pub fn apply_event(&mut self, evt: &DebugEvent) {
        let time = hhmm_from_ts(evt.ts.as_deref());
        match evt.event_type.as_str() {
            "composer.user.message" => {
                let text = evt.payload_str("text").unwrap_or_default().to_string();
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let user = ChatMessage {
                    id: evt.id.clone().unwrap_or_else(|| format!("user-{}", evt.seq.unwrap_or(0))),
                    role: ChatRole::User,
                    text,
                    reasoning: String::new(),
                    run_id: run_id.clone(),
                    status: MessageStatus::Completed,
                    time,
                    blocks: Vec::new(),
                };
                self.upsert_user(user, run_id.as_deref());
            }
            "agent.started" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                self.active_run_id = run_id.clone();
                let msg = self.ensure_assistant(run_id);
                if msg.time.is_empty() {
                    msg.time = time;
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
                if name == "Task" {
                    let prompt = evt
                        .payload
                        .get("args")
                        .and_then(|a| a.get("prompt"))
                        .and_then(Value::as_str)
                        .or_else(|| evt.payload_str("prompt"))
                        .unwrap_or_default()
                        .to_string();
                    let label = evt
                        .payload
                        .get("args")
                        .and_then(|a| a.get("description"))
                        .and_then(Value::as_str)
                        .unwrap_or("子代理任务")
                        .to_string();
                    msg.blocks.push(ChatBlock::Subagent {
                        id,
                        label,
                        prompt,
                        summary: String::new(),
                        status: BlockStatus::Running,
                    });
                } else {
                    let args = evt
                        .payload
                        .get("args")
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
                if let Some(block) = msg.blocks.iter_mut().rev().find(|b| match b {
                    ChatBlock::Tool { id, status, .. } => {
                        call_id.as_deref().map(|c| c == id).unwrap_or(*status == BlockStatus::Running)
                    }
                    _ => false,
                }) {
                    if let ChatBlock::Tool { result, status, .. } = block {
                        *result = output;
                        *status = if failed { BlockStatus::Error } else { BlockStatus::Done };
                    }
                }
            }
            "subagent.created" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let sub_id = evt
                    .payload_str("subagentRunId")
                    .or_else(|| evt.payload_str("parentToolCallId"))
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("sub-{}", evt.seq.unwrap_or(0)));
                let label = evt.payload_str("label").unwrap_or("子代理任务").to_string();
                let prompt = evt.payload_str("prompt").unwrap_or_default().to_string();
                let msg = self.ensure_assistant(run_id);
                if !msg.blocks.iter().any(|b| matches!(b, ChatBlock::Subagent { id, .. } if *id == sub_id)) {
                    msg.blocks.push(ChatBlock::Subagent {
                        id: sub_id,
                        label,
                        prompt,
                        summary: String::new(),
                        status: BlockStatus::Running,
                    });
                }
            }
            "subagent.completed" | "subagent.failed" => {
                let failed = evt.event_type == "subagent.failed";
                let sub_id = evt
                    .payload_str("subagentRunId")
                    .or_else(|| evt.payload_str("parentToolCallId"))
                    .map(str::to_string);
                let new_summary = evt
                    .payload_str("summary")
                    .or_else(|| evt.payload_str("message"))
                    .unwrap_or_default()
                    .to_string();
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                if let Some(ChatBlock::Subagent { summary, status, .. }) =
                    msg.blocks.iter_mut().rev().find(|b| match b {
                        ChatBlock::Subagent { id, status, .. } => sub_id
                            .as_deref()
                            .map(|c| c == id)
                            .unwrap_or(*status == BlockStatus::Running),
                        _ => false,
                    })
                {
                    *status = if failed { BlockStatus::Error } else { BlockStatus::Done };
                    if !new_summary.is_empty() {
                        *summary = new_summary;
                    }
                }
            }
            "agent.completed" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let text = evt.payload_str("text").unwrap_or_default();
                let msg = self.ensure_assistant(run_id);
                if !text.is_empty() {
                    msg.text = text.to_string();
                    if !matches!(msg.blocks.last(), Some(ChatBlock::Text { .. })) {
                        msg.blocks.push(ChatBlock::Text { text: text.to_string() });
                    }
                }
                msg.status = MessageStatus::Completed;
                self.active_run_id = None;
            }
            "agent.failed" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                msg.status = MessageStatus::Failed;
                self.active_run_id = None;
            }
            "agent.cancelled" => {
                let run_id = payload_run_id(&evt.payload).or_else(|| evt.correlation_id.clone());
                let msg = self.ensure_assistant(run_id);
                msg.status = MessageStatus::Cancelled;
                self.active_run_id = None;
            }
            _ => {}
        }
        self.normalize_messages();
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
            if let Some(idx) = self
                .messages
                .iter()
                .position(|m| m.role == ChatRole::Assistant && m.run_id.as_deref() == Some(run_id_ref))
            {
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
                let duplicate_run_user = self.messages[scan].role == ChatRole::User && self.messages[scan].run_id == run_id;
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
            let user_after_idx = self.messages.iter().enumerate().skip(assistant_idx + 1).find_map(|(idx, m)| {
                (m.role == ChatRole::User && m.run_id.as_deref() == Some(run_id.as_str())).then_some(idx)
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
}

/// "2026-06-10T01:24:49…" → "01:24". Empty when absent/malformed.
fn hhmm_from_ts(ts: Option<&str>) -> String {
    ts.and_then(|s| s.get(11..16)).unwrap_or_default().to_string()
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
        self.chat.messages.clear();
        for evt in &snapshot.events {
            self.chat.apply_event(evt);
        }
    }

    pub fn apply_event(&mut self, evt: DebugEvent) {
        self.chat.apply_event(&evt);
        match evt.event_type.as_str() {
            "todo.created" | "todo.updated" | "todo.running" | "todo.completed" | "todo.failed" => {
                if let Ok(todo) = serde_json::from_value::<TodoItem>(evt.payload.clone()) {
                    if let Some(existing) = self.todos.iter_mut().find(|t| t.id == todo.id) {
                        *existing = todo;
                    } else {
                        self.todos.push(todo);
                    }
                }
            }
            _ => {}
        }
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
}

impl BuiltinTab {
    pub fn id(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Todo => "todo",
            Self::Diff => "diff",
            Self::Swarm => "swarm",
            Self::Readme => "readme",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Todo => "Todo",
            Self::Diff => "Diff",
            Self::Swarm => "Swarm",
            Self::Readme => "README",
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

    pub fn write_text_file(&self, relative: impl AsRef<Path>, content: &str) -> std::io::Result<usize> {
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
    fn chat_streams_token_and_completes() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt("agent.token.stream.delta", json!({ "runId": "r1", "delta": "hel" })));
        chat.apply_event(&evt("agent.token.stream.delta", json!({ "runId": "r1", "delta": "lo" })));
        chat.apply_event(&evt("agent.completed", json!({ "runId": "r1" })));
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].text, "hello");
        assert_eq!(chat.messages[0].status, MessageStatus::Completed);
    }

    #[test]
    fn user_message_is_replayed() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("composer.user.message", json!({ "runId": "r1", "text": "build it" })));
        assert_eq!(chat.messages[0].role, ChatRole::User);
        assert_eq!(chat.messages[0].text, "build it");
    }

    #[test]
    fn replayed_user_replaces_local_echo() {
        let mut chat = ChatStore::default();
        chat.push_local_user("你好".into());
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt("composer.user.message", json!({ "runId": "r1", "text": "你好" })));
        chat.apply_event(&evt("agent.completed", json!({ "runId": "r1", "text": "你好！" })));

        assert_eq!(chat.messages.iter().filter(|m| m.role == ChatRole::User).count(), 1);
        assert_eq!(chat.messages[0].role, ChatRole::User);
        assert_eq!(chat.messages[1].role, ChatRole::Assistant);
    }

    #[test]
    fn replayed_user_is_inserted_before_existing_agent() {
        let mut chat = ChatStore::default();
        chat.apply_event(&evt("agent.started", json!({ "runId": "r1" })));
        chat.apply_event(&evt("composer.user.message", json!({ "runId": "r1", "text": "你好" })));

        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, ChatRole::User);
        assert_eq!(chat.messages[1].role, ChatRole::Assistant);
    }

    #[test]
    fn truncate_at_drops_message_and_tail() {
        let mut chat = ChatStore::default();
        let mut user2 = evt("composer.user.message", json!({ "runId": "r2", "text": "第二轮" }));
        user2.id = Some("evt-user-2".into());
        chat.apply_event(&evt("composer.user.message", json!({ "runId": "r1", "text": "第一轮" })));
        chat.apply_event(&evt("agent.completed", json!({ "runId": "r1", "text": "好的" })));
        chat.apply_event(&user2);
        chat.apply_event(&evt("agent.completed", json!({ "runId": "r2", "text": "收到" })));
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
        assert!(proposal.diff.iter().any(|line| line.tag == moonlit_uikit::DiffTag::Insert));
    }

    #[test]
    fn native_shell_reads_and_writes_workspace_file() {
        let root = std::env::temp_dir().join(format!("moonlit-shell-{}", std::process::id()));
        let shell = NativeShell::new(&root);
        shell.write_text_file("nested/a.txt", "hello").unwrap();
        assert_eq!(shell.read_text_file("nested/a.txt").unwrap(), "hello");
    }
}
