//! Todo REST operations.

use serde_json::{json, Value};

use agent_protocol::events::EventDraft;
use agent_protocol::ApiResult;

use crate::api::AppServices;

impl AppServices {
    pub fn get_todos(&self, session_id: &str) -> Value {
        let items =
            serde_json::to_value(self.todos.list_by_session(session_id)).unwrap_or(json!([]));
        // Python serves `items`; older agent-ide builds read `todos`.
        json!({ "items": items.clone(), "todos": items })
    }

    pub fn add_todo(&self, payload: &Value) -> ApiResult<Value> {
        Ok(json!({ "todo": self.todos.add_from_payload(payload)? }))
    }

    pub fn patch_todo(&self, id: &str, patch: &Value) -> ApiResult<Value> {
        let todo = self.todos.patch(id, patch)?;
        // Manual REST edits surface in the event stream too, so the checklist
        // UI stays consistent with agent-driven updates.
        self.bus.emit(
            EventDraft::new(&todo.session_id, "todo.updated", "todo").payload(
                json!({ "id": todo.id, "title": todo.title, "kind": todo.kind, "status": todo.status }),
            ),
        );
        Ok(json!({ "todo": todo }))
    }

    pub fn batch_rerun_todos(&self, payload: &Value) -> ApiResult<Value> {
        let ids = payload
            .get("todoIds")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in &ids {
            self.todos.set_status(id, "queued");
        }
        Ok(json!({ "ok": true, "requeued": ids }))
    }
}
