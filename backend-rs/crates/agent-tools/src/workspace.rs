//! Workspace file tools: read/list/grep/write/create/delete.
//! `grep` uses the ripgrep ecosystem crates (`ignore` walker + `regex`).
//!
//! File IO uses `tokio::fs` (and `spawn_blocking` for the grep walk) so the
//! agent loop never blocks a runtime worker on disk access.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{resolve_in_root, AgentTool, ToolContext};
use agent_protocol::{ApiError, ApiResult};

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
    fn read_only(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "读取工作区内一个 UTF-8 文本文件的完整内容。修改任何文件之前必须先用它读过该文件。\
         超长文件只返回前约 4 万字符（超出部分被截断）。读取文件一律用本工具，不要用 run_command。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "相对工作区根目录的文件路径"} },
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
    fn read_only(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "列出工作区某个目录的直接子项（每行一个名字，目录以 / 结尾），用于把握项目结构。\
         只列一层，不递归；要按内容找文件请用 grep。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "相对工作区根目录的目录路径，留空表示根目录"} }
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
    fn read_only(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "在工作区文件内容中按正则表达式搜索（逐行匹配，遵循 .gitignore），是定位代码的首选工具：\
         按符号名、报错文本、配置键等检索。返回每行命中，格式为 `路径:行号:行内容`，最多 200 条；\
         无命中返回 (no matches)。提示：结果过多就缩小 path 或写更精确的 pattern。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "正则表达式（Rust regex 语法，区分大小写）"},
                "path": {"type": "string", "description": "限定搜索的子目录（可选，默认整个工作区）"}
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
        "创建或整体覆盖一个工作区文件（父目录会自动创建）。注意：内容是全量替换——\
         修改已有文件时必须先 read_file 拿到原文，在原文基础上改出完整新内容再写入，\
         否则未包含的部分会丢失。大段中日韩文本（约超过 1 万字符）请拆成多次写入。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的文件路径"},
                "content": {"type": "string", "description": "文件的完整新内容（全量替换）"}
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
        "新建一个文件（目标已存在时报错，绝不覆盖）。确定是“新增文件”时优先用它而不是 write_file，\
         以免误覆盖既有内容。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的新文件路径"},
                "content": {"type": "string", "description": "文件初始内容（可省略，默认空文件）"}
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
        "删除工作区内的一个文件（不可恢复，不支持删除目录）。仅在任务明确需要时使用，\
         删除前确认没有其它代码引用该文件。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "相对工作区根目录的文件路径"} },
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
