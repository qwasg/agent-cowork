//! System-surface endpoints: permission rules, lifecycle hooks and shell
//! job management.

use serde_json::{json, Value};

use agent_core::permission::PermissionRule;
use agent_protocol::{ApiError, ApiResult};

use crate::api::AppServices;

impl AppServices {
    /// `GET /permissions/rules`
    pub fn permission_rules(&self) -> Value {
        let rules: Vec<Value> = self
            .permissions
            .rules()
            .into_iter()
            .map(|r| json!({ "action": r.action, "pattern": r.pattern }))
            .collect();
        json!({ "rules": rules })
    }

    /// `PUT /permissions/rules` — replace the global rule list.
    pub fn set_permission_rules(&self, body: &Value) -> ApiResult<Value> {
        let rules: Vec<PermissionRule> =
            serde_json::from_value(body.get("rules").cloned().unwrap_or(json!([])))
                .map_err(|e| ApiError::new("AUTH_INVALID_INPUT", format!("invalid rules: {e}")))?;
        let saved = self.permissions.set_rules(rules)?;
        Ok(json!({
            "ok": true,
            "rules": saved
                .into_iter()
                .map(|r| json!({ "action": r.action, "pattern": r.pattern }))
                .collect::<Vec<_>>(),
        }))
    }

    /// `GET /hooks` — the parsed hook configuration (`data/hooks.json`).
    pub fn list_hooks(&self) -> Value {
        let hooks: Vec<Value> = self
            .runtime
            .hooks
            .hooks()
            .into_iter()
            .map(|h| {
                json!({
                    "event": h.event,
                    "matcher": h.matcher,
                    "command": h.command,
                    "timeoutMs": h.timeout_ms,
                })
            })
            .collect();
        json!({ "hooks": hooks })
    }

    /// `GET /shells` — all known shell jobs.
    pub fn list_shells(&self) -> Value {
        let jobs: Vec<Value> = self
            .runtime
            .shell
            .jobs()
            .into_iter()
            .map(|j| {
                json!({
                    "id": j.id,
                    "command": j.command,
                    "running": j.is_running(),
                    "exitCode": j.exit_code(),
                    "outputPath": j.output_path.display().to_string(),
                })
            })
            .collect();
        json!({ "shells": jobs })
    }

    /// `GET /shells/{id}/output?offset=N` — resumable output read.
    pub fn shell_output(&self, id: &str, offset: u64) -> ApiResult<Value> {
        let (chunk, next, code) = self
            .runtime
            .shell
            .read_output(id, offset)
            .ok_or_else(|| ApiError::new("SHELL_NOT_FOUND", format!("unknown shell: {id}")))?;
        Ok(json!({
            "id": id,
            "output": chunk,
            "nextOffset": next,
            "running": code.is_none(),
            "exitCode": code,
        }))
    }

    /// `POST /shells/{id}:kill`
    pub fn kill_shell(&self, id: &str) -> Value {
        json!({ "ok": self.runtime.shell.kill(id), "id": id })
    }
}
