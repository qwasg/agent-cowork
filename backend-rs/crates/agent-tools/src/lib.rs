//! Agent tool abstraction + registry. Tools expose an OpenAI function-calling
//! JSON schema and run against a `ToolContext` (workspace root, session, web).

pub mod command;
pub mod document;
pub mod edit;
pub mod search_config;
pub mod shell;
pub mod skill;
pub mod web;
pub mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use agent_protocol::{ApiError, ApiResult};
use agent_providers::types::ToolSpec;

use crate::search_config::SearchConfigService;

#[derive(Clone)]
pub struct WebConfig {
    pub fetch_max_chars: usize,
    /// Allow `web_fetch` to reach private / internal addresses (SSRF opt-out).
    pub allow_private: bool,
}

pub struct ToolContext {
    pub session_id: String,
    pub run_id: String,
    pub workspace_root: PathBuf,
    pub web: WebConfig,
    /// Live search configuration (store-backed, env fallback) resolved per call.
    pub search: Arc<SearchConfigService>,
    pub skill_dirs: Vec<PathBuf>,
    /// Where oversized tool outputs spill to disk (`data/tool-outputs/`).
    pub tool_output_dir: PathBuf,
    /// Shared shell manager (persistent sessions + background jobs).
    pub shell: Arc<shell::ShellManager>,
}

/// Static tool metadata (v2): schema plus execution semantics.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// No side effects → safe to run in parallel within a tool batch.
    pub read_only: bool,
    /// Should interactive permission modes ask before running this tool.
    pub needs_approval: bool,
}

/// Structured tool result with truncation metadata.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// Content fed back to the model (truncated when over limits, with a
    /// pointer to the spill file appended).
    pub content: String,
    pub truncated: bool,
    pub total_bytes: usize,
    /// Full output on disk when truncated.
    pub spill_path: Option<PathBuf>,
}

impl ToolOutcome {
    pub fn complete(content: String) -> Self {
        let total_bytes = content.len();
        ToolOutcome {
            content,
            truncated: false,
            total_bytes,
            spill_path: None,
        }
    }
}

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    /// Read-only tools have no side effects and may run in parallel within a
    /// tool batch. Defaults to `false` (safe: side-effecting tools serialize).
    fn read_only(&self) -> bool {
        false
    }
    /// Whether interactive permission modes should ask before running.
    /// Defaults to "anything with side effects".
    fn needs_approval(&self) -> bool {
        !self.read_only()
    }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
            read_only: self.read_only(),
            needs_approval: self.needs_approval(),
        }
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String>;
}

/// Output truncation policy applied by the registry to every tool result.
#[derive(Debug, Clone, Copy)]
pub struct OutputLimits {
    pub max_chars: usize,
    pub max_lines: usize,
}

impl Default for OutputLimits {
    fn default() -> Self {
        OutputLimits {
            max_chars: 20_000,
            max_lines: 400,
        }
    }
}

/// Runtime-mutable tool registry: builtin tools at construction, dynamic
/// registration afterwards (MCP servers, per-session capabilities).
pub struct ToolRegistry {
    tools: std::sync::RwLock<Vec<Arc<dyn AgentTool>>>,
    limits: OutputLimits,
}

impl ToolRegistry {
    pub fn build(web_enabled: bool) -> Self {
        Self::build_with_limits(web_enabled, OutputLimits::default())
    }

    pub fn build_with_limits(web_enabled: bool, limits: OutputLimits) -> Self {
        let mut tools: Vec<Arc<dyn AgentTool>> = vec![
            Arc::new(workspace::ReadFile),
            Arc::new(workspace::ListDir),
            Arc::new(workspace::Grep),
            Arc::new(workspace::WriteFile),
            Arc::new(workspace::CreateDocument),
            Arc::new(workspace::DeleteFile),
            Arc::new(command::RunCommand),
            Arc::new(command::ShellOutput),
            Arc::new(command::StopCommand),
            Arc::new(skill::ReadSkill),
            Arc::new(document::ReadDocument),
            Arc::new(document::CreateWordDocument),
            Arc::new(document::CreatePresentation),
            Arc::new(document::CreatePdf),
            Arc::new(document::EditWordDocument),
            Arc::new(document::EditPresentation),
        ];
        if web_enabled {
            tools.push(Arc::new(web::WebSearch));
            tools.push(Arc::new(web::WebFetch));
        }
        ToolRegistry {
            tools: std::sync::RwLock::new(tools),
            limits,
        }
    }

    /// Register (or replace, by name) a tool at runtime.
    pub fn register(&self, tool: Arc<dyn AgentTool>) {
        let mut guard = self.tools.write().unwrap();
        let name = tool.name().to_string();
        guard.retain(|t| t.name() != name);
        guard.push(tool);
    }

