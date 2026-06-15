//! Runtime-handled structured edit tools: `str_replace_edit` (exact old/new
//! replacement) and `apply_patch` (Codex multi-hunk patch). Both apply to the
//! workspace immediately and record an applied proposal so the existing
//! proposal UI / replay sees every model edit.

use serde_json::{json, Value};

use agent_protocol::models::{new_id, now_ts};
use agent_protocol::{ApiError, ApiResult};
use agent_providers::types::ToolSpec;
use agent_tools::edit::{apply_hunks, parse_patch, str_replace, FileOp};
use agent_tools::resolve_in_root;

use super::Runtime;
use crate::code_edit::SCHEMA_VERSION;

/// One applied file change: `(path, original, proposed, description)`.
type AppliedChange = (String, String, String, String);

impl Runtime {
    pub async fn handle_edit_tool(
        &self,
        session_id: &str,
        run_id: &str,
        name: &str,
        args: &Value,
    ) -> ApiResult<String> {
        let root = self.workspace_root();
        let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");

        let (summary, changes): (String, Vec<AppliedChange>) = match name {
            "str_replace_edit" => {
                let path = s("path").trim().to_string();
                if path.is_empty() {
                    return Err(ApiError::new("TOOL_INVALID_ARGS", "path required"));
                }
                let old = s("old_string");
                let new = s("new_string");
                let replace_all = args
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let abs = resolve_in_root(&root, &path)?;
                let original = tokio::fs::read_to_string(&abs)
                    .await
                    .map_err(|_| ApiError::path_not_found(&path))?;
                let proposed = str_replace(&original, old, new, replace_all)
                    .map_err(|m| ApiError::new("EDIT_CONFLICT", m))?;
                tokio::fs::write(&abs, proposed.as_bytes())
                    .await
                    .map_err(|e| ApiError::filesystem(e.to_string()))?;
                (
                    format!("str_replace_edit: 修改 {path}"),
                    vec![(path, original, proposed, "精确字符串替换".to_string())],
                )
            }
            "apply_patch" => {
                let patch = args
                    .get("patch")
                    .or_else(|| args.get("input"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let ops = parse_patch(patch).map_err(|m| ApiError::new("PATCH_INVALID", m))?;
                let mut changes: Vec<AppliedChange> = Vec::with_capacity(ops.len());
                for op in &ops {
                    let path = op.path().to_string();
                    let abs = resolve_in_root(&root, &path)?;
                    match op {
                        FileOp::Add { content, .. } => {
                            if abs.exists() {
                                return Err(ApiError::new(
                                    "PATCH_INVALID",
                                    format!("Add File 目标已存在: {path}"),
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
                            changes.push((path, String::new(), content.clone(), "新增文件".into()));
                        }
                        FileOp::Delete { .. } => {
                            let original = tokio::fs::read_to_string(&abs)
                                .await
                                .map_err(|_| ApiError::path_not_found(&path))?;
                            tokio::fs::remove_file(&abs)
                                .await
                                .map_err(|e| ApiError::filesystem(e.to_string()))?;
                            changes.push((path, original, String::new(), "删除文件".into()));
                        }
                        FileOp::Update { hunks, .. } => {
                            let original = tokio::fs::read_to_string(&abs)
                                .await
                                .map_err(|_| ApiError::path_not_found(&path))?;
                            let proposed = apply_hunks(&original, hunks)
                                .map_err(|m| ApiError::new("EDIT_CONFLICT", m))?;
                            tokio::fs::write(&abs, proposed.as_bytes())
                                .await
                                .map_err(|e| ApiError::filesystem(e.to_string()))?;
                            changes.push((path, original, proposed, "patch 更新".into()));
                        }
                    }
                }
                let files: Vec<&str> = changes.iter().map(|c| c.0.as_str()).collect();
                (format!("apply_patch: {}", files.join(", ")), changes)
            }
            other => {
                return Err(ApiError::new(
                    "TOOL_NOT_FOUND",
                    format!("unknown edit tool: {other}"),
                ))
            }
        };

        let files = self.record_applied_edit(session_id, run_id, &summary, &changes);
        Ok(format!("已完成编辑（{}）：{}", name, files.join(", ")))
    }

    /// Record the edit through the proposal registry (status `applied`) and
    /// emit the proposal event so the frontend diff view picks it up.
    fn record_applied_edit(
        &self,
        session_id: &str,
        run_id: &str,
        summary: &str,
        changes: &[AppliedChange],
    ) -> Vec<String> {
        let files: Vec<String> = changes.iter().map(|c| c.0.clone()).collect();
        let payload = json!({
            "schemaVersion": SCHEMA_VERSION,
            "proposalId": new_id("edit"),
            "sessionId": session_id,
            "source": "agent-edit-tool",
            "summary": summary,
            "createdAt": now_ts(),
            "changes": changes.iter().map(|(path, original, proposed, desc)| json!({
                "changeId": new_id("change"),
                "path": path,
                "language": "plaintext",
                "description": desc,
                "strategy": "replace-file",
                "originalContent": original,
                "proposedContent": proposed,
                "selection": Value::Null,
            })).collect::<Vec<_>>(),
        });
        let rec = self.proposals.record(&payload);
        self.proposals.mark_applied(&rec.id, files.clone());
        self.emit(
            session_id,
            "agent.code_edit.proposed",
            "agent",
            json!({
                "proposalId": rec.id,
                "sessionId": session_id,
                "summary": summary,
                "status": "applied",
                "files": files,
                "runId": run_id,
            }),
            Some(run_id.to_string()),
        );
        files
    }
}

pub(crate) fn str_replace_edit_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "str_replace_edit".to_string(),
        description:
            "对一个已有文件做精确字符串替换（首选的编辑方式）。old_string 必须与原文逐字符一致\
（含缩进/空白）且在文件中唯一；不唯一时请扩大上下文或设置 replace_all=true。\
相比 write_file 全量覆盖，这种方式不会意外丢失文件其余内容。"
                .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的文件路径"},
                "old_string": {"type": "string", "description": "要被替换的原文片段（需唯一匹配）"},
                "new_string": {"type": "string", "description": "替换后的新内容"},
                "replace_all": {"type": "boolean", "description": "替换所有匹配（默认 false）"}
            },
            "required": ["path", "old_string", "new_string"]
        }),
    }
}

pub(crate) fn apply_patch_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "apply_patch".to_string(),
        description: "用结构化 patch 一次性新增/更新/删除多个文件。格式：\n\
*** Begin Patch\n\
*** Add File: 相对路径（每行内容以 + 开头）\n\
*** Update File: 相对路径（@@ 开新 hunk；上下文行以空格开头、删除行以 - 开头、新增行以 + 开头）\n\
*** Delete File: 相对路径\n\
*** End Patch\n\
Update hunk 的上下文必须与原文唯一匹配。"
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "patch": {"type": "string", "description": "完整 patch 文本（含 Begin/End Patch 包裹）"}
            },
            "required": ["patch"]
        }),
    }
}
