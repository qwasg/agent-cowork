//! Session rollout schema (Codex-style append-only conversation log).
//!
//! A session's conversation history is a JSONL stream of [`RolloutItem`]s at
//! `data/agent-sessions/{id}/rollout.jsonl`. Appends are O(1), the file is
//! crash-recoverable, and resume / fork / revert are file operations.
//!
//! Tool calls ride inside the assistant [`ChatMessage`] (`tool_calls`) and
//! tool results are `role: "tool"` messages with `tool_call_id` — the same
//! wire shape the providers consume, so rebuilding a prompt is a straight map.

use serde::{Deserialize, Serialize};

use crate::models::ChatMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RolloutItem {
    /// Marks the start of a turn. Revert / checkpoint truncation cuts the
    /// file at these boundaries.
    TurnBoundary { turn_id: String, ts: String },
    /// Any conversation message: user / assistant (may carry `tool_calls`) /
    /// tool result (`role: "tool"`).
    Message { message: ChatMessage },
    /// Context-compaction marker: `summary` stands in for everything that
    /// came before this line when rebuilding the prompt.
    Compaction { summary: String, ts: String },
}

impl RolloutItem {
    pub fn message(message: ChatMessage) -> Self {
        RolloutItem::Message { message }
    }
}

/// Rebuild the (compaction-aware) message sequence from rollout items.
///
/// On a `Compaction` marker, everything collected so far is replaced with a
/// single system message carrying the summary.
pub fn rebuild_messages(items: &[RolloutItem]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for item in items {
        match item {
            RolloutItem::TurnBoundary { .. } => {}
            RolloutItem::Message { message } => out.push(message.clone()),
            RolloutItem::Compaction { summary, .. } => {
                out.clear();
                out.push(ChatMessage::system(format!("[对话压缩摘要] {summary}")));
            }
        }
    }
    out
}
