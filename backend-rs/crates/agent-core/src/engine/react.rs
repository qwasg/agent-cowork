//! Composer chat entry point + the core turn-based tool loop.
//!
//! One user input drives one [`Turn`]: every provider response and tool
//! call/result is recorded as a [`TurnItem`] through the unified
//! [`Runtime::record_item`] path (single source for EventBus emission), and a
//! successful turn is appended to the session rollout as wire-shape messages.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use agent_protocol::events::EventDraft;
use agent_protocol::models::{new_id, now_ts, AgentRun, ChatMessage};
use agent_protocol::{ApiError, ApiResult};
use agent_providers::types::{ProviderRequest, StreamEvent};

use super::control::RunControl;
use super::turn::{Turn, TurnItem};
use super::Runtime;

/// Result of one agent turn: the final assistant text plus the full item
/// stream (for rollout persistence / inspection).
pub struct TurnOutcome {
    pub text: String,
    pub turn: Turn,
}

impl Runtime {
    pub async fn run_composer_chat(
        self: &Arc<Self>,
        session_id: &str,
        user_input: &str,
        composer_mode: &str,
        context_window: Option<&serde_json::Value>,
    ) -> ApiResult<serde_json::Value> {
        let session = self.sessions.get(session_id)?;
        let model = session
            .selected_model_id
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let mut run = AgentRun {
            id: new_id("run"),
            session_id: session_id.to_string(),
            plan_id: None,
            trigger: "composer_chat".to_string(),
            status: "running".to_string(),
            active_todo_ids: vec![],
            completed_todo_ids: vec![],
            failed_todo_ids: vec![],
            parallel_limit: self.parallel_limit as i64,
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        self.save_run(&run).await;
        self.sessions
            .set_active_run(session_id, Some(run.id.clone()), None);
        let control = self.register_run(&run.id);

        self.emit(
            session_id,
            "composer.user.message",
            "agent",
            json!({ "text": user_input, "composerMode": composer_mode, "runId": run.id }),
            Some(run.id.clone()),
        );
        self.emit(
            session_id,
            "agent.started",
            "agent",
            json!({ "runId": run.id, "model": model }),
            Some(run.id.clone()),
        );

        let allowed = self.allowed_tools(session_id, composer_mode);
        let profile = self.session_profile(session_id);
        let mut prompt_tools = allowed.clone();
        if !allowed.is_empty() && profile.wants_todo_tools() {
            if composer_mode == "plan" {
                prompt_tools.push("plan_write".to_string());
            } else {
                prompt_tools.push("todo_write".to_string());
                prompt_tools.push("todo_update".to_string());
            }
        }
        // #region agent log
        {
            use std::io::Write;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let has_write = allowed.iter().any(|t| {
                matches!(
                    t.as_str(),
                    "write_file" | "run_command" | "str_replace_edit" | "apply_patch"
                )
            });
            let line = serde_json::json!({
                "sessionId": "f79bb5",
                "hypothesisId": "H4",
                "location": "react.rs:run_composer_chat",
                "message": "composer chat runtime",
                "data": {
                    "sessionId": session_id,
                    "composerMode": composer_mode,
                    "allowedToolCount": allowed.len(),
                    "promptToolCount": prompt_tools.len(),
                    "hasWriteTools": has_write,
                    "runtimeTools": prompt_tools.iter().filter(|t| matches!(t.as_str(), "plan_write" | "todo_write" | "todo_update")).cloned().collect::<Vec<_>>(),
                    "allowedToolsSample": allowed.iter().take(8).cloned().collect::<Vec<_>>(),
                },
                "timestamp": ts,
                "runId": "post-fix",
            });
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(r"h:\agent-debug-frontend-backend-copy-20260530\debug-f79bb5.log")
            {
                let _ = writeln!(f, "{line}");
            }
        }
        // #endregion
        let system = self.build_main_system_prompt(
            session_id,
            composer_mode,
            &prompt_tools,
            context_window,
            Some(user_input),
        );
        let mut messages = vec![ChatMessage::system(system)];
        messages.extend(self.sessions.history(session_id));
        messages.push(ChatMessage::user(user_input));

        let result = self
            .run_react(
                session_id,
                &run.id,
                &model,
                messages,
                &allowed,
                control.clone(),
                0,
                Some(composer_mode),
            )
            .await;

        self.unregister_run(&run.id);
        match result {
            Ok(outcome) => {
                run.status = "completed".to_string();
                run.updated_at = now_ts();
                self.save_run(&run).await;
                // Persist the full turn (assistant tool calls + results
                // included) to the session rollout.
                self.sessions.append_turn(
                    session_id,
                    &outcome.turn.id,
                    ChatMessage::user(user_input),
                    outcome.turn.to_messages(),
                );
                self.emit(
                    session_id,
                    "agent.completed",
                    "agent",
                    json!({ "runId": run.id, "text": outcome.text }),
                    Some(run.id.clone()),
                );
                self.hooks
                    .turn_end(session_id, &run.id, &outcome.text)
                    .await;
                self.sessions.set_active_run(session_id, None, None);
                Ok(json!({
                    "message": { "text": outcome.text },
                    "run": { "id": run.id, "status": "completed" },
                    "composerMode": composer_mode,
                }))
            }
            Err(e) => {
                run.status = if e.code == "RUN_CANCELLED" {
                    "cancelled"
                } else {
                    "failed"
                }
                .to_string();
                run.updated_at = now_ts();
                self.save_run(&run).await;
                self.emit(
                    session_id,
                    "agent.failed",
                    "agent",
                    json!({ "runId": run.id, "error": e.message }),
                    Some(run.id.clone()),
                );
                self.sessions.set_active_run(session_id, None, None);
                Err(e)
            }
        }
    }

    /// The core tool loop. `depth` 0 is the main agent loop; subagents run at
    /// `depth + 1` up to the configured `subagent_max_depth`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_react(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        model: &str,
        messages: Vec<ChatMessage>,
        allowed: &[String],
        control: Arc<RunControl>,
        depth: usize,
        composer_mode: Option<&str>,
    ) -> ApiResult<TurnOutcome> {
        let steps = self.tool_loop_steps;
        self.run_react_bounded(
            session_id,
            run_id,
            model,
            messages,
            allowed,
            control,
            depth,
            steps,
            composer_mode,
        )
        .await
    }

