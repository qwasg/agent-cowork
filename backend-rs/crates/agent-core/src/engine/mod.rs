//! Agent engine: the tool loop, streaming, run lifecycle, cancellation and
//! the plan DAG executor (port of `runtime.py`), decomposed by concern:
//!
//! - [`control`]: run registration, pause/resume/cancel.
//! - [`react`]: composer chat entry + the core tool loop.
//! - [`dispatch`]: per-tool-call dispatch, permission gating, approval.
//! - [`subagent`]: the `task` tool (nested agent loops).
//! - [`todos`]: the runtime-handled `todo_write` / `todo_update` tools.
//! - [`plan_exec`]: the plan DAG executor.

pub mod control;
pub mod dispatch;
pub mod edits;
pub mod memory;
pub mod plan_exec;
pub mod react;
pub mod subagent;
pub mod todos;
pub mod turn;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_config::Config;
use agent_protocol::events::EventDraft;
use agent_providers::ProviderExecutionService;
use agent_store::{AsyncStore, EventBus, Store};
use agent_tools::shell::ShellManager;
use agent_tools::{ToolContext, ToolRegistry, WebConfig};

use crate::code_edit::ProposalRegistry;
use crate::hooks::HookRunner;
use crate::memory::MemoryService;
use crate::permission::PermissionService;
use crate::profile::{AgentKind, AgentProfile};
use crate::search_config::SearchConfigService;
use crate::session::SessionService;
use crate::todo::TodoEngine;

pub use control::RunControl;
pub use plan_exec::PlanOutcome;
pub use react::TurnOutcome;
pub use turn::{Turn, TurnItem};

pub struct Runtime {
    pub providers: Arc<ProviderExecutionService>,
    pub tools: Arc<ToolRegistry>,
    pub bus: Arc<EventBus>,
    pub sessions: Arc<SessionService>,
    pub permissions: Arc<PermissionService>,
    pub todos: Arc<TodoEngine>,
    /// Durable cross-session structured memory.
    pub memory: Arc<MemoryService>,
    pub proposals: Arc<ProposalRegistry>,
    pub store: Arc<Store>,
    /// Async write facade (dedicated writer thread) — use for writes issued
    /// from the async tool loop so commits never block the runtime.
    pub astore: Arc<AsyncStore>,
    pub web: WebConfig,
    pub search_config: Arc<SearchConfigService>,
    pub skill_dirs: Vec<PathBuf>,
    pub default_workspace_root: PathBuf,
    /// Oversized tool outputs spill here (`data/tool-outputs/{run_id}/`).
    pub tool_output_dir: PathBuf,
    /// Shared shell manager (persistent sessions + background jobs).
    pub shell: Arc<ShellManager>,
    /// Lifecycle hooks (PreToolUse / PostToolUse / TurnEnd).
    pub hooks: Arc<HookRunner>,
    /// Generic MCP client manager (`data/mcp.json`).
    pub mcp: Arc<agent_mcp::McpManager>,
    /// Subagent profiles: builtin + disk (`data/agents/*.md`).
    pub subagents: crate::subagents::SubagentRegistry,
    pub parallel_limit: usize,
    pub edit_parallel: usize,
    pub tool_loop_steps: usize,
    pub repeated_tool_limit: usize,
    /// How deep `task` (subagent) nesting may go (1 = subagents cannot spawn
    /// further subagents).
    pub subagent_max_depth: usize,
    /// Loop guard: consecutive all-failed tool steps before intervention.
    pub empty_progress_limit: usize,
    /// Plan executor: retries per failed todo (0 = no retry).
    pub todo_retry_limit: usize,
    /// Plan executor: per-todo wall-clock timeout in seconds (0 = none).
    pub todo_timeout_secs: u64,
    pub stream: bool,
    pub context_budget_tokens: usize,
    pub approval_timeout_secs: u64,
    pub(crate) controls: Mutex<HashMap<String, Arc<RunControl>>>,
}