    /// Remove a dynamically registered tool. Returns whether it existed.
    pub fn unregister(&self, name: &str) -> bool {
        let mut guard = self.tools.write().unwrap();
        let before = guard.len();
        guard.retain(|t| t.name() != name);
        guard.len() != before
    }

    /// Remove every tool whose name starts with `prefix` (MCP server reload).
    pub fn unregister_prefix(&self, prefix: &str) {
        self.tools
            .write()
            .unwrap()
            .retain(|t| !t.name().starts_with(prefix));
    }

    fn find(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .find(|t| t.name() == name)
            .cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .map(|t| t.descriptor())
            .collect()
    }

    /// Whether `name` is a registered read-only (parallel-safe) tool.
    pub fn is_read_only(&self, name: &str) -> bool {
        self.find(name).map(|t| t.read_only()).unwrap_or(false)
    }

    pub fn specs(&self, allowed: &[String]) -> Vec<ToolSpec> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .filter(|t| allowed.is_empty() || allowed.iter().any(|a| a == t.name()))
            .map(|t| ToolSpec {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect()
    }

    pub fn describe(&self) -> Vec<Value> {
        self.descriptors()
            .into_iter()
            .map(|d| {
                serde_json::json!({
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                    "readOnly": d.read_only,
                    "needsApproval": d.needs_approval,
                })
            })
            .collect()
    }

    /// Run a tool and apply the output truncation policy: oversized results
    /// are spilled to `ctx.tool_output_dir` and replaced by a head excerpt
    /// plus a pointer for follow-up reads.
    pub async fn run(&self, name: &str, args: Value, ctx: &ToolContext) -> ApiResult<ToolOutcome> {
        let tool = self
            .find(name)
            .ok_or_else(|| ApiError::new("TOOL_NOT_FOUND", format!("unknown tool: {name}")))?;
        let raw = tool.run(args, ctx).await?;
        Ok(truncate_output(raw, name, ctx, self.limits))
    }
}

/// Apply line/byte caps; when exceeded, persist the full output under
/// `tool_output_dir` and return a head excerpt with continuation metadata.
fn truncate_output(
    raw: String,
    tool: &str,
    ctx: &ToolContext,
    limits: OutputLimits,
) -> ToolOutcome {
    let total_bytes = raw.len();
    let over_chars = raw.chars().count() > limits.max_chars;
    let over_lines = raw.lines().count() > limits.max_lines;
    if !over_chars && !over_lines {
        return ToolOutcome::complete(raw);
    }

    // Head excerpt honoring both caps.
    let mut head: String = raw
        .lines()
        .take(limits.max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    if head.chars().count() > limits.max_chars {
        head = head.chars().take(limits.max_chars).collect();
    }

    let spill_path = {
        let dir = ctx.tool_output_dir.join(&ctx.run_id);
        let file = dir.join(format!("{tool}-{}.txt", uuid::Uuid::new_v4().simple()));
        let write =
            std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, raw.as_bytes()));
        match write {
            Ok(()) => Some(file),
            Err(e) => {
                tracing::warn!("tool-output spill for {tool} failed: {e}");
                None
            }
        }
    };

    let footer = match &spill_path {
        Some(p) => format!(
            "\n\n[输出已截断：完整输出共 {total_bytes} 字节，已写入 {}；可用 read_file 按需续读]",
            p.display()
        ),
        None => {
            format!("\n\n[输出已截断：完整输出共 {total_bytes} 字节，落盘失败，仅保留开头部分]")
        }
    };
    head.push_str(&footer);
    ToolOutcome {
        content: head,
        truncated: true,
        total_bytes,
        spill_path,
    }
}

/// Resolve `rel` under `root`, rejecting traversal outside the workspace root.
///
/// Hardened against:
/// - `..` traversal (lexical normalization)
/// - symlink escape (the deepest *existing* ancestor is canonicalized and the
///   real path is checked against the canonical root)
/// - Windows case / `\\?\`-prefix mismatches (`dunce` canonicalization +
///   case-insensitive component comparison on Windows)
pub fn resolve_in_root(root: &PathBuf, rel: &str) -> ApiResult<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(ApiError::invalid_path(rel));
    }
    let candidate = PathBuf::from(rel);
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    };
    let normalized = normalize(&joined);
    let root_real = dunce::canonicalize(root).unwrap_or_else(|_| normalize(root));
    let real = canonicalize_existing_prefix(&normalized);
    if !path_starts_with(&real, &root_real) {
        return Err(ApiError::path_outside_root(rel));
    }
    Ok(normalized)
}

