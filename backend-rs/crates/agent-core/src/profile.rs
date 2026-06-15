//! Agent profiles: the project is positioned as three distinct agents that
//! share one runtime but differ in prompt engineering, tool surface and the
//! interaction (composer) modes they support.
//!
//! - [`AgentKind::General`]  — a general-purpose assistant (chat-first,
//!   read-only investigation + web, no file writes / command execution).
//! - [`AgentKind::Document`] — a document-processing agent (research + write
//!   structured documents; no shell, no code-patch tools).
//! - [`AgentKind::Coding`]  — the vibe-coding agent (full tool surface; this
//!   is the historical behavior and the default for legacy sessions).
//!
//! The kind is stored on the session (`DebugSession.agent_kind`) so it is
//! fixed for the life of a conversation and drives both
//! [`Runtime::allowed_tools`](crate::engine::Runtime) and the system prompt.

/// Which of the three agent profiles a session runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    General,
    Document,
    Coding,
}

impl Default for AgentKind {
    fn default() -> Self {
        AgentKind::Coding
    }
}

impl AgentKind {
    /// Parse a wire string. Unknown / empty values fall back to `Coding` so a
    /// missing `agentKind` keeps the legacy code-agent behavior.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "general" | "chat" | "assistant" => AgentKind::General,
            "document" | "doc" | "writer" => AgentKind::Document,
            _ => AgentKind::Coding,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::General => "general",
            AgentKind::Document => "document",
            AgentKind::Coding => "coding",
        }
    }

    /// Human-facing label for API listings.
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::General => "通用助手",
            AgentKind::Document => "文档处理",
            AgentKind::Coding => "编码 (Vibe Coding)",
        }
    }

    pub fn all() -> [AgentKind; 3] {
        [AgentKind::General, AgentKind::Document, AgentKind::Coding]
    }
}

/// Resolved capabilities for an [`AgentKind`]: the registry tool whitelist plus
/// flags for the runtime-managed tools (todo / task / edit / memory) and the
/// set of composer modes the profile understands.
pub struct AgentProfile {
    pub kind: AgentKind,
}

/// Registry tools available to read-only / web-capable profiles.
const READ_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "read_skill",
    "read_document",
];
const WEB_TOOLS: &[&str] = &["web_search", "web_fetch"];
/// Document profile may additionally create/write document files, including the
/// Office/PDF document tools.
const DOC_WRITE_TOOLS: &[&str] = &[
    "write_file",
    "create_document",
    "create_word_document",
    "create_presentation",
    "create_pdf",
    "edit_word_document",
    "edit_presentation",
];

impl AgentProfile {
    pub fn new(kind: AgentKind) -> Self {
        AgentProfile { kind }
    }

    pub fn from_kind_str(s: &str) -> Self {
        AgentProfile::new(AgentKind::from_str(s))
    }

    /// Whether a *registry* tool (workspace / command / web / skill) is part of
    /// this profile's surface. `coding` allows everything (historical
    /// behavior); the other two profiles use an explicit allow-list.
    pub fn registry_tool_allowed(&self, name: &str) -> bool {
        match self.kind {
            AgentKind::Coding => true,
            AgentKind::Document => {
                READ_TOOLS.contains(&name)
                    || WEB_TOOLS.contains(&name)
                    || DOC_WRITE_TOOLS.contains(&name)
            }
            AgentKind::General => READ_TOOLS.contains(&name) || WEB_TOOLS.contains(&name),
        }
    }

    /// Whether the runtime-managed structured-edit tools (`str_replace_edit` /
    /// `apply_patch`) ride along. Only the coding agent does code edits; the
    /// document agent writes whole files via `write_file` / `create_document`.
    pub fn wants_edit_tools(&self) -> bool {
        matches!(self.kind, AgentKind::Coding)
    }

    /// Whether the runtime-managed checklist tools (`todo_write` /
    /// `todo_update`) are exposed. The general chat agent doesn't plan with
    /// todos; coding and document agents do.
    pub fn wants_todo_tools(&self) -> bool {
        !matches!(self.kind, AgentKind::General)
    }

