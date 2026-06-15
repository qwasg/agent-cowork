//! Per-session tool permission mode (port of `permission_service.py`) plus a
//! Claude-Code-style rule layer.
//!
//! Modes (matching the Python semantics):
//! - `bypass` (default): every tool runs without confirmation
//! - `plan`: write tools are blocked entirely (read-only)
//! - `auto`: write tools require interactive approval — the run emits a
//!   `permission.requested` event and suspends on a oneshot channel until
//!   `POST /permissions/{id}:approve|deny` resolves it (or it times out).
//!
//! Rules refine the mode defaults. Each rule is `{action, pattern}` where
//! `action ∈ {allow, deny, ask}` and `pattern` is `tool` or `tool(arg-glob)`,
//! e.g. `run_command(git *)` or `delete_file(*)`. The arg glob matches the
//! tool's *argument summary* (command line for `run_command`, path for file
//! tools). First matching rule wins; `deny` blocks outright, `allow` skips
//! interactive approval even in `auto` mode, `ask` forces approval even in
//! `bypass` mode. `plan` mode's read-only invariant cannot be overridden.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use agent_protocol::models::new_id;
use agent_protocol::{ApiError, ApiResult};
use agent_store::Store;

/// Tools with side effects (file writes / command execution). Public so the
/// runtime can gate `explore`-kind todos down to a read-only tool set.
pub const WRITE_TOOLS: &[&str] = &[
    "write_file",
    "create_document",
    "delete_file",
    "str_replace_edit",
    "apply_patch",
    "run_command",
    "stop_command",
    "create_word_document",
    "create_presentation",
    "create_pdf",
    "edit_word_document",
    "edit_presentation",
];

/// One permission rule: `action` applied when `pattern` matches a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// `allow` | `deny` | `ask`
    pub action: String,
    /// `tool_name` or `tool_name(arg glob)`; both parts support `*`.
    pub pattern: String,
}

/// Outcome of evaluating the rule list against a tool call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuleDecision {
    Allow,
    Deny,
    Ask,
    /// No rule matched — fall back to the session mode defaults.
    Default,
}

const RULES_KEY: &str = "perm:rules:v1";