impl Runtime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: &Config,
        providers: Arc<ProviderExecutionService>,
        tools: Arc<ToolRegistry>,
        bus: Arc<EventBus>,
        sessions: Arc<SessionService>,
        permissions: Arc<PermissionService>,
        todos: Arc<TodoEngine>,
        proposals: Arc<ProposalRegistry>,
        store: Arc<Store>,
        astore: Arc<AsyncStore>,
        skill_dirs: Vec<PathBuf>,
        search_config: Arc<SearchConfigService>,
    ) -> Arc<Self> {
        Arc::new(Runtime {
            providers,
            tools,
            bus,
            sessions,
            permissions,
            todos,
            memory: Arc::new(MemoryService::new(store.clone())),
            proposals,
            store,
            astore,
            web: WebConfig {
                fetch_max_chars: cfg.web_fetch_max_chars,
                allow_private: cfg.web_fetch_allow_private,
            },
            search_config,
            skill_dirs,
            default_workspace_root: cfg.workspace_root.clone(),
            tool_output_dir: cfg.data_dir.join("tool-outputs"),
            shell: ShellManager::new(cfg.data_dir.join("shell-outputs")),
            hooks: Arc::new(HookRunner::new(cfg.data_dir.join("hooks.json"))),
            mcp: agent_mcp::McpManager::new(cfg.data_dir.join("mcp.json")),
            subagents: crate::subagents::SubagentRegistry::new(cfg.data_dir.join("agents")),
            parallel_limit: cfg.parallel_limit,
            edit_parallel: cfg.edit_parallel,
            tool_loop_steps: cfg.tool_loop_steps,
            repeated_tool_limit: cfg.repeated_tool_limit,
            subagent_max_depth: cfg.subagent_max_depth,
            empty_progress_limit: cfg.empty_progress_limit,
            todo_retry_limit: cfg.todo_retry_limit,
            todo_timeout_secs: cfg.todo_timeout_secs,
            stream: cfg.stream,
            context_budget_tokens: cfg.context_budget_tokens,
            approval_timeout_secs: cfg.approval_timeout_secs,
            controls: Mutex::new(HashMap::new()),
        })
    }

    pub fn workspace_root(&self) -> PathBuf {
        self.store
            .kv_get("workspace_root")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_workspace_root.clone())
    }

    pub(crate) fn tool_ctx(&self, session_id: &str, run_id: &str) -> ToolContext {
        ToolContext {
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            workspace_root: self.workspace_root(),
            web: self.web.clone(),
            search: self.search_config.clone(),
            skill_dirs: self.skill_dirs.clone(),
            tool_output_dir: self.tool_output_dir.clone(),
            shell: self.shell.clone(),
        }
    }

    pub(crate) fn emit(
        &self,
        session_id: &str,
        etype: &str,
        domain: &str,
        payload: serde_json::Value,
        corr: Option<String>,
    ) {
        self.bus.emit(
            EventDraft::new(session_id, etype, domain)
                .payload(payload)
                .correlation(corr),
        );
    }

    pub(crate) fn emit_actor(
        &self,
        session_id: &str,
        etype: &str,
        domain: &str,
        actor: &str,
        payload: serde_json::Value,
        corr: Option<String>,
    ) {
        self.bus.emit(
            EventDraft::new(session_id, etype, domain)
                .actor(actor)
                .payload(payload)
                .correlation(corr),
        );
    }

    /// The agent profile (general / document / coding) for a session. Unknown
    /// or legacy sessions resolve to [`AgentKind::Coding`].
    pub(crate) fn session_kind(&self, session_id: &str) -> AgentKind {
        self.sessions
            .get(session_id)
            .ok()
            .map(|s| AgentKind::from_str(&s.agent_kind))
            .unwrap_or_default()
    }

    pub(crate) fn session_profile(&self, session_id: &str) -> AgentProfile {
        AgentProfile::new(self.session_kind(session_id))
    }

    /// Gather the dynamic context (branch, AGENT.md, skills, relevant memories)
    /// and build the layered system prompt for a main-loop turn. `memory_query`
    /// (usually the current user input) drives memory retrieval.
    pub(crate) fn build_main_system_prompt(
        &self,
        session_id: &str,
        mode: &str,
        allowed: &[String],
        context_window: Option<&serde_json::Value>,
        memory_query: Option<&str>,
    ) -> String {
        let root = self.workspace_root();
        let profile = self.session_profile(session_id);
        let tools_enabled = !allowed.is_empty() && mode != "ask";
        let memories = match memory_query {
            Some(q) if !q.trim().is_empty() => self.retrieve_memory_lines(session_id, q),
            _ => vec![],
        };
        let ctx = crate::prompts::PromptContext {
            kind: profile.kind,
            mode,
            tools: allowed,
            workspace_root: Some(root.as_path()),
            branch: crate::workspace::read_branch(&root),
            agent_md: read_agent_md(&root),
            rules: read_rules(&root),
            context_window,
            skills: if tools_enabled {
                discover_skill_items(&self.skill_dirs)
            } else {
                vec![]
            },
            memories,
            todo_tools: tools_enabled && profile.wants_todo_tools(),
            task_tool: tools_enabled && profile.wants_task_tool(),
            memory_tools: tools_enabled && profile.wants_memory_tools(),
            subagents_prompt: if tools_enabled && profile.wants_task_tool() {
                Some(self.subagents.render_prompt())
            } else {
                None
            },
        };
        crate::prompts::build_system_prompt(&ctx)
    }

    pub(crate) fn allowed_tools(&self, session_id: &str, composer_mode: &str) -> Vec<String> {
        if composer_mode == "ask" {
            return vec![];
        }
        let session = self.sessions.get(session_id).ok();
        let web_on = session
            .as_ref()
            .map(|s| s.web_search_enabled)
            .unwrap_or(false);
        let profile = AgentProfile::new(
            session
                .as_ref()
                .map(|s| AgentKind::from_str(&s.agent_kind))
                .unwrap_or_default(),
        );
        self.tools
            .names()
            .into_iter()
            .filter(|n| profile.registry_tool_allowed(n))
            .filter(|n| web_on || (n != "web_search" && n != "web_fetch"))
            .filter(|n| self.permissions.can_use_tool(session_id, n))
            // Plan mode is read-only research + todo planning: side-effect
            // tools stay out of the loop entirely.
            .filter(|n| {
                composer_mode != "plan" || !crate::permission::WRITE_TOOLS.contains(&n.as_str())
            })
            .collect()
    }
}

