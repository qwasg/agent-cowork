//! Workspace tree / browse / file IO and the local-file escape hatch.

use std::path::PathBuf;

use serde_json::{json, Value};

use agent_core::workspace;
use agent_protocol::{ApiError, ApiResult};

use crate::api::AppServices;

/// Binary document formats that must not be read as UTF-8 text.
fn is_binary_document(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".docx", ".pptx", ".pdf", ".xlsx"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

impl AppServices {
    pub fn get_workspace_info(&self) -> Value {
        workspace::workspace_info(&self.runtime.workspace_root())
    }

    pub fn get_workspace_tree(&self, path: &str, show_heavy: bool) -> ApiResult<Value> {
        workspace::list_tree(&self.runtime.workspace_root(), path, show_heavy)
    }

    /// Resolve a caller-supplied filesystem path. Unless
    /// `AGENT_DEBUG_ALLOW_LOCAL_FS` is set, paths are confined to the
    /// workspace root (closes the arbitrary read/write hole).
    fn local_fs_target(&self, path: &str) -> ApiResult<PathBuf> {
        if self.cfg.allow_local_fs {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid_path(path));
            }
            Ok(PathBuf::from(trimmed))
        } else {
            agent_tools::resolve_in_root(&self.runtime.workspace_root(), path)
        }
    }

    pub fn browse_directories(&self, path: &str) -> ApiResult<Value> {
        // The workspace picker browses arbitrary local paths (Python parity);
        // confined deployments can still gate this behind the edge gateway.
        if !self.cfg.allow_local_fs && !path.trim().is_empty() {
            // Confined mode: only browse inside the workspace root.
            let base = self.local_fs_target(path)?;
            return workspace::browse(&base.to_string_lossy());
        }
        if path.trim().is_empty() && !self.cfg.allow_local_fs {
            return workspace::browse(&self.runtime.workspace_root().to_string_lossy());
        }
        workspace::browse(path)
    }

    pub async fn set_workspace_root(&self, path: &str) -> ApiResult<Value> {
        let p = PathBuf::from(path.trim());
        if !p.is_dir() {
            return Err(ApiError::new("PATH_NOT_DIRECTORY", "not a directory"));
        }
        self.astore
            .kv_put("workspace_root", path.trim())
            .await
            .map_err(|e| ApiError::store(format!("failed to persist workspace root: {e}")))?;
        Ok(json!({ "workspaceRoot": path.trim() }))
    }

    pub fn read_workspace_file(&self, path: &str) -> ApiResult<Value> {
        let abs = agent_tools::resolve_in_root(&self.runtime.workspace_root(), path)?;
        // Binary documents (Word/PPT/PDF) are not UTF-8 text: reading them as a
        // string yields garbage in the IDE file tab. Return a friendly notice
        // and steer callers to the `read_document` agent tool instead.
        if is_binary_document(path) {
            return Ok(json!({
                "path": path,
                "content": "[binary document — 用 read_document 工具读取其文本内容]",
                "binary": true,
            }));
        }
        let content = std::fs::read_to_string(&abs).map_err(|_| ApiError::path_not_found(path))?;
        Ok(json!({ "path": path, "content": content }))
    }

    pub fn write_workspace_file(&self, path: &str, content: &str) -> ApiResult<Value> {
        let abs = agent_tools::resolve_in_root(&self.runtime.workspace_root(), path)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(json!({ "ok": true, "path": path }))
    }

    pub fn read_local_file(&self, path: &str) -> ApiResult<Value> {
        let abs = self.local_fs_target(path)?;
        let content = std::fs::read_to_string(&abs).map_err(|_| ApiError::path_not_found(path))?;
        Ok(json!({ "path": path, "content": content }))
    }

    pub fn write_local_file(&self, path: &str, content: &str) -> ApiResult<Value> {
        let abs = self.local_fs_target(path)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(json!({ "ok": true, "path": path }))
    }

    /// Real git revert: `git checkout HEAD -- <path>`.
    pub fn revert_workspace_file(&self, path: &str) -> ApiResult<Value> {
        if path.trim().is_empty() {
            return Err(ApiError::invalid_path(path));
        }
        workspace::checkout_head(&self.runtime.workspace_root(), path)
    }

    pub fn read_workspace_document(&self, path: &str) -> ApiResult<Value> {
        agent_tools::document::read_workspace_document(&self.runtime.workspace_root(), path)
    }

    pub fn write_workspace_document(&self, path: &str, ir: &serde_json::Value) -> ApiResult<Value> {
        agent_tools::document::write_workspace_document(
            &self.runtime.workspace_root(),
            path,
            ir.clone(),
        )
    }
}
