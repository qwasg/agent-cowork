//! Workspace file tools: read/list/grep/write/create/delete.
//! `grep` uses the ripgrep ecosystem crates (`ignore` walker + `regex`).
//!
//! File IO uses `tokio::fs` (and `spawn_blocking` for the grep walk) so the
//! agent loop never blocks a runtime worker on disk access.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::contracts::{ApiError, ApiResult};
use crate::tools::{resolve_in_root, AgentTool, ToolContext};

fn arg_str(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

pub struct ReadFile;

#[async_trait]
impl AgentTool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a UTF-8 text file from the workspace. Returns its content."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "workspace-relative path"} },
            "required": ["path"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        match tokio::fs::read_to_string(&abs).await {
            Ok(content) => {
                let truncated: String = content.chars().take(40_000).collect();
                Ok(truncated)
            }
            Err(_) => Err(ApiError::path_not_found(&path)),
        }
    }
}

pub struct ListDir;

#[async_trait]
impl AgentTool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List entries of a workspace directory."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "workspace-relative dir (default root)"} }
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        let abs = if path.trim().is_empty() {
            ctx.workspace_root.clone()
        } else {
            resolve_in_root(&ctx.workspace_root, &path)?
        };
        let mut rd = tokio::fs::read_dir(&abs)
            .await
            .map_err(|_| ApiError::path_not_found(&path))?;
        let mut entries = Vec::new();
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(if is_dir { format!("{name}/") } else { name });
        }
        entries.sort();
        Ok(entries.join("\n"))
    }
}

pub struct Grep;

#[async_trait]
impl AgentTool for Grep {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents in the workspace with a regex (ripgrep-style)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "regular expression"},
                "path": {"type": "string", "description": "subdir to limit search (optional)"}
            },
            "required": ["pattern"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let pattern = arg_str(&args, "pattern");
        if pattern.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "pattern required"));
        }
        let re = regex::Regex::new(&pattern)
            .map_err(|e| ApiError::new("TOOL_INVALID_ARGS", format!("bad regex: {e}")))?;
        let sub = arg_str(&args, "path");
        let base = if sub.trim().is_empty() {
            ctx.workspace_root.clone()
        } else {
            resolve_in_root(&ctx.workspace_root, &sub)?
        };
        let root = ctx.workspace_root.clone();

        // The directory walk + file reads are blocking work; run them off the
        // async worker threads.
        let hits = tokio::task::spawn_blocking(move || {
            const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
            let mut hits = Vec::new();
            let walker = ignore::WalkBuilder::new(&base).max_depth(Some(12)).build();
            'outer: for entry in walker.flatten() {
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                if entry
                    .metadata()
                    .map(|m| m.len() > MAX_FILE_BYTES)
                    .unwrap_or(false)
                {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };
                let rel = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .to_string();
                for (i, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        hits.push(format!("{}:{}:{}", rel, i + 1, line.trim()));
                        if hits.len() >= 200 {
                            break 'outer;
                        }
                    }
                }
            }
            hits
        })
        .await
        .map_err(|e| ApiError::new("FILESYSTEM_ERROR", format!("grep task failed: {e}")))?;

        if hits.is_empty() {
            Ok("(no matches)".to_string())
        } else {
            Ok(hits.join("\n"))
        }
    }
}

pub struct WriteFile;

#[async_trait]
impl AgentTool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Create or overwrite a workspace file with the given content."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        let content = arg_str(&args, "content");
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        tokio::fs::write(&abs, content.as_bytes())
            .await
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(format!("wrote {} bytes to {}", content.len(), path))
    }
}

pub struct CreateDocument;

#[async_trait]
impl AgentTool for CreateDocument {
    fn name(&self) -> &str {
        "create_document"
    }
    fn description(&self) -> &str {
        "Create a new document file (fails if it already exists)."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        let content = arg_str(&args, "content");
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        if abs.exists() {
            return Err(ApiError::new(
                "INVALID_PATH",
                format!("already exists: {path}"),
            ));
        }
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        tokio::fs::write(&abs, content.as_bytes())
            .await
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(format!("created {path}"))
    }
}

pub struct DeleteFile;

#[async_trait]
impl AgentTool for DeleteFile {
    fn name(&self) -> &str {
        "delete_file"
    }
    fn description(&self) -> &str {
        "Delete a workspace file."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string"} },
            "required": ["path"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        tokio::fs::remove_file(&abs)
            .await
            .map_err(|_| ApiError::path_not_found(&path))?;
        Ok(format!("deleted {path}"))
    }
}