fn normalize(p: &std::path::Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            ParentDir => {
                out.pop();
            }
            CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the deepest existing ancestor of `p` (resolving symlinks),
/// then re-append the non-existing tail. Lets us validate paths that are
/// about to be created.
fn canonicalize_existing_prefix(p: &std::path::Path) -> PathBuf {
    let mut existing = p.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if existing.exists() {
            if let Ok(canon) = dunce::canonicalize(&existing) {
                let mut out = canon;
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return out;
            }
        }
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                if !existing.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    p.to_path_buf()
}

fn path_starts_with(p: &std::path::Path, root: &std::path::Path) -> bool {
    let pc: Vec<_> = p.components().collect();
    let rc: Vec<_> = root.components().collect();
    if rc.len() > pc.len() {
        return false;
    }
    pc.iter().zip(rc.iter()).all(|(a, b)| component_eq(a, b))
}

#[cfg(windows)]
fn component_eq(a: &std::path::Component, b: &std::path::Component) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

#[cfg(not(windows))]
fn component_eq(a: &std::path::Component, b: &std::path::Component) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_ctx(root: PathBuf) -> ToolContext {
        let store = Arc::new(agent_store::Store::open(root.join("t.redb")).unwrap());
        let crypto = agent_store::CryptoStore::open(root.join("k.key"));
        let cfg = agent_config::Config::load();
        ToolContext {
            session_id: "s".into(),
            run_id: "r".into(),
            workspace_root: root.clone(),
            web: WebConfig {
                fetch_max_chars: 1000,
                allow_private: false,
            },
            search: SearchConfigService::new(store, crypto, &cfg),
            skill_dirs: vec![],
            tool_output_dir: root.join("tool-outputs"),
            shell: shell::ShellManager::new(root.join("shell-outputs")),
        }
    }

    #[test]
    fn small_output_passes_through() {
        let ctx = tmp_ctx(tmp_root());
        let out = truncate_output("short".into(), "t", &ctx, OutputLimits::default());
        assert!(!out.truncated);
        assert_eq!(out.content, "short");
        assert!(out.spill_path.is_none());
    }

    #[test]
    fn oversized_output_spills_to_disk() {
        let ctx = tmp_ctx(tmp_root());
        let big: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let limits = OutputLimits {
            max_chars: 500,
            max_lines: 50,
        };
        let out = truncate_output(big.clone(), "grep", &ctx, limits);
        assert!(out.truncated);
        assert_eq!(out.total_bytes, big.len());
        assert!(out.content.contains("输出已截断"));
        let spill = out.spill_path.expect("spill file");
        assert_eq!(std::fs::read_to_string(spill).unwrap(), big);
        // Head respects the line cap.
        let head_lines = out.content.lines().count();
        assert!(head_lines <= 50 + 3, "head too long: {head_lines}");
    }

    #[test]
    fn registry_dynamic_register_unregister() {
        struct Echo;
        #[async_trait]
        impl AgentTool for Echo {
            fn name(&self) -> &str {
                "echo_test"
            }
            fn description(&self) -> &str {
                "echo"
            }
            fn parameters(&self) -> Value {
                serde_json::json!({ "type": "object", "properties": {} })
            }
            fn read_only(&self) -> bool {
                true
            }
            async fn run(&self, _args: Value, _ctx: &ToolContext) -> ApiResult<String> {
                Ok("echo".into())
            }
        }
        let reg = ToolRegistry::build(false);
        assert!(!reg.names().contains(&"echo_test".to_string()));
        reg.register(Arc::new(Echo));
        assert!(reg.names().contains(&"echo_test".to_string()));
        assert!(reg.is_read_only("echo_test"));
        assert!(reg
            .descriptors()
            .iter()
            .any(|d| d.name == "echo_test" && !d.needs_approval));
        assert!(reg.unregister("echo_test"));
        assert!(!reg.names().contains(&"echo_test".to_string()));
    }

    fn tmp_root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agentd_sandbox_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        root
    }

    #[test]
    fn allows_paths_inside_root() {
        let root = tmp_root();
        assert!(resolve_in_root(&root, "sub/file.txt").is_ok());
        assert!(resolve_in_root(&root, "new-dir/new-file.txt").is_ok());
    }

    #[test]
    fn rejects_parent_traversal() {
        let root = tmp_root();
        assert!(resolve_in_root(&root, "../outside.txt").is_err());
        assert!(resolve_in_root(&root, "sub/../../outside.txt").is_err());
    }

    #[test]
    fn rejects_absolute_outside_root() {
        let root = tmp_root();
        let outside = std::env::temp_dir().join("definitely-outside.txt");
        assert!(resolve_in_root(&root, &outside.to_string_lossy()).is_err());
    }

    #[test]
    fn rejects_empty_path() {
        let root = tmp_root();
        assert!(resolve_in_root(&root, "  ").is_err());
    }
}
