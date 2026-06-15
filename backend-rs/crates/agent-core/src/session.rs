//! Session service: durable CRUD + conversation history.
//!
//! History lives in an append-only rollout JSONL per session (Codex-style,
//! see [`agent_store::rollout`]); redb keeps only session metadata. Legacy
//! `hist:{id}` blobs in redb are migrated to rollout on first access.

use std::sync::Arc;

use agent_protocol::models::{new_id, now_ts, ChatMessage, DebugSession};
use agent_protocol::rollout::{rebuild_messages, RolloutItem};
use agent_protocol::{ApiError, ApiResult};
use agent_store::store::T_SESSIONS;
use agent_store::{RolloutStore, Store};

pub struct SessionService {
    store: Arc<Store>,
    rollout: Arc<RolloutStore>,
    history_turns: usize,
}

impl SessionService {
    pub fn new(store: Arc<Store>, rollout: Arc<RolloutStore>, history_turns: usize) -> Self {
        SessionService {
            store,
            rollout,
            history_turns,
        }
    }

    /// Direct access to the rollout log (turn boundaries, compaction markers).
    pub fn rollout(&self) -> &Arc<RolloutStore> {
        &self.rollout
    }

    pub fn create(
        &self,
        title: &str,
        agent_kind: &str,
        model_id: Option<String>,
        web_search: bool,
    ) -> DebugSession {
        let title = if title.trim().is_empty() {
            "Agent Debug Session".to_string()
        } else {
            title.to_string()
        };
        // Normalize to a canonical kind string (general / document / coding).
        let kind = crate::profile::AgentKind::from_str(agent_kind)
            .as_str()
            .to_string();
        let session = DebugSession::new(new_id("sess"), title, kind, model_id, web_search);
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
        if let Some(kind) = patch.get("agentKind").and_then(|v| v.as_str()) {
            session.agent_kind = crate::profile::AgentKind::from_str(kind)
                .as_str()
                .to_string();
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
        self.rollout.delete(id);
        self.store.kv_delete(&hist_key(id)).ok();
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

    // ---- conversation history (rollout-backed) ----

    /// Rebuild the recent message window from the rollout log (respecting
    /// compaction markers), trimmed to the configured turn budget.
    ///
    /// The rollout keeps full fidelity (assistant tool calls, tool results);
    /// the prompt window feeds providers only user / assistant text plus the
    /// compaction summary, so trimming can never orphan a tool-call pair.
    pub fn history(&self, id: &str) -> Vec<ChatMessage> {
        self.ensure_migrated(id);
        let mut msgs: Vec<ChatMessage> = rebuild_messages(&self.rollout.read(id))
            .into_iter()
            .filter(|m| match m.role.as_str() {
                "user" | "system" => true,
                "assistant" => !m.content.trim().is_empty(),
                _ => false,
            })
            .map(|mut m| {
                m.tool_calls.clear();
                m
            })
            .collect();
        let max = self.history_turns * 2;
        if msgs.len() > max {
            msgs = msgs.split_off(msgs.len() - max);
        }
        msgs
    }

    /// Append a completed turn (O(1) — no rewrite of prior history).
    pub fn remember_turn(&self, id: &str, user: ChatMessage, assistant: ChatMessage) {
        self.append_turn(id, &new_id("turn"), user, vec![assistant]);
    }

    /// Persist a full turn: boundary marker + user input + the turn's
    /// wire-shape messages (assistant tool calls / tool results included).
    pub fn append_turn(
        &self,
        id: &str,
        turn_id: &str,
        user: ChatMessage,
        turn_msgs: Vec<ChatMessage>,
    ) {
        self.ensure_migrated(id);
        let mut items = Vec::with_capacity(turn_msgs.len() + 2);
        items.push(RolloutItem::TurnBoundary {
            turn_id: turn_id.to_string(),
            ts: now_ts(),
        });
        items.push(RolloutItem::message(user));
        items.extend(turn_msgs.into_iter().map(RolloutItem::message));
        let _ = self.rollout.append_many(id, &items);
    }

    /// Record a context-compaction marker: when the prompt window is rebuilt
    /// from the rollout, everything before this line collapses into `summary`.
    pub fn note_compaction(&self, id: &str, summary: &str) {
        self.ensure_migrated(id);
        let _ = self.rollout.append_many(
            id,
            &[RolloutItem::Compaction {
                summary: summary.to_string(),
                ts: now_ts(),
            }],
        );
    }

    /// Drop the persisted conversation history (used by session revert so the
    /// runtime doesn't keep "remembering" truncated turns).
    pub fn clear_history(&self, id: &str) {
        self.rollout.delete(id);
        self.store.kv_delete(&hist_key(id)).ok();
    }

    /// Copy conversation history from one session to another (session fork —
    /// a file copy on the rollout log).
    pub fn copy_history(&self, from: &str, to: &str) {
        self.ensure_migrated(from);
        let _ = self.rollout.fork(from, to);
    }

    /// One-time migration: convert a legacy `hist:{id}` redb blob into a
    /// rollout file, then drop the blob. No-op once the rollout file exists.
    fn ensure_migrated(&self, id: &str) {
        if self.rollout.exists(id) {
            return;
        }
        let Some(raw) = self.store.kv_get(&hist_key(id)) else {
            return;
        };
        let msgs: Vec<ChatMessage> = serde_json::from_str(&raw).unwrap_or_default();
        if !msgs.is_empty() {
            let mut items = Vec::with_capacity(msgs.len() + msgs.len() / 2);
            for msg in msgs {
                if msg.role == "user" {
                    items.push(RolloutItem::TurnBoundary {
                        turn_id: new_id("turn"),
                        ts: now_ts(),
                    });
                }
                items.push(RolloutItem::message(msg));
            }
            if self.rollout.append_many(id, &items).is_err() {
                // Keep the legacy blob so the next read retries the migration.
                return;
            }
            tracing::info!("session {id}: migrated legacy history to rollout");
        }
        self.store.kv_delete(&hist_key(id)).ok();
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
