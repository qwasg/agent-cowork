//! Context-window compaction. Keeps the system message and the most recent
//! turns; once the estimated token budget is exceeded the oldest middle turns
//! are either summarized via the LLM (preferred, port of
//! `context_compactor.py`) or hard-dropped as a fallback.

use agent_protocol::models::ChatMessage;
use agent_providers::tokens::estimate_tokens;
use agent_providers::types::{noop_sink, ProviderRequest};
use agent_providers::ProviderExecutionService;

const KEEP_TAIL: usize = 6;
/// Cap on the transcript fed to the summarizer (chars).
const SUMMARY_INPUT_MAX_CHARS: usize = 24_000;
/// Tool-result decay: never touch the most recent N messages…
const DECAY_KEEP_RECENT: usize = 8;
/// …and only decay tool results larger than this (chars).
const DECAY_MIN_CHARS: usize = 1_500;

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

/// Stage 1 of compaction: "decay" older oversized tool results into short
/// placeholders (the agent can always re-run the tool). Returns how many
/// messages were decayed.
pub fn decay_tool_results(messages: &mut [ChatMessage]) -> usize {
    let cutoff = messages.len().saturating_sub(DECAY_KEEP_RECENT);
    let mut decayed = 0usize;
    for m in messages.iter_mut().take(cutoff) {
        if m.role != "tool" {
            continue;
        }
        let chars = m.content.chars().count();
        if chars <= DECAY_MIN_CHARS {
            continue;
        }
        let head: String = m.content.chars().take(160).collect();
        m.content = format!(
            "[较早的工具结果已省略（原 {chars} 字符）。如仍需要该内容请重新调用相应工具。\
             开头片段：{head}…]"
        );
        decayed += 1;
    }
    decayed
}

/// What a compaction pass actually did (None ⇒ context was under budget).
pub struct CompactionReport {
    /// Older tool results replaced with placeholders.
    pub decayed: usize,
    /// LLM summary that replaced the oldest middle turns (if any).
    pub summary: Option<String>,
    pub before_tokens: usize,
    pub after_tokens: usize,
}

/// Compaction pipeline: tool-result decay first, then LLM summary of the
/// oldest middle turns, hard truncation as the last resort.
pub async fn compact_with_summary(
    providers: &ProviderExecutionService,
    model: &str,
    messages: &mut Vec<ChatMessage>,
    budget_tokens: usize,
) -> Option<CompactionReport> {
    let before_tokens = messages_tokens(messages);
    if before_tokens <= budget_tokens {
        return None;
    }

    // Stage 1: decay older oversized tool results (cheapest, most precise).
    let decayed = decay_tool_results(messages);
    if messages_tokens(messages) <= budget_tokens {
        return Some(CompactionReport {
            decayed,
            summary: None,
            before_tokens,
            after_tokens: messages_tokens(messages),
        });
    }

    let mut summary_text: Option<String> = None;
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
                ChatMessage::system(crate::prompts::COMPACTION_SYSTEM_PROMPT),
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
                let text = resp.text.trim().to_string();
                let summary = ChatMessage::system(format!("（早前对话的压缩摘要）\n{text}"));
                messages.splice(1..cut, [summary]);
                summary_text = Some(text);
            }
        }
    }
    // Whatever happened above, guarantee we end up under budget.
    compact(messages, budget_tokens);
    Some(CompactionReport {
        decayed,
        summary: summary_text,
        before_tokens,
        after_tokens: messages_tokens(messages),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::models::ToolCall;

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
            messages.push(msg(
                "user",
                &format!("message number {i} {}", "x".repeat(400)),
            ));
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

    #[test]
    fn decay_replaces_old_large_tool_results_only() {
        let mut messages = vec![msg("system", "sys")];
        // Old large tool result (will decay).
        messages.push(msg("tool", &"x".repeat(5_000)));
        // Old small tool result (kept).
        messages.push(msg("tool", "short result"));
        // Filler so the large one falls outside the recent window.
        for i in 0..10 {
            messages.push(msg("user", &format!("m{i}")));
        }
        // Recent large tool result (kept — inside the recent window).
        messages.push(msg("tool", &"y".repeat(5_000)));

        let decayed = decay_tool_results(&mut messages);
        assert_eq!(decayed, 1);
        assert!(messages[1].content.contains("已省略"));
        assert_eq!(messages[2].content, "short result");
        assert!(messages.last().unwrap().content.len() >= 5_000);
    }
}