/// Read project memory: `AGENT.md` at the workspace root (`AGENTS.md` as a
/// fallback). Injected into the system prompt's dynamic block.
pub(crate) fn read_agent_md(root: &std::path::Path) -> Option<String> {
    for name in ["AGENT.md", "AGENTS.md"] {
        if let Ok(content) = std::fs::read_to_string(root.join(name)) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Per-rule / total caps for project rule injection.
const RULE_FILE_MAX_CHARS: usize = 2_000;
const RULE_MAX_FILES: usize = 8;

/// Read project rules from `.cursor/rules/*.md(c)` and `.agent/rules/*.md`,
/// returned as `(file_stem, clipped_content)` pairs sorted by name.
pub(crate) fn read_rules(root: &std::path::Path) -> Vec<(String, String)> {
    let mut rules: Vec<(String, String)> = Vec::new();
    for dir in [root.join(".cursor/rules"), root.join(".agent/rules")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "md" | "mdc") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let trimmed = content.trim();
            if trimmed.is_empty() {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("rule")
                .to_string();
            rules.push((name, trimmed.chars().take(RULE_FILE_MAX_CHARS).collect()));
        }
    }
    rules.sort_by(|a, b| a.0.cmp(&b.0));
    rules.truncate(RULE_MAX_FILES);
    rules
}

/// Discovered skills as `(name, summary)` pairs for the prompt builder.
pub(crate) fn discover_skill_items(skill_dirs: &[PathBuf]) -> Vec<(String, String)> {
    agent_tools::skill::discover_skills(skill_dirs)
        .into_iter()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_string();
            let desc = s
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((name, desc))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_md_lookup_handles_missing_file() {
        let dir = std::env::temp_dir().join(format!("agentd_md_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_agent_md(&dir).is_none());
        std::fs::write(dir.join("AGENT.md"), "# 项目约定\n用中文写注释。").unwrap();
        assert!(read_agent_md(&dir).unwrap().contains("项目约定"));
    }

    #[test]
    fn rules_lookup_reads_cursor_and_agent_dirs() {
        let dir =
            std::env::temp_dir().join(format!("agentd_rules_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join(".cursor/rules")).unwrap();
        std::fs::create_dir_all(dir.join(".agent/rules")).unwrap();
        assert!(read_rules(&dir).is_empty());
        std::fs::write(dir.join(".cursor/rules/style.mdc"), "缩进用 4 空格。").unwrap();
        std::fs::write(dir.join(".agent/rules/commit.md"), "提交信息用英文。").unwrap();
        std::fs::write(dir.join(".cursor/rules/ignored.txt"), "非 markdown 忽略").unwrap();
        let rules = read_rules(&dir);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].0, "commit");
        assert_eq!(rules[1].0, "style");
        assert!(rules[1].1.contains("4 空格"));
    }
}