    /// Whether the `task` delegation tool is exposed. All three profiles can
    /// delegate read-only investigation / research subagents.
    pub fn wants_task_tool(&self) -> bool {
        true
    }

    /// Whether the runtime-managed memory tools (`memory_write` /
    /// `memory_search` / `memory_delete`) are exposed. Enabled for every
    /// profile so each can build up durable knowledge.
    pub fn wants_memory_tools(&self) -> bool {
        true
    }

    /// Composer modes the profile supports. Coding keeps the full set; the
    /// other two are conversational and only expose `ask` / `build`.
    pub fn allowed_modes(&self) -> &'static [&'static str] {
        match self.kind {
            AgentKind::Coding => &["build", "plan", "debug", "ask", "multitask"],
            AgentKind::Document | AgentKind::General => &["ask", "build"],
        }
    }

    /// Clamp a requested composer mode to one the profile supports. Modes a
    /// profile doesn't understand (e.g. `plan` / `debug` for the chat agent)
    /// degrade to `build`, except `ask` which always stays read-only.
    pub fn normalize_mode(&self, mode: &str) -> String {
        let m = mode.trim();
        if self.allowed_modes().contains(&m) {
            return m.to_string();
        }
        if m == "ask" {
            return "ask".to_string();
        }
        "build".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_defaults_to_coding() {
        assert_eq!(AgentKind::from_str("general"), AgentKind::General);
        assert_eq!(AgentKind::from_str("document"), AgentKind::Document);
        assert_eq!(AgentKind::from_str("coding"), AgentKind::Coding);
        assert_eq!(AgentKind::from_str(""), AgentKind::Coding);
        assert_eq!(AgentKind::from_str("nonsense"), AgentKind::Coding);
    }

    #[test]
    fn coding_allows_all_registry_tools() {
        let p = AgentProfile::new(AgentKind::Coding);
        for t in [
            "read_file",
            "write_file",
            "run_command",
            "delete_file",
            "create_document",
        ] {
            assert!(p.registry_tool_allowed(t), "coding should allow {t}");
        }
        assert!(p.wants_edit_tools());
        assert!(p.wants_todo_tools());
    }

    #[test]
    fn document_allows_writes_but_no_commands_or_code_edits() {
        let p = AgentProfile::new(AgentKind::Document);
        assert!(p.registry_tool_allowed("read_file"));
        assert!(p.registry_tool_allowed("write_file"));
        assert!(p.registry_tool_allowed("create_document"));
        assert!(p.registry_tool_allowed("web_search"));
        assert!(!p.registry_tool_allowed("run_command"));
        assert!(!p.registry_tool_allowed("delete_file"));
        assert!(!p.wants_edit_tools());
        assert!(p.wants_todo_tools());
    }

    #[test]
    fn general_is_read_only_plus_web() {
        let p = AgentProfile::new(AgentKind::General);
        assert!(p.registry_tool_allowed("read_file"));
        assert!(p.registry_tool_allowed("web_fetch"));
        assert!(!p.registry_tool_allowed("write_file"));
        assert!(!p.registry_tool_allowed("run_command"));
        assert!(!p.wants_edit_tools());
        assert!(!p.wants_todo_tools());
    }

    #[test]
    fn non_coding_modes_downgrade() {
        let doc = AgentProfile::new(AgentKind::Document);
        assert_eq!(doc.normalize_mode("plan"), "build");
        assert_eq!(doc.normalize_mode("debug"), "build");
        assert_eq!(doc.normalize_mode("multitask"), "build");
        assert_eq!(doc.normalize_mode("ask"), "ask");
        assert_eq!(doc.normalize_mode("build"), "build");

        let coding = AgentProfile::new(AgentKind::Coding);
        assert_eq!(coding.normalize_mode("plan"), "plan");
        assert_eq!(coding.normalize_mode("multitask"), "multitask");
    }
}
