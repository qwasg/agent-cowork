//! Permission modes, interactive approvals, builtin subagents and skills.

use serde_json::{json, Value};

use agent_protocol::{ApiError, ApiResult};
use agent_tools::skill::discover_skills;

use crate::api::AppServices;

impl AppServices {
    pub fn get_permission_mode(&self, session_id: &str) -> Value {
        json!({ "sessionId": session_id, "mode": self.permissions.mode(session_id) })
    }

    pub fn set_permission_mode(&self, session_id: &str, mode: &str) -> ApiResult<Value> {
        Ok(json!({
            "sessionId": session_id,
            "mode": self.permissions.set_mode(session_id, mode)?,
        }))
    }

    pub fn list_builtin_subagents(&self) -> Value {
        json!({ "subagents": self.runtime.subagents.as_dicts() })
    }

    /// Resolve a pending interactive tool-permission request.
    pub fn resolve_permission(&self, request_id: &str, approved: bool) -> ApiResult<Value> {
        if self.permissions.resolve(request_id, approved) {
            Ok(json!({ "ok": true, "requestId": request_id, "approved": approved }))
        } else {
            Err(ApiError::new(
                "PERMISSION_REQUEST_NOT_FOUND",
                format!("permission request not found or already resolved: {request_id}"),
            ))
        }
    }

    /// Skill list, Python `list_skills` shape: `{items: [{name, summary,
    /// scope, path}]}` (a `skills` alias is kept for older clients).
    pub fn list_skills(&self) -> Value {
        let items: Vec<Value> = discover_skills(&self.skill_dirs);
        json!({ "items": items.clone(), "skills": items })
    }

    /// Python `read_skill_content` shape: `{skill: {name, path, content}}`.
    pub fn read_skill_content(&self, name: &str) -> ApiResult<Value> {
        if !agent_tools::skill::is_safe_skill_name(name) {
            return Err(ApiError::new(
                "SKILL_NOT_FOUND",
                format!("invalid skill name: {name}"),
            ));
        }
        for dir in &self.skill_dirs {
            let p = dir.join(name).join("SKILL.md");
            if let Ok(content) = std::fs::read_to_string(&p) {
                return Ok(json!({ "skill": {
                    "name": name,
                    "path": p.to_string_lossy(),
                    "content": content,
                } }));
            }
        }
        Err(ApiError::new(
            "SKILL_NOT_FOUND",
            format!("skill not found: {name}"),
        ))
    }
}
