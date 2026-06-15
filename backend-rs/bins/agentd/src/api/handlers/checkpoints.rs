//! Checkpoint create / list / rewind.

use serde_json::{json, Value};

use agent_protocol::events::EventDraft;
use agent_protocol::ApiResult;

use crate::api::AppServices;

impl AppServices {
    pub fn list_checkpoints(&self, session_id: &str) -> Value {
        self.checkpoints.list(session_id)
    }

    pub fn create_checkpoint(
        &self,
        session_id: &str,
        paths: Option<Vec<String>>,
        label: &str,
    ) -> Value {
        // Capture the current event seq so a rewind can truncate the stream.
        let event_seq = self.bus.latest_seq(session_id);
        self.checkpoints.create(session_id, paths, label, event_seq)
    }

    /// Rewind restores files *and* truncates the session's event stream back
    /// to the checkpoint's seq, invalidating later conversation turns.
    pub fn rewind_checkpoint(&self, id: &str) -> ApiResult<Value> {
        let result = self.checkpoints.rewind(id)?;
        let session_id = result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let event_seq = result.get("eventSeq").and_then(|v| v.as_i64()).unwrap_or(0);
        if !session_id.is_empty() && event_seq > 0 {
            self.bus.truncate_to_seq(&session_id, event_seq);
            self.sessions.clear_history(&session_id);
            self.bus.emit(
                EventDraft::new(&session_id, "checkpoint.rewound", "session")
                    .payload(json!({ "checkpointId": id, "eventSeq": event_seq })),
            );
        }
        Ok(result)
    }
}