    /// [`Self::run_react`] with an explicit step cap (subagent resource
    /// boundary; `0` means the default of 64).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_react_bounded(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        model: &str,
        mut messages: Vec<ChatMessage>,
        allowed: &[String],
        control: Arc<RunControl>,
        depth: usize,
        step_cap: usize,
        composer_mode: Option<&str>,
    ) -> ApiResult<TurnOutcome> {
        let mut turn = Turn::new(session_id, run_id);
        let mut specs = self.tools.specs(allowed);
        // Runtime tools are gated by the session's agent profile at the main
        // loop (depth 0); nested subagent loops (depth > 0) keep the historical
        // behavior so a profile can't strip a subagent's declared tools.
        let main_loop = depth == 0;
        let profile = self.session_profile(session_id);
        if depth < self.subagent_max_depth
            && !allowed.is_empty()
            && (!main_loop || profile.wants_task_tool())
        {
            specs.push(self.task_tool_spec());
        }
        if main_loop && !allowed.is_empty() && profile.wants_todo_tools() {
            if composer_mode == Some("plan") {
                specs.push(super::todos::plan_write_tool_spec());
            } else {
                specs.push(super::todos::todo_write_tool_spec());
                specs.push(super::todos::todo_update_tool_spec());
            }
        }
        // Structured edit tools ride along whenever writes are permitted; the
        // coding profile owns code edits (other profiles write whole files).
        let edits_enabled = if main_loop {
            profile.wants_edit_tools()
        } else {
            true
        };
        if edits_enabled && allowed.iter().any(|n| n == "write_file") {
            specs.push(super::edits::str_replace_edit_tool_spec());
            specs.push(super::edits::apply_patch_tool_spec());
        }
        // Memory tools (main loop only) let the agent build durable knowledge.
        if main_loop && !allowed.is_empty() && profile.wants_memory_tools() {
            specs.push(super::memory::memory_write_tool_spec());
            specs.push(super::memory::memory_search_tool_spec());
            specs.push(super::memory::memory_delete_tool_spec());
        }
        // MCP demo tools (`mcp__demo__*`) join the loop when the demo server
        // binary is available (parity with the Python MCP injection).
        if !allowed.is_empty() {
            specs.extend(self.mcp.tool_specs());
        }
        let mut repeated: HashMap<String, usize> = HashMap::new();
        // Empty-progress guard: consecutive steps whose tool calls all failed.
        let mut no_progress_steps = 0usize;
        let max_steps = if step_cap == 0 { 64 } else { step_cap };

        for step in 0..max_steps {
            if control.cancel.is_cancelled() {
                return Err(ApiError::new("RUN_CANCELLED", "run cancelled"));
            }
            // Lightweight progress signal for the UI (not persisted).
            self.bus.emit_ephemeral(
                EventDraft::new(session_id, "agent.step", "agent")
                    .payload(json!({ "step": step + 1, "maxSteps": max_steps, "runId": run_id }))
                    .correlation(Some(run_id.to_string())),
            );
            // Nested loops additionally surface `subagent.progress` so the
            // frontend's subagent card can show step-level activity.
            if depth > 0 {
                self.bus.emit_ephemeral(
                    EventDraft::new(session_id, "subagent.progress", "subagent")
                        .payload(json!({
                            "subRunId": run_id,
                            "step": step + 1,
                            "maxSteps": max_steps,
                        }))
                        .correlation(Some(run_id.to_string())),
                );
            }
            // Honor pause: emit a single paused/resumed pair around the wait
            // (previously `agent.paused` was re-emitted on every wakeup).
            if control.paused.load(std::sync::atomic::Ordering::SeqCst)
                && !control.cancel.is_cancelled()
            {
                self.emit(
                    session_id,
                    "agent.paused",
                    "agent",
                    json!({ "runId": run_id }),
                    Some(run_id.to_string()),
                );
                while control.paused.load(std::sync::atomic::Ordering::SeqCst)
                    && !control.cancel.is_cancelled()
                {
                    control.resume.notified().await;
                }
                if !control.cancel.is_cancelled() {
                    self.emit(
                        session_id,
                        "agent.resumed",
                        "agent",
                        json!({ "runId": run_id }),
                        Some(run_id.to_string()),
                    );
                }
            }

            // Steering: inject any user messages queued mid-run before the
            // next LLM step.
            for text in control.drain_steering() {
                messages.push(ChatMessage::user(text.clone()));
                self.record_item(&mut turn, TurnItem::SteeredUser { text });
            }

            // Context compaction guard (token budget): tool-result decay →
            // LLM summary → hard truncation. Compaction is transparent to the
            // model; the frontend learns about it via `agent.context.compacted`
            // and the rollout gets a Compaction marker for replay fidelity.
            if let Some(report) = crate::context::compact_with_summary(
                &self.providers,
                model,
                &mut messages,
                self.context_budget_tokens,
            )
            .await
            {
                if let Some(summary) = &report.summary {
                    self.sessions.note_compaction(session_id, summary);
                }
                self.emit(
                    session_id,
                    "agent.context.compacted",
                    "agent",
                    json!({
                        "runId": run_id,
                        "decayedToolResults": report.decayed,
                        "summarized": report.summary.is_some(),
                        "beforeTokens": report.before_tokens,
                        "afterTokens": report.after_tokens,
                    }),
                    Some(run_id.to_string()),
                );
            }

            let req = ProviderRequest {
                model: model.to_string(),
                messages: messages.clone(),
                tools: specs.clone(),
                temperature: Some(0.7),
                stream: self.stream,
                max_tokens: None,
            };

            // Streaming sink → ephemeral delta events.
            let bus = self.bus.clone();
            let sid = session_id.to_string();
            let rid = run_id.to_string();
            let sink = move |ev: StreamEvent| match ev {
                StreamEvent::Text(t) => {
                    bus.emit_ephemeral(
                        EventDraft::new(&sid, "agent.token.stream.delta", "agent")
                            .payload(json!({ "delta": t, "runId": rid }))
                            .correlation(Some(rid.clone())),
                    );
                }
                StreamEvent::Reasoning(r) => {
                    bus.emit_ephemeral(
                        EventDraft::new(&sid, "agent.reasoning.delta", "agent")
                            .payload(json!({ "delta": r, "runId": rid }))
                            .correlation(Some(rid.clone())),
                    );
                }
                StreamEvent::ToolCall {
                    index,
                    id,
                    name,
                    arguments,
                } => {
                    // Incremental tool-call arguments: the UI can render the
                    // arguments JSON as the model generates it.
                    bus.emit_ephemeral(
                        EventDraft::new(&sid, "agent.tool.args.delta", "agent")
                            .payload(json!({
                                "runId": rid,
                                "index": index,
                                "toolCallId": id,
                                "name": name,
                                "delta": arguments,
                            }))
                            .correlation(Some(rid.clone())),
                    );
                }
                StreamEvent::Reset => {
                    // A streaming attempt failed mid-flight and is being
                    // retried; the UI should discard buffered deltas.
                    bus.emit_ephemeral(
                        EventDraft::new(&sid, "agent.stream.reset", "agent")
                            .payload(json!({ "runId": rid }))
                            .correlation(Some(rid.clone())),
                    );
                }
                _ => {}
            };

            let resp = self
                .providers
                .execute(&req, &sink, Some(&control.cancel))
                .await?;

            // Per-call usage accounting (persisted; aggregated by
            // `GET /runs/{id}/metrics`).
            if resp.usage.total_tokens > 0 {
                self.emit(
                    session_id,
                    "agent.usage",
                    "provider",
                    json!({
                        "runId": run_id,
                        "provider": resp.provider,
                        "model": resp.model,
                        "promptTokens": resp.usage.prompt_tokens,
                        "completionTokens": resp.usage.completion_tokens,
                        "totalTokens": resp.usage.total_tokens,
                        "cacheReadTokens": resp.usage.cache_read_tokens,
                    }),
                    Some(run_id.to_string()),
                );
            }

            if resp.degraded {
                self.emit(
                    session_id,
                    "provider.degraded",
                    "provider",
                    json!({
                        "runId": run_id,
                        "provider": resp.provider,
                        "message": "所有真实模型渠道调用失败，已降级为离线模拟回复。",
                    }),
                    Some(run_id.to_string()),
                );
            }

            if let Some(r) = &resp.reasoning {
                if !r.is_empty() && resp.tool_calls.is_empty() {
                    self.record_item(&mut turn, TurnItem::Reasoning { text: r.clone() });
                }
            }

            if resp.tool_calls.is_empty() {
                let text = resp.text;
                self.record_item(
                    &mut turn,
                    TurnItem::AssistantText {
                        text: text.clone(),
                        provider: resp.provider,
                        degraded: resp.degraded,
                    },
                );
                // A steer that landed while the final answer was generating
                // keeps the loop alive instead of dropping the message.
                let steered = control.drain_steering();
                if !steered.is_empty() {
                    messages.push(ChatMessage::assistant(text));
                    for t in steered {
                        messages.push(ChatMessage::user(t.clone()));
                        self.record_item(&mut turn, TurnItem::SteeredUser { text: t });
                    }
                    continue;
                }
                return Ok(TurnOutcome { text, turn });
            }

            // Append the assistant turn carrying the tool calls.
            messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: resp.text.clone(),
                reasoning: None,
                tool_calls: resp.tool_calls.clone(),
                tool_call_id: None,
                name: None,
            });
            // Record the intermediate assistant text (if any) + every tool
            // call before execution (the `agent.tool.invoked` events).
            if !resp.text.trim().is_empty() {
                turn.items.push(TurnItem::AssistantText {
                    text: resp.text.clone(),
                    provider: resp.provider.clone(),
                    degraded: resp.degraded,
                });
            }
            for call in &resp.tool_calls {
                self.record_item(
                    &mut turn,
                    TurnItem::ToolCall {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    },
                );
            }

            // Same-turn read-only tools (read/grep/list/web/...) and `task`
            // subagents fan out concurrently (bounded by `parallel_limit`);
            // write tools stay sequential so filesystem effects keep order.
            let mut parallel: HashMap<usize, (ApiResult<String>, u64)> = HashMap::new();
            let parallelizable: Vec<usize> = resp
                .tool_calls
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    (c.name == "task" && depth < self.subagent_max_depth)
                        || self.tools.is_read_only(&c.name)
                })
                .map(|(idx, _)| idx)
                .collect();
            if parallelizable.len() >= 2 {
                use futures_util::StreamExt;
                let tool_calls = &resp.tool_calls;
                let mut stream =
                    futures_util::stream::iter(parallelizable.into_iter().map(|idx| {
                        let call = &tool_calls[idx];
                        let ctrl = control.clone();
                        async move {
                            let started = std::time::Instant::now();
                            let res = self
                                .dispatch_tool(session_id, run_id, call, &ctrl, depth)
                                .await;
                            (idx, res, started.elapsed().as_millis() as u64)
                        }
                    }))
                    .buffer_unordered(self.parallel_limit.max(1));
                while let Some((idx, res, elapsed)) = stream.next().await {
                    parallel.insert(idx, (res, elapsed));
                }
            }

            let mut step_failures = 0usize;

            for (idx, call) in resp.tool_calls.iter().enumerate() {
                // Loop guard keyed by (tool, arguments) fingerprint: only the
                // *identical* repeated call counts. Keying by tool name alone
                // falsely killed exploration (e.g. reading 9 different files).
                let key = repeated_call_fingerprint(&call.name, &call.arguments);
                let count = repeated.entry(key).or_insert(0);
                *count += 1;
                if *count > self.repeated_tool_limit {
                    let msg = format!("tool '{}' invoked too many times; stopping", call.name);
                    self.record_item(
                        &mut turn,
                        TurnItem::ToolResult {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            output: msg,
                            is_error: true,
                            duration_ms: 0,
                        },
                    );
                    let text = format!("已停止：检测到重复工具调用（{}）。", call.name);
                    self.record_item(
                        &mut turn,
                        TurnItem::AssistantText {
                            text: text.clone(),
                            provider: String::new(),
                            degraded: false,
                        },
                    );
                    return Ok(TurnOutcome { text, turn });
                }
                let (result, duration_ms) = match parallel.remove(&idx) {
                    Some(res) => res,
                    None => {
                        let started = std::time::Instant::now();
                        let res = self
                            .dispatch_tool(session_id, run_id, call, &control, depth)
                            .await;
                        (res, started.elapsed().as_millis() as u64)
                    }
                };
                let (content, is_error) = match result {
                    Ok(out) => (out, false),
                    Err(e) => (format!("ERROR[{}]: {}", e.code, e.message), true),
                };
                if is_error {
                    step_failures += 1;
                }
                self.record_item(
                    &mut turn,
                    TurnItem::ToolResult {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        output: content.clone(),
                        is_error,
                        duration_ms,
                    },
                );
                messages.push(ChatMessage::tool_result(
                    call.id.clone(),
                    call.name.clone(),
                    content,
                ));
            }

            // Empty-progress guard: a step where *every* tool call failed is
            // no progress. After `empty_progress_limit` such steps inject a
            // strategy-change reminder; one more stops the loop.
            if step_failures == resp.tool_calls.len() {
                no_progress_steps += 1;
            } else {
                no_progress_steps = 0;
            }
            if no_progress_steps == self.empty_progress_limit {
                messages.push(ChatMessage::user(
                    "[系统提醒] 最近几步的工具调用全部失败，没有任何进展。请改变策略：\
检查参数是否正确、换一种方式完成目标，或基于已有信息直接给出结论。",
                ));
            } else if no_progress_steps > self.empty_progress_limit {
                let text = "已停止：连续多步无进展（工具调用全部失败）。".to_string();
                self.record_item(
                    &mut turn,
                    TurnItem::AssistantText {
                        text: text.clone(),
                        provider: String::new(),
                        degraded: false,
                    },
                );
                return Ok(TurnOutcome { text, turn });
            }
        }
        let text = "（已达到最大工具循环步数，结束本轮。）".to_string();
        self.record_item(
            &mut turn,
            TurnItem::AssistantText {
                text: text.clone(),
                provider: String::new(),
                degraded: false,
            },
        );
        Ok(TurnOutcome { text, turn })
    }
}

/// Fingerprint for the repeated-tool-call loop guard: identical name + args.
fn repeated_call_fingerprint(name: &str, arguments: &str) -> String {
    format!("{name}|{}", arguments.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_distinguishes_arguments() {
        let a = repeated_call_fingerprint("read_file", r#"{"path":"a.rs"}"#);
        let b = repeated_call_fingerprint("read_file", r#"{"path":"b.rs"}"#);
        let a2 = repeated_call_fingerprint("read_file", r#"{"path":"a.rs"} "#);
        assert_ne!(a, b, "different args must not share a fingerprint");
        assert_eq!(a, a2, "trailing whitespace is normalized");
    }
}
