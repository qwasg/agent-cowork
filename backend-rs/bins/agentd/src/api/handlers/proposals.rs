//! Code-edit proposal apply / discard.

use serde_json::{json, Value};

use agent_protocol::events::EventDraft;
use agent_protocol::models::now_ts;
use agent_protocol::{ApiError, ApiResult};

use crate::api::AppServices;

impl AppServices {
    pub fn list_proposals(&self, session_id: &str) -> ApiResult<Value> {
        if !session_id.is_empty() {
            self.sessions.get(session_id)?;
        }
        let records = if session_id.is_empty() {
            self.proposals.all()
        } else {
            self.proposals.list_for_session(session_id)
        };
        let items = serde_json::to_value(records).unwrap_or(json!([]));
        // Python serves `items`; agent-ide reads `proposals`.
        Ok(json!({ "items": items.clone(), "proposals": items }))
    }

    /// Apply a pending proposal: write each change's `proposedContent` into
    /// the workspace, then emit `agent.code_edit.applied` / `failed`.
    pub fn apply_proposal(&self, id: &str) -> ApiResult<Value> {
        let record = self.proposals.get(id).ok_or_else(|| {
            ApiError::new("PROPOSAL_NOT_FOUND", format!("proposal not found: {id}"))
        })?;
        if record.status != "pending" {
            return Err(ApiError::new(
                "PROPOSAL_INVALID_STATE",
                format!("proposal already {}", record.status),
            ));
        }
        let root = self.runtime.workspace_root();
        let mut applied: Vec<String> = Vec::new();
        for change in &record.changes {
            let Some(path) = change
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|p| !p.trim().is_empty())
            else {
                continue;
            };
            let content = change
                .get("proposedContent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let write = agent_tools::resolve_in_root(&root, path).and_then(|abs| {
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| ApiError::filesystem(e.to_string()))?;
                }
                std::fs::write(&abs, content.as_bytes())
                    .map_err(|e| ApiError::filesystem(e.to_string()))
            });
            if let Err(e) = write {
                self.proposals.mark_apply_failed(id, &e.message);
                self.bus.emit(
                    EventDraft::new(&record.session_id, "agent.code_edit.failed", "agent")
                        .payload(json!({
                            "proposalId": id,
                            "code": "PROPOSAL_APPLY_FAILED",
                            "message": e.message,
                        }))
                        .correlation(Some(id.to_string())),
                );
                return Err(ApiError::new("PROPOSAL_APPLY_FAILED", e.message));
            }
            applied.push(path.to_string());
        }
        let updated = self.proposals.mark_applied(id, applied.clone());
        self.bus.emit(
            EventDraft::new(&record.session_id, "agent.code_edit.applied", "agent")
                .payload(json!({
                    "proposalId": id,
                    "appliedAt": now_ts(),
                    "appliedFiles": applied,
                }))
                .correlation(Some(id.to_string())),
        );
        Ok(json!({ "proposal": updated }))
    }

    pub fn discard_proposal(&self, id: &str) -> ApiResult<Value> {
        let record = self.proposals.get(id).ok_or_else(|| {
            ApiError::new("PROPOSAL_NOT_FOUND", format!("proposal not found: {id}"))
        })?;
        let updated = self.proposals.mark_discarded(id);
        self.bus.emit(
            EventDraft::new(&record.session_id, "agent.code_edit.discarded", "agent")
                .payload(json!({ "proposalId": id, "discardedAt": now_ts() }))
                .correlation(Some(id.to_string())),
        );
        Ok(json!({ "proposal": updated }))
    }
}
