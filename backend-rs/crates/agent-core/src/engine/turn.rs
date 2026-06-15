//! Turn model: one user input → one [`Turn`] made of [`TurnItem`]s.
//!
//! Every item flows through [`Runtime::record_item`], which is the single
//! place that maps engine progress onto EventBus events — replay (rollout)
//! and the live stream can no longer drift apart. At the end of a successful
//! turn the items are converted back into wire-shape [`ChatMessage`]s and
//! appended to the session rollout.

use serde_json::json;

use agent_protocol::models::{new_id, ChatMessage, ToolCall};

use super::Runtime;

#[derive(Debug, Clone)]
pub enum TurnItem {
    Reasoning {
        text: String,
    },
    AssistantText {
        text: String,
        provider: String,
        degraded: bool,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
        /// Wall-clock execution time, surfaced in events for run metrics.
        duration_ms: u64,
    },
    /// A user message injected mid-run via the steering queue.
    SteeredUser {
        text: String,
    },
    Error {
        code: String,
        message: String,
    },
}

pub struct Turn {
    pub id: String,
    pub session_id: String,
    pub run_id: String,
    pub items: Vec<TurnItem>,
}

impl Turn {
    pub fn new(session_id: &str, run_id: &str) -> Self {
        Turn {
            id: new_id("turn"),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            items: Vec::new(),
        }
    }

    /// Convert the item stream back into provider wire-shape messages:
    /// assistant messages carry their `tool_calls`, tool results become
    /// `role: "tool"` messages. Reasoning / Error items are events-only.
    pub fn to_messages(&self) -> Vec<ChatMessage> {
        let mut out: Vec<ChatMessage> = Vec::new();
        let mut pending_text: Option<String> = None;
        let mut pending_calls: Vec<ToolCall> = Vec::new();

        fn flush(out: &mut Vec<ChatMessage>, text: &mut Option<String>, calls: &mut Vec<ToolCall>) {
            if text.is_none() && calls.is_empty() {
                return;
            }
            out.push(ChatMessage {
                role: "assistant".to_string(),
                content: text.take().unwrap_or_default(),
                reasoning: None,
                tool_calls: std::mem::take(calls),
                tool_call_id: None,
                name: None,
            });
        }

        for item in &self.items {
            match item {
                TurnItem::Reasoning { .. } | TurnItem::Error { .. } => {}
                TurnItem::SteeredUser { text } => {
                    flush(&mut out, &mut pending_text, &mut pending_calls);
                    out.push(ChatMessage::user(text.clone()));
                }
                TurnItem::AssistantText { text, .. } => {
                    flush(&mut out, &mut pending_text, &mut pending_calls);
                    pending_text = Some(text.clone());
                }
                TurnItem::ToolCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    pending_calls.push(ToolCall {
                        id: call_id.clone(),
                        kind: "function".to_string(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
                TurnItem::ToolResult {
                    call_id,
                    name,
                    output,
                    ..
                } => {
                    flush(&mut out, &mut pending_text, &mut pending_calls);
                    out.push(ChatMessage::tool_result(
                        call_id.clone(),
                        name.clone(),
                        output.clone(),
                    ));
                }
            }
        }
        flush(&mut out, &mut pending_text, &mut pending_calls);
        out
    }
}

impl Runtime {
    /// The single emission point for turn progress: push the item onto the
    /// turn *and* emit the matching EventBus event.
    pub(crate) fn record_item(&self, turn: &mut Turn, item: TurnItem) {
        let (etype, domain, payload) = match &item {
            TurnItem::Reasoning { text } => (
                "agent.reasoning",
                "agent",
                json!({ "text": text, "runId": turn.run_id }),
            ),
            TurnItem::AssistantText {
                text,
                provider,
                degraded,
            } => (
                "agent.message",
                "agent",
                json!({
                    "text": text,
                    "runId": turn.run_id,
                    "provider": provider,
                    "degraded": degraded,
                }),
            ),
            TurnItem::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(json!({}));
                (
                    "agent.tool.invoked",
                    "tool",
                    json!({ "name": name, "args": args, "toolCallId": call_id, "runId": turn.run_id }),
                )
            }
            TurnItem::ToolResult {
                call_id,
                name,
                output,
                is_error,
                duration_ms,
            } => {
                if *is_error {
                    (
                        "agent.tool.failed",
                        "tool",
                        json!({ "name": name, "error": output, "toolCallId": call_id, "runId": turn.run_id, "durationMs": duration_ms }),
                    )
                } else {
                    (
                        "agent.tool.completed",
                        "tool",
                        json!({ "name": name, "ok": true, "toolCallId": call_id, "runId": turn.run_id, "durationMs": duration_ms }),
                    )
                }
            }
            TurnItem::SteeredUser { text } => (
                "agent.steered",
                "agent",
                json!({ "text": text, "runId": turn.run_id, "phase": "injected" }),
            ),
            TurnItem::Error { code, message } => (
                "agent.failed",
                "agent",
                json!({ "runId": turn.run_id, "code": code, "error": message }),
            ),
        };
        self.emit(
            &turn.session_id,
            etype,
            domain,
            payload,
            Some(turn.run_id.clone()),
        );
        turn.items.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_messages_groups_calls_under_assistant() {
        let mut turn = Turn::new("s", "r");
        turn.items = vec![
            TurnItem::Reasoning {
                text: "think".into(),
            },
            TurnItem::AssistantText {
                text: "let me check".into(),
                provider: "mock".into(),
                degraded: false,
            },
            TurnItem::ToolCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"a.rs"}"#.into(),
            },
            TurnItem::ToolCall {
                call_id: "c2".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"b.rs"}"#.into(),
            },
            TurnItem::ToolResult {
                call_id: "c1".into(),
                name: "read_file".into(),
                output: "fn a() {}".into(),
                is_error: false,
                duration_ms: 5,
            },
            TurnItem::ToolResult {
                call_id: "c2".into(),
                name: "read_file".into(),
                output: "ERROR[NOT_FOUND]: nope".into(),
                is_error: true,
                duration_ms: 5,
            },
            TurnItem::AssistantText {
                text: "done".into(),
                provider: "mock".into(),
                degraded: false,
            },
        ];
        let msgs = turn.to_messages();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "assistant");
        assert_eq!(msgs[0].tool_calls.len(), 2);
        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("c2"));
        assert_eq!(msgs[3].content, "done");
        assert!(msgs[3].tool_calls.is_empty());
    }

    #[test]
    fn to_messages_skips_reasoning_and_errors() {
        let mut turn = Turn::new("s", "r");
        turn.items = vec![
            TurnItem::Reasoning { text: "hm".into() },
            TurnItem::Error {
                code: "X".into(),
                message: "boom".into(),
            },
        ];
        assert!(turn.to_messages().is_empty());
    }
}
