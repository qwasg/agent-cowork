//! Context-window compaction. Keeps the system message and the most recent
//! turns; once the estimated token budget is exceeded the oldest middle turns
//! are either summarized via the LLM (preferred, port of
//! `context_compactor.py`) or hard-dropped as a fallback.

use crate::contracts::models::ChatMessage;
use crate::provider::tokens::estimate_tokens;
use crate::provider::types::{noop_sink, ProviderRequest};
use crate::provider::ProviderExecutionService;

const KEEP_TAIL: usize = 6;
/// Cap on the transcript fed to the summarizer (chars).
const SUMMARY_INPUT_MAX_CHARS: usize = 24_000;

/// Token estimate for one message, including tool-call payloads (the previous
/// version ignored `tool_calls`, under-counting tool-heavy contexts).
pub fn message_tokens(m: &ChatMessage) -> usize {
    estimate_tokens(&m.content)
        + m.reasoning.as_deref().map(estimate_tokens).unwrap_or(0)
        + m.tool_calls
            .iter()
            .map(|c| estimate_tokens(&c.name) + estimate_tokens(&c.arguments))
            .sum::<usize>()
        + 4
}

pub fn messages_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_tokens).sum()
}

/// Trim `messages` in place to fit under `budget_tokens`, preserving the
/// leading system message and the last `KEEP_TAIL` messages.
///
/// O(n): per-message token counts are computed once, then the drop prefix is
/// found in a single pass (the previous version recomputed the full total
/// after every removal — O(n²) on long histories).
pub fn compact(messages: &mut Vec<ChatMessage>, budget_tokens: usize) {
    let toks: Vec<usize> = messages.iter().map(message_tokens).collect();
    let mut total: usize = toks.iter().sum();
    if total <= budget_tokens {
        return;
    }
    let mut drop_n = 0usize;
    while total > budget_tokens && messages.len() - drop_n > KEEP_TAIL + 1 {
        total -= toks[1 + drop_n];
        drop_n += 1;
    }
    if drop_n > 0 {
        messages.drain(1..1 + drop_n);
    }
}

/// LLM-summary compaction: replace the oldest middle turns with a structured
/// summary message, falling back to hard truncation if summarization fails
/// (or only mock is available).
pub async fn compact_with_summary(
    providers: &ProviderExecutionService,
    model: &str,
    messages: &mut Vec<ChatMessage>,
    budget_tokens: usize,
) {
    if messages_tokens(messages) <= budget_tokens {
        return;
    }
    let keep_tail = KEEP_TAIL.min(messages.len().saturating_sub(1));
    let cut = messages.len() - keep_tail;
    // Need at least a few middle messages for a summary to be worthwhile.
    if cut > 3 {
        let mut transcript = String::new();
        for m in &messages[1..cut] {
            let snippet: String = m.content.chars().take(2_000).collect();
            transcript.push_str(&format!("[{}] {}\n", m.role, snippet));
            if transcript.len() > SUMMARY_INPUT_MAX_CHARS {
                break;
            }
        }
        let req = ProviderRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage::system(
                    "你是上下文压缩器。请把下面的对话历史压缩为结构化要点：已完成的事项、关键决策、重要文件/路径、未解决的问题。直接输出要点，不要其它说明。",
                ),
                ChatMessage::user(transcript),
            ],
            tools: vec![],
            temperature: Some(0.2),
            stream: false,
            max_tokens: Some(1024),
        };
        let sink = noop_sink();
        if let Ok(resp) = providers.execute(&req, &sink, None).await {
            if !resp.degraded && !resp.text.trim().is_empty() {
                let summary =
                    ChatMessage::system(format!("（早前对话的压缩摘要）\n{}", resp.text.trim()));
                messages.splice(1..cut, [summary]);
            }
        }
    }
    // Whatever happened above, guarantee we end up under budget.
    compact(messages, budget_tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::models::ToolCall;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage::simple(role, content)
    }

    #[test]
    fn counts_tool_call_tokens() {
        let mut m = msg("assistant", "");
        m.tool_calls.push(ToolCall {
            id: "c1".into(),
            kind: "function".into(),
            name: "write_file".into(),
            arguments: "x".repeat(400),
        });
        assert!(message_tokens(&m) > 100);
    }

    #[test]
    fn compact_keeps_system_and_tail() {
        let mut messages = vec![msg("system", "sys")];
        for i in 0..50 {
            messages.push(msg("user", &format!("message number {i} {}", "x".repeat(400))));
        }
        let last = messages.last().unwrap().content.clone();
        compact(&mut messages, 500);
        assert_eq!(messages[0].content, "sys");
        assert_eq!(messages.last().unwrap().content, last);
        assert!(messages.len() <= 7 + 1);
    }

    #[test]
    fn compact_noop_under_budget() {
        let mut messages = vec![msg("system", "sys"), msg("user", "hello")];
        compact(&mut messages, 10_000);
        assert_eq!(messages.len(), 2);
    }
}
