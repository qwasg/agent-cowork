//! Session CRUD, fork and revert.

use std::collections::HashMap;

use serde_json::{json, Value};

use agent_protocol::models::{new_id, now_ts};
use agent_protocol::ApiResult;

use crate::api::AppServices;

impl AppServices {
    pub fn list_sessions(&self) -> Value {
        json!({ "sessions": self.sessions.list() })
    }

    pub fn create_session(
        &self,
        title: &str,
        agent_kind: &str,
        model_id: Option<String>,
        web: bool,
    ) -> Value {
        let session = self.sessions.create(title, agent_kind, model_id, web);
        json!({ "session": session })
    }

    pub fn get_session(&self, id: &str) -> ApiResult<Value> {
        Ok(json!({ "session": self.sessions.get(id)? }))
    }

    pub fn patch_session(&self, id: &str, patch: &Value) -> ApiResult<Value> {
        Ok(json!({ "session": self.sessions.patch(id, patch)? }))
    }

    pub fn delete_session(&self, id: &str) -> ApiResult<Value> {
        self.sessions.delete(id)?;
        self.bus.purge_session(id);
        self.forget_context_window(id);
        Ok(json!({ "ok": true }))
    }

    /// Fork: new session inheriting model/web settings, with cloned plan,
    /// cloned todos (id-remapped dependencies), copied conversation history
    /// and a full copy of the event stream (port of Python `fork_session`).
    pub fn fork_session(&self, id: &str) -> ApiResult<Value> {
        let src = self.sessions.get(id)?;
        let mut forked = self.sessions.create(
            &format!("分支 · {}", src.title),
            &src.agent_kind,
            src.selected_model_id.clone(),
            src.web_search_enabled,
        );

        // Clone todos with new ids, remapping dependency edges.
        let src_todos = self.todos.list_by_session(id);
        let mut todo_id_map: HashMap<String, String> = HashMap::new();
        for t in &src_todos {
            todo_id_map.insert(t.id.clone(), new_id("todo"));
        }
        for t in &src_todos {
            let mut cloned = t.clone();
            cloned.id = todo_id_map[&t.id].clone();
            cloned.session_id = forked.id.clone();
            cloned.related_agent_run_id = None;
            cloned.dependencies = t
                .dependencies
                .iter()
                .filter_map(|d| todo_id_map.get(d).cloned())
                .collect();
            self.todos.save(&cloned);
        }

        // Clone the active plan (new plan/stage ids, task ids follow todos).
        if let Some(plan_id) = src.active_plan_id.clone() {
            if let Ok(src_plan) = self.plan_engine.get(&plan_id) {
                let mut plan = src_plan.clone();
                plan.id = new_id("plan");
                plan.session_id = forked.id.clone();
                plan.latest_execution_id = None;
                plan.created_at = now_ts();
                plan.updated_at = now_ts();
                for stage in &mut plan.stages {
                    stage.id = new_id("stage");
                    stage.plan_id = plan.id.clone();
                    for task in &mut stage.tasks {
                        if let Some(mapped) = todo_id_map.get(&task.id) {
                            task.id = mapped.clone();
                        }
                        task.stage_id = stage.id.clone();
                        task.depends_on = task
                            .depends_on
                            .iter()
                            .map(|d| todo_id_map.get(d).cloned().unwrap_or_else(|| d.clone()))
                            .collect();
                    }
                }
                self.plan_engine.save(&plan);
                forked.active_plan_id = Some(plan.id.clone());
                forked.touch();
                self.sessions.save(&forked);
            }
        }

        self.sessions.copy_history(id, &forked.id);
        self.bus.fork_session(id, &forked.id);
        Ok(json!({ "session": forked }))
    }

    /// Revert: truncate the event stream at `message_id` (`mode=before` is
    /// exclusive — used by "edit & resend"), drop conversation history and
    /// reset active plan/run pointers (port of Python `revert_session`).
    pub fn revert_session(
        &self,
        id: &str,
        message_id: Option<&str>,
        mode: Option<&str>,
    ) -> ApiResult<Value> {
        let mut session = self.sessions.get(id)?;
        if let Some(message_id) = message_id.filter(|m| !m.trim().is_empty()) {
            if mode == Some("before") {
                self.bus.truncate_before_event(id, message_id);
            } else {
                self.bus.truncate_session(id, message_id);
            }
            self.sessions.clear_history(id);
            session.active_run_id = None;
            session.active_plan_id = None;
            session.status = "idle".to_string();
        } else {
            session.active_run_id = None;
        }
        session.touch();
        self.sessions.save(&session);
        Ok(json!({ "session": session }))
    }

    pub fn set_session_model(&self, id: &str, model_id: Option<String>) -> ApiResult<Value> {
        Ok(json!({ "session": self.sessions.set_model(id, model_id)? }))
    }
}
