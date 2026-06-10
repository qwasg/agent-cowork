//! Agent tool abstraction + registry. Tools expose an OpenAI function-calling
//! JSON schema and run against a `ToolContext` (workspace root, session, web).

pub mod command;
pub mod skill;
pub mod web;
pub mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::contracts::{ApiError, ApiResult};
use crate::provider::types::ToolSpec;

#[derive(Clone)]
pub struct WebConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub fetch_max_chars: usize,
    /// Allow `web_fetch` to reach private / internal addresses (SSRF opt-out).
    pub allow_private: bool,
}

pub struct ToolContext {
    pub session_id: String,
    pub run_id: String,
    pub workspace_root: PathBuf,
    pub web: WebConfig,
    pub skill_dirs: Vec<PathBuf>,
}

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String>;
}

pub struct ToolRegistry {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn build(web_enabled: bool) -> Self {
        let mut tools: Vec<Arc<dyn AgentTool>> = vec![
            Arc::new(workspace::ReadFile),
            Arc::new(workspace::ListDir),
            Arc::new(workspace::Grep),
            Arc::new(workspace::WriteFile),
            Arc::new(workspace::CreateDocument),
            Arc::new(workspace::DeleteFile),
            Arc::new(command::RunCommand),
            Arc::new(skill::ReadSkill),
        ];
        if web_enabled {
            tools.push(Arc::new(web::WebSearch));
            tools.push(Arc::new(web::WebFetch));
        }
        ToolRegistry { tools }
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    pub fn specs(&self, allowed: &[String]) -> Vec<ToolSpec> {
        self.tools
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
        self.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters(),
                })
            })
            .collect()
    }

    pub async fn run(&self, name: &str, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ApiError::new("TOOL_NOT_FOUND", format!("unknown tool: {name}")))?;
        tool.run(args, ctx).await
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

    fn tmp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agentd_sandbox_{}",
            uuid::Uuid::new_v4().simple()
        ));
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
