//! Session service: durable CRUD + conversation history (port of
//! `session_service.py`). History is persisted so it survives restarts.

use std::sync::Arc;

use crate::contracts::models::{new_id, ChatMessage, DebugSession};
use crate::contracts::{ApiError, ApiResult};
use crate::infra::store::T_SESSIONS;
use crate::infra::Store;

pub struct SessionService {
    store: Arc<Store>,
    history_turns: usize,
}

impl SessionService {
    pub fn new(store: Arc<Store>, history_turns: usize) -> Self {
        SessionService {
            store,
            history_turns,
        }
    }

    pub fn create(&self, title: &str, model_id: Option<String>, web_search: bool) -> DebugSession {
        let title = if title.trim().is_empty() {
            "Agent Debug Session".to_string()
        } else {
            title.to_string()
        };
        let session = DebugSession::new(new_id("sess"), title, model_id, web_search);
        let _ = self.store.put(T_SESSIONS, &session.id, &session);
        session
    }

    pub fn get(&self, id: &str) -> ApiResult<DebugSession> {
        self.store
            .get::<DebugSession>(T_SESSIONS, id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::session_not_found(id))
    }

    pub fn list(&self) -> Vec<DebugSession> {
        let mut sessions = self
            .store
            .list::<DebugSession>(T_SESSIONS)
            .unwrap_or_default();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }

    pub fn save(&self, session: &DebugSession) {
        let _ = self.store.put(T_SESSIONS, &session.id, session);
    }

    pub fn patch(&self, id: &str, patch: &serde_json::Value) -> ApiResult<DebugSession> {
        let mut session = self.get(id)?;
        if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
            if title.trim().is_empty() {
                return Err(ApiError::new("INVALID_TITLE", "title cannot be empty"));
            }
            session.title = title.to_string();
            session.title_manually_set = true;
        }
        if let Some(pinned) = patch.get("pinned").and_then(|v| v.as_bool()) {
            session.pinned = pinned;
        }
        if let Some(ws) = patch.get("webSearchEnabled").and_then(|v| v.as_bool()) {
            session.web_search_enabled = ws;
        }
        if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
            session.status = status.to_string();
        }
        session.touch();
        self.save(&session);
        Ok(session)
    }

    pub fn delete(&self, id: &str) -> ApiResult<()> {
        let existed = self.store.delete(T_SESSIONS, id).unwrap_or(false);
        if !existed {
            return Err(ApiError::session_not_found(id));
        }
        self.store.kv_put(&hist_key(id), "[]").ok();
        Ok(())
    }

    pub fn set_model(&self, id: &str, model_id: Option<String>) -> ApiResult<DebugSession> {
        let mut session = self.get(id)?;
        session.selected_model_id = model_id;
        session.touch();
        self.save(&session);
        Ok(session)
    }

    pub fn auto_title_from_input(&self, id: &str, user_input: &str) {
        if let Ok(mut session) = self.get(id) {
            if !session.title_manually_set
                && (session.title == "Agent Debug Session" || session.title.trim().is_empty())
            {
                let trimmed = user_input.trim();
                if !trimmed.is_empty() {
                    session.title = trimmed.chars().take(48).collect();
                    session.touch();
                    self.save(&session);
                }
            }
        }
    }

    // ---- conversation history ----

    pub fn history(&self, id: &str) -> Vec<ChatMessage> {
        self.store
            .kv_get(&hist_key(id))
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn remember_turn(&self, id: &str, user: ChatMessage, assistant: ChatMessage) {
        let mut hist = self.history(id);
        hist.push(user);
        hist.push(assistant);
        let max = self.history_turns * 2;
        if hist.len() > max {
            hist = hist.split_off(hist.len() - max);
        }
        if let Ok(raw) = serde_json::to_string(&hist) {
            let _ = self.store.kv_put(&hist_key(id), &raw);
        }
    }

    pub fn set_active_run(&self, id: &str, run_id: Option<String>, plan_id: Option<String>) {
        if let Ok(mut session) = self.get(id) {
            session.active_run_id = run_id;
            if plan_id.is_some() {
                session.active_plan_id = plan_id;
            }
            session.touch();
            self.save(&session);
        }
    }
}

fn hist_key(id: &str) -> String {
    format!("hist:{id}")
}