pub struct PermissionService {
    store: Arc<Store>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl PermissionService {
    pub fn new(store: Arc<Store>) -> Self {
        PermissionService {
            store,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn mode(&self, session_id: &str) -> String {
        self.store
            .kv_get(&key(session_id))
            .unwrap_or_else(|| "bypass".to_string())
    }

    pub fn set_mode(&self, session_id: &str, mode: &str) -> ApiResult<String> {
        if !matches!(mode, "auto" | "plan" | "bypass") {
            return Err(ApiError::new(
                "AUTH_INVALID_INPUT",
                "invalid permission mode",
            ));
        }
        self.store
            .kv_put(&key(session_id), mode)
            .map_err(|e| ApiError::store(format!("failed to persist permission mode: {e}")))?;
        Ok(mode.to_string())
    }

    pub fn can_use_tool(&self, session_id: &str, tool: &str) -> bool {
        match self.mode(session_id).as_str() {
            "plan" => !WRITE_TOOLS.contains(&tool),
            _ => true,
        }
    }

    /// Whether the tool needs interactive user approval in the current mode.
    pub fn requires_approval(&self, session_id: &str, tool: &str) -> bool {
        self.mode(session_id) == "auto" && WRITE_TOOLS.contains(&tool)
    }

    // ---- rule layer ----

    /// The persisted global rule list (empty when unset).
    pub fn rules(&self) -> Vec<PermissionRule> {
        self.store
            .kv_get(RULES_KEY)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Replace the rule list. Validates actions and pattern syntax.
    pub fn set_rules(&self, rules: Vec<PermissionRule>) -> ApiResult<Vec<PermissionRule>> {
        for r in &rules {
            if !matches!(r.action.as_str(), "allow" | "deny" | "ask") {
                return Err(ApiError::new(
                    "AUTH_INVALID_INPUT",
                    format!("invalid rule action: {}", r.action),
                ));
            }
            if parse_pattern(&r.pattern).is_none() {
                return Err(ApiError::new(
                    "AUTH_INVALID_INPUT",
                    format!("invalid rule pattern: {}", r.pattern),
                ));
            }
        }
        let raw = serde_json::to_string(&rules)
            .map_err(|e| ApiError::store(format!("failed to serialize rules: {e}")))?;
        self.store
            .kv_put(RULES_KEY, &raw)
            .map_err(|e| ApiError::store(format!("failed to persist rules: {e}")))?;
        Ok(rules)
    }

    /// Evaluate the rule list against a tool call; first match wins.
    pub fn evaluate(&self, tool: &str, args: &serde_json::Value) -> RuleDecision {
        let summary = arg_summary(tool, args);
        for rule in self.rules() {
            let Some((name_glob, arg_glob)) = parse_pattern(&rule.pattern) else {
                continue;
            };
            if !glob_match(&name_glob, tool) {
                continue;
            }
            if let Some(arg_glob) = arg_glob {
                if !glob_match(&arg_glob, &summary) {
                    continue;
                }
            }
            return match rule.action.as_str() {
                "allow" => RuleDecision::Allow,
                "deny" => RuleDecision::Deny,
                "ask" => RuleDecision::Ask,
                _ => RuleDecision::Default,
            };
        }
        RuleDecision::Default
    }

    // ---- interactive approval requests ----

    /// Register a pending approval; the caller awaits the receiver.
    pub fn create_request(&self) -> (String, oneshot::Receiver<bool>) {
        let id = new_id("perm");
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        (id, rx)
    }

    /// Resolve a pending request. Returns false if it doesn't exist (already
    /// resolved, timed out, or unknown).
    pub fn resolve(&self, request_id: &str, approved: bool) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some(tx) => tx.send(approved).is_ok(),
            None => false,
        }
    }

    /// Drop a request that timed out or was cancelled.
    pub fn discard_request(&self, request_id: &str) {
        self.pending.lock().unwrap().remove(request_id);
    }
}

fn key(session_id: &str) -> String {
    format!("perm:{session_id}")
}

/// The argument string a rule's arg-glob matches against.
pub fn arg_summary(tool: &str, args: &serde_json::Value) -> String {
    let s = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match tool {
        "run_command" => s("command"),
        "stop_command" | "shell_output" => s("shellId"),
        "task" => s("subagent_type"),
        _ => {
            let p = s("path");
            if p.is_empty() {
                s("query")
            } else {
                p
            }
        }
    }
}

/// Split `tool(arg glob)` into `(tool_glob, Some(arg_glob))`; bare `tool`
/// gives `(tool_glob, None)`. Returns `None` on malformed patterns.
fn parse_pattern(pattern: &str) -> Option<(String, Option<String>)> {
    let p = pattern.trim();
    if p.is_empty() {
        return None;
    }
    match p.find('(') {
        None => Some((p.to_string(), None)),
        Some(open) => {
            if !p.ends_with(')') || open == 0 {
                return None;
            }
            let name = p[..open].trim().to_string();
            let arg = p[open + 1..p.len() - 1].trim().to_string();
            Some((name, Some(arg)))
        }
    }
}

/// Minimal `*` wildcard matcher (no `?`, no character classes). Public so
/// the hook runner can reuse it for tool-name matchers.
pub fn glob_match(glob: &str, text: &str) -> bool {
    let parts: Vec<&str> = glob.split('*').collect();
    if parts.len() == 1 {
        return glob == text;
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            return text.len() >= pos && text[pos..].ends_with(part);
        } else {
            match text[pos..].find(part) {
                Some(found) => pos += found + part.len(),
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matching() {
        assert!(glob_match("git *", "git status"));
        assert!(!glob_match("git *", "rm -rf /"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.rs", "src/main.rs"));
        assert!(glob_match("run_command", "run_command"));
        assert!(!glob_match("run_command", "run_commands"));
        assert!(glob_match("mcp__*__list", "mcp__demo__list"));
    }

    #[test]
    fn pattern_parsing() {
        assert_eq!(
            parse_pattern("run_command(git *)"),
            Some(("run_command".into(), Some("git *".into())))
        );
        assert_eq!(
            parse_pattern("delete_file"),
            Some(("delete_file".into(), None))
        );
        assert_eq!(parse_pattern("(x)"), None);
        assert_eq!(parse_pattern("foo(x"), None);
        assert_eq!(parse_pattern(""), None);
    }

    #[test]
    fn arg_summaries() {
        assert_eq!(
            arg_summary("run_command", &serde_json::json!({"command": "git push"})),
            "git push"
        );
        assert_eq!(
            arg_summary("write_file", &serde_json::json!({"path": "a/b.rs"})),
            "a/b.rs"
        );
        assert_eq!(
            arg_summary("task", &serde_json::json!({"subagent_type": "explore"})),
            "explore"
        );
    }
}
