//! Todo engine with a dependency DAG (port of `todo_engine.py`).
//! `ready_todos` returns queued items whose dependencies are all completed,
//! enabling parallel batch execution in the runtime.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;

use agent_protocol::models::{new_id, now_ts, TodoItem};
use agent_protocol::{ApiError, ApiResult};
use agent_store::store::{IDX_TODOS_BY_SESSION, T_TODOS};
use agent_store::Store;

pub struct TodoEngine {
    store: Arc<Store>,
}

impl TodoEngine {
    pub fn new(store: Arc<Store>) -> Self {
        TodoEngine { store }
    }

    pub fn list_by_session(&self, session_id: &str) -> Vec<TodoItem> {
        // Fast path: secondary index (session -> todo ids). Falls back to a
        // full scan for databases written before the index existed.
        let ids = self.store.index_values(IDX_TODOS_BY_SESSION, session_id);
        let mut todos: Vec<TodoItem> = if ids.is_empty() {
            self.store
                .list::<TodoItem>(T_TODOS)
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.session_id == session_id)
                .collect()
        } else {
            ids.iter()
                .filter_map(|id| self.store.get::<TodoItem>(T_TODOS, id).ok().flatten())
                .collect()
        };
        todos.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        todos
    }

    pub fn get(&self, id: &str) -> ApiResult<TodoItem> {
        self.store
            .get::<TodoItem>(T_TODOS, id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::todo_not_found(id))
    }

    pub fn save(&self, todo: &TodoItem) {
        let _ = self.store.put(T_TODOS, &todo.id, todo);
        // Idempotent; also self-heals index entries for legacy rows.
        let _ = self
            .store
            .index_add(IDX_TODOS_BY_SESSION, &todo.session_id, &todo.id);
    }

    pub fn add(
        &self,
        session_id: &str,
        title: &str,
        description: &str,
        kind: &str,
        deps: Vec<String>,
    ) -> TodoItem {
        let ts = now_ts();
        let todo = TodoItem {
            id: new_id("todo"),
            session_id: session_id.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            kind: normalize_kind(kind),
            source: "agent".to_string(),
            priority: 3,
            status: "queued".to_string(),
            dependencies: deps,
            related_agent_run_id: None,
            retry_count: 0,
            summary: None,
            last_error: None,
            created_at: ts.clone(),
            updated_at: ts,
        };
        self.save(&todo);
        todo
    }

    pub fn add_from_payload(&self, payload: &Value) -> ApiResult<TodoItem> {
        let session_id = payload
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::new("TODO_INVALID", "sessionId required"))?;
        let title = payload
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            return Err(ApiError::new("TODO_INVALID", "title required"));
        }
        let description = payload
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let kind = payload
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("edit");
        let deps = payload
            .get("dependencies")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(self.add(session_id, title, description, kind, deps))
    }

    pub fn patch(&self, id: &str, patch: &Value) -> ApiResult<TodoItem> {
        let mut todo = self.get(id)?;
        if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
            todo.status = status.to_string();
        }
        if let Some(kind) = patch.get("kind").and_then(|v| v.as_str()) {
            todo.kind = normalize_kind(kind);
        }
        if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
            todo.title = title.to_string();
        }
        if let Some(desc) = patch.get("description").and_then(|v| v.as_str()) {
            todo.description = desc.to_string();
        }
        if let Some(summary) = patch.get("summary").and_then(|v| v.as_str()) {
            todo.summary = Some(summary.to_string());
        }
        todo.updated_at = now_ts();
        self.save(&todo);
        Ok(todo)
    }

    pub fn set_status(&self, id: &str, status: &str) {
        if let Ok(mut todo) = self.get(id) {
            todo.status = status.to_string();
            todo.updated_at = now_ts();
            self.save(&todo);
        }
    }

    /// Queued todos whose dependencies are all completed, exploration first.
    ///
    /// Structural "explore before edit" guarantee: an `edit` todo is held back
    /// while any `explore` todo of the session is still unfinished, even when
    /// the planner produced no explicit dependency edges. Within the result,
    /// `explore` todos sort ahead of `edit` todos.
    pub fn ready_todos(&self, session_id: &str) -> Vec<TodoItem> {
        let todos = self.list_by_session(session_id);
        let completed: HashSet<String> = todos
            .iter()
            .filter(|t| t.status == "completed")
            .map(|t| t.id.clone())
            .collect();
        let explore_pending = todos
            .iter()
            .any(|t| t.kind == "explore" && !is_terminal_status(&t.status));
        let mut ready: Vec<TodoItem> = todos
            .into_iter()
            .filter(|t| {
                matches!(t.status.as_str(), "queued" | "ready")
                    && t.dependencies.iter().all(|d| completed.contains(d))
                    && !(t.kind == "edit" && explore_pending)
            })
            .collect();
        ready.sort_by(|a, b| {
            kind_rank(&a.kind)
                .cmp(&kind_rank(&b.kind))
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        ready
    }

    /// Queued todos that can never become ready because a dependency ended in
    /// a terminal non-completed state.
    pub fn blocked_todos(&self, session_id: &str) -> Vec<TodoItem> {
        let todos = self.list_by_session(session_id);
        let dead: HashSet<String> = todos
            .iter()
            .filter(|t| is_terminal_status(&t.status) && t.status != "completed")
            .map(|t| t.id.clone())
            .collect();
        todos
            .into_iter()
            .filter(|t| {
                matches!(t.status.as_str(), "queued" | "ready")
                    && t.dependencies.iter().any(|d| dead.contains(d))
            })
            .collect()
    }
}

fn normalize_kind(kind: &str) -> String {
    match kind.trim() {
        "explore" | "research" | "investigate" => "explore",
        _ => "edit",
    }
    .to_string()
}

fn kind_rank(kind: &str) -> u8 {
    if kind == "explore" {
        0
    } else {
        1
    }
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "blocked")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> TodoEngine {
        let path = std::env::temp_dir().join(format!("agentd_todo_{}.redb", new_id("t")));
        TodoEngine::new(Arc::new(Store::open(path).unwrap()))
    }

    #[test]
    fn dag_gates_dependent_todos() {
        let eng = engine();
        let sid = "sess_dag";
        let a = eng.add(sid, "task a", "", "edit", vec![]);
        let b = eng.add(sid, "task b", "", "edit", vec![a.id.clone()]);
        let c = eng.add(sid, "task c", "", "edit", vec![]);

        let ready: Vec<String> = eng.ready_todos(sid).into_iter().map(|t| t.id).collect();
        assert!(ready.contains(&a.id));
        assert!(ready.contains(&c.id));
        assert!(!ready.contains(&b.id), "b depends on a, not ready yet");

        eng.set_status(&a.id, "completed");
        let ready: Vec<String> = eng.ready_todos(sid).into_iter().map(|t| t.id).collect();
        assert!(ready.contains(&b.id), "b becomes ready once a completes");
    }

    #[test]
    fn list_by_session_uses_index() {
        let eng = engine();
        eng.add("s1", "one", "", "edit", vec![]);
        eng.add("s1", "two", "", "edit", vec![]);
        eng.add("s2", "other", "", "edit", vec![]);
        assert_eq!(eng.list_by_session("s1").len(), 2);
        assert_eq!(eng.list_by_session("s2").len(), 1);
        assert!(eng.list_by_session("s3").is_empty());
    }

    #[test]
    fn explore_todos_gate_and_sort_before_edit() {
        let eng = engine();
        let sid = "sess_phase";
        let edit = eng.add(sid, "implement", "", "edit", vec![]);
        let exp = eng.add(sid, "investigate", "", "explore", vec![]);

        // While the explore todo is unfinished, only it is ready.
        let ready = eng.ready_todos(sid);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, exp.id);

        // Once exploration finishes, the edit todo is released.
        eng.set_status(&exp.id, "completed");
        let ready = eng.ready_todos(sid);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, edit.id);
    }

    #[test]
    fn explore_sorts_ahead_within_ready_set() {
        let eng = engine();
        let sid = "sess_sort";
        eng.add(sid, "e1", "", "explore", vec![]);
        eng.add(sid, "e2", "", "explore", vec![]);
        let ready = eng.ready_todos(sid);
        assert!(ready.iter().all(|t| t.kind == "explore"));
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn blocked_todos_detects_dead_dependencies() {
        let eng = engine();
        let sid = "sess_blocked";
        let a = eng.add(sid, "a", "", "edit", vec![]);
        let b = eng.add(sid, "b", "", "edit", vec![a.id.clone()]);
        eng.set_status(&a.id, "failed");
        let blocked: Vec<String> = eng.blocked_todos(sid).into_iter().map(|t| t.id).collect();
        assert_eq!(blocked, vec![b.id]);
    }

    #[test]
    fn normalizes_kind() {
        let eng = engine();
        let t = eng.add("s_kind", "x", "", "research", vec![]);
        assert_eq!(t.kind, "explore");
        let t = eng.add("s_kind", "y", "", "whatever", vec![]);
        assert_eq!(t.kind, "edit");
    }
}
