//! Long-term memory management REST surface (`/memories`).
//!
//! Backs the settings "memory" panel: list / create / edit / delete the
//! durable structured memories the agent consults and writes.

use serde_json::{json, Value};

use agent_protocol::models::now_ts;
use agent_protocol::{ApiError, ApiResult};

use crate::api::AppServices;

impl AppServices {
    /// Resolve a scope keyword (`global` / `workspace` / `session`) to the
    /// concrete scope string. Anything else is treated as a literal scope.
    fn resolve_memory_scope(&self, keyword: &str, session_id: Option<&str>) -> String {
        match keyword.trim() {
            "global" => "global".to_string(),
            "workspace" => format!("workspace:{}", self.runtime.workspace_root().display()),
            "session" => format!("session:{}", session_id.unwrap_or("")),
            other => other.to_string(),
        }
    }

    /// `GET /memories?scope=&sessionId=`. Without a scope, returns every
    /// memory; with a keyword scope, returns just that scope.
    pub fn list_memories(&self, scope: Option<&str>, session_id: Option<&str>) -> Value {
        let memories = match scope.filter(|s| !s.trim().is_empty()) {
            None => self.runtime.memory.list_all(),
            Some(kw) => {
                let resolved = self.resolve_memory_scope(kw, session_id);
                self.runtime.memory.list_by_scope(&resolved)
            }
        };
        json!({ "memories": memories })
    }

    /// `POST /memories` — manually add a memory.
    pub fn create_memory(&self, payload: &Value) -> ApiResult<Value> {
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(ApiError::new("MEMORY_INVALID", "content is required"));
        }
        let kind = match payload
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("fact")
        {
            "preference" => "preference",
            "convention" => "convention",
            _ => "fact",
        };
        let session_id = payload.get("sessionId").and_then(|v| v.as_str());
        let scope_kw = payload
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("global");
        let scope = self.resolve_memory_scope(scope_kw, session_id);
        let tags: Vec<String> = payload
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let entry = self.runtime.memory.upsert(
            &scope,
            kind,
            content,
            tags,
            session_id.map(String::from),
        )?;
        Ok(json!({ "memory": entry }))
    }

    /// `PATCH /memories/{id}` — edit content / kind / tags.
    pub fn patch_memory(&self, id: &str, patch: &Value) -> ApiResult<Value> {
        let mut entry = self.runtime.memory.get(id)?;
        if let Some(content) = patch.get("content").and_then(|v| v.as_str()) {
            if content.trim().is_empty() {
                return Err(ApiError::new("MEMORY_INVALID", "content cannot be empty"));
            }
            entry.content = content.trim().to_string();
        }
        if let Some(kind) = patch.get("kind").and_then(|v| v.as_str()) {
            entry.kind = match kind {
                "preference" => "preference",
                "convention" => "convention",
                _ => "fact",
            }
            .to_string();
        }
        if let Some(tags) = patch.get("tags").and_then(|v| v.as_array()) {
            entry.tags = tags
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
        entry.updated_at = now_ts();
        self.runtime.memory.save(&entry);
        Ok(json!({ "memory": entry }))
    }

    /// `DELETE /memories/{id}`.
    pub fn delete_memory(&self, id: &str) -> ApiResult<Value> {
        self.runtime.memory.delete(id)?;
        Ok(json!({ "ok": true }))
    }
}
