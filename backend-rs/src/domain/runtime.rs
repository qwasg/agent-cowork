//! Agent runtime: the ReAct tool loop, streaming, run lifecycle, cancellation
//! and the plan DAG executor (port of `runtime.py`).
//!
//! Optimizations vs. the Python version:
//! - True parallel DAG execution with `tokio::JoinSet` (no GIL).
//! - `CancellationToken` for clean cancel/timeout; `Notify` for pause/resume.
//! - Streaming token deltas are emitted as ephemeral (non-persisted) events.
//!
//! Additions vs. the first Rust port:
//! - Cancellation propagates into in-flight LLM calls.
//! - `task` tool: delegates a subtask to a nested ReAct loop (subagent) and
//!   feeds a summarized result back to the main loop.
//! - Interactive permission approval (`auto` mode) for write tools.
//! - Plan execution reports an accurate outcome (cancelled / partial failure),
//!   updates `AgentRun` todo metrics, and survives task panics.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::contracts::events::EventDraft;
use crate::contracts::models::{new_id, now_ts, AgentRun, ChatMessage, ToolCall};
use crate::contracts::{ApiError, ApiResult};
use crate::domain::permission::PermissionService;
use crate::domain::session::SessionService;
use crate::domain::todo::TodoEngine;
use crate::infra::store::T_RUNS;
use crate::infra::{EventBus, Store};
use crate::provider::tokens::estimate_tokens;
use crate::provider::types::{noop_sink, ProviderRequest, StreamEvent, ToolSpec};
use crate::provider::ProviderExecutionService;
use crate::tools::{ToolContext, ToolRegistry, WebConfig};

pub struct RunControl {
    pub cancel: CancellationToken,
    pub paused: AtomicBool,
    pub resume: Notify,
}

/// Result of executing a plan DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOutcome {
    Completed,
    Cancelled,
    PartialFailure,
}

pub struct Runtime {
    pub providers: Arc<ProviderExecutionService>,
    pub tools: Arc<ToolRegistry>,
    pub bus: Arc<EventBus>,
    pub sessions: Arc<SessionService>,
    pub permissions: Arc<PermissionService>,
    pub todos: Arc<TodoEngine>,
    pub store: Arc<Store>,
    pub web: WebConfig,
    pub skill_dirs: Vec<PathBuf>,
    pub default_workspace_root: PathBuf,
    pub parallel_limit: usize,
    pub edit_parallel: usize,
    pub tool_loop_steps: usize,
    pub repeated_tool_limit: usize,
    pub stream: bool,
    pub context_budget_tokens: usize,
    pub approval_timeout_secs: u64,
    controls: Mutex<HashMap<String, Arc<RunControl>>>,
}

impl Runtime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: &Config,
        providers: Arc<ProviderExecutionService>,
        tools: Arc<ToolRegistry>,
        bus: Arc<EventBus>,
        sessions: Arc<SessionService>,
        permissions: Arc<PermissionService>,
        todos: Arc<TodoEngine>,
        store: Arc<Store>,
        skill_dirs: Vec<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Runtime {
            providers,
            tools,
            bus,
            sessions,
            permissions,
            todos,
            store,
            web: WebConfig {
                api_key: cfg.tavily_api_key.clone(),
                base_url: cfg.tavily_base_url.clone(),
                fetch_max_chars: cfg.web_fetch_max_chars,
                allow_private: cfg.web_fetch_allow_private,
            },
            skill_dirs,
            default_workspace_root: cfg.workspace_root.clone(),
            parallel_limit: cfg.parallel_limit,
            edit_parallel: cfg.edit_parallel,
            tool_loop_steps: cfg.tool_loop_steps,
            repeated_tool_limit: cfg.repeated_tool_limit,
            stream: cfg.stream,
            context_budget_tokens: cfg.context_budget_tokens,
            approval_timeout_secs: cfg.approval_timeout_secs,
            controls: Mutex::new(HashMap::new()),
        })
    }

    pub fn workspace_root(&self) -> PathBuf {
        self.store
            .kv_get("workspace_root")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_workspace_root.clone())
    }

    fn tool_ctx(&self, session_id: &str, run_id: &str) -> ToolContext {
        ToolContext {
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            workspace_root: self.workspace_root(),
            web: self.web.clone(),
            skill_dirs: self.skill_dirs.clone(),
        }
    }

    fn emit(
        &self,
        session_id: &str,
        etype: &str,
        domain: &str,
        payload: serde_json::Value,
        corr: Option<String>,
    ) {
        self.bus.emit(
            EventDraft::new(session_id, etype, domain)
                .payload(payload)
                .correlation(corr),
        );
    }

    fn emit_actor(
        &self,
        session_id: &str,
        etype: &str,
        domain: &str,
        actor: &str,
        payload: serde_json::Value,
        corr: Option<String>,
    ) {
        self.bus.emit(
            EventDraft::new(session_id, etype, domain)
                .actor(actor)
                .payload(payload)
                .correlation(corr),
        );
    }

    // ---------------- run control ----------------

    fn register_run(&self, run_id: &str) -> Arc<RunControl> {
        let control = Arc::new(RunControl {
            cancel: CancellationToken::new(),
            paused: AtomicBool::new(false),
            resume: Notify::new(),
        });
        self.controls
            .lock()
            .unwrap()
            .insert(run_id.to_string(), control.clone());
        control
    }

    fn control(&self, run_id: &str) -> Option<Arc<RunControl>> {
        self.controls.lock().unwrap().get(run_id).cloned()
    }

    pub fn pause_run(&self, run_id: &str) -> bool {
        if let Some(c) = self.control(run_id) {
            c.paused.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn resume_run(&self, run_id: &str) -> bool {
        if let Some(c) = self.control(run_id) {
            c.paused.store(false, Ordering::SeqCst);
            c.resume.notify_waiters();
            true
        } else {
            false
        }
    }

    pub fn cancel_run(&self, run_id: &str) -> bool {
        if let Some(c) = self.control(run_id) {
            c.cancel.cancel();
            c.resume.notify_waiters();
            true
        } else {
            false
        }
    }

    /// Cancel every active run (graceful shutdown).
    pub fn cancel_all(&self) {
        let controls = self.controls.lock().unwrap();
        for c in controls.values() {
            c.cancel.cancel();
            c.resume.notify_waiters();
        }
    }

    pub fn save_run(&self, run: &AgentRun) {
        let _ = self.store.put(T_RUNS, &run.id, run);
    }

    pub fn get_run(&self, run_id: &str) -> ApiResult<AgentRun> {
        self.store
            .get::<AgentRun>(T_RUNS, run_id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::run_not_found(run_id))
    }

    // ---------------- composer chat ----------------

    pub async fn run_composer_chat(
        self: &Arc<Self>,
        session_id: &str,
        user_input: &str,
        composer_mode: &str,
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
        self.save_run(&run);
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
        let system = composer_system_prompt(composer_mode, &allowed);
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
            )
            .await;

        self.controls.lock().unwrap().remove(&run.id);
        match result {
            Ok(text) => {
                run.status = "completed".to_string();
                run.updated_at = now_ts();
                self.save_run(&run);
                self.sessions.remember_turn(
                    session_id,
                    ChatMessage::user(user_input),
                    ChatMessage::assistant(text.clone()),
                );
                self.emit(
                    session_id,
                    "agent.completed",
                    "agent",
                    json!({ "runId": run.id, "text": text }),
                    Some(run.id.clone()),
                );
                self.sessions.set_active_run(session_id, None, None);
                Ok(json!({
                    "message": { "text": text },
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
                self.save_run(&run);
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

    fn allowed_tools(&self, session_id: &str, composer_mode: &str) -> Vec<String> {
        if composer_mode == "ask" {
            return vec![];
        }
        let session = self.sessions.get(session_id).ok();
        let web_on = session.map(|s| s.web_search_enabled).unwrap_or(false);
        self.tools
            .names()
            .into_iter()
            .filter(|n| web_on || (n != "web_search" && n != "web_fetch"))
            .filter(|n| self.permissions.can_use_tool(session_id, n))
            .collect()
    }

    // ---------------- the ReAct loop ----------------

    /// `depth` 0 is the main agent loop; subagents run at depth 1 (and cannot
    /// spawn further subagents).
    #[allow(clippy::too_many_arguments)]
    async fn run_react(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        model: &str,
        mut messages: Vec<ChatMessage>,
        allowed: &[String],
        control: Arc<RunControl>,
        depth: usize,
    ) -> ApiResult<String> {
        let mut specs = self.tools.specs(allowed);
        if depth == 0 && !allowed.is_empty() {
            specs.push(task_tool_spec());
            specs.push(todo_write_tool_spec());
            specs.push(todo_update_tool_spec());
        }
        let mut repeated: HashMap<String, usize> = HashMap::new();
        let max_steps = if self.tool_loop_steps == 0 {
            64
        } else {
            self.tool_loop_steps
        };

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
            // Honor pause: emit a single paused/resumed pair around the wait
            // (previously `agent.paused` was re-emitted on every wakeup).
            if control.paused.load(Ordering::SeqCst) && !control.cancel.is_cancelled() {
                self.emit(
                    session_id,
                    "agent.paused",
                    "agent",
                    json!({ "runId": run_id }),
                    Some(run_id.to_string()),
                );
                while control.paused.load(Ordering::SeqCst) && !control.cancel.is_cancelled() {
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

            // Context compaction guard (token budget): LLM summary first,
            // hard truncation as fallback.
            crate::domain::context::compact_with_summary(
                &self.providers,
                model,
                &mut messages,
                self.context_budget_tokens,
            )
            .await;

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

            if resp.tool_calls.is_empty() {
                if let Some(r) = &resp.reasoning {
                    if !r.is_empty() {
                        self.emit(
                            session_id,
                            "agent.reasoning",
                            "agent",
                            json!({ "text": r, "runId": run_id }),
                            Some(run_id.to_string()),
                        );
                    }
                }
                self.emit(
                    session_id,
                    "agent.message",
                    "agent",
                    json!({
                        "text": resp.text,
                        "runId": run_id,
                        "provider": resp.provider,
                        "degraded": resp.degraded,
                    }),
                    Some(run_id.to_string()),
                );
                return Ok(resp.text);
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

            for call in &resp.tool_calls {
                // Loop guard keyed by (tool, arguments) fingerprint: only the
                // *identical* repeated call counts. Keying by tool name alone
                // falsely killed exploration (e.g. reading 9 different files).
                let key = repeated_call_fingerprint(&call.name, &call.arguments);
                let count = repeated.entry(key).or_insert(0);
                *count += 1;
                if *count > self.repeated_tool_limit {
                    let msg = format!("tool '{}' invoked too many times; stopping", call.name);
                    self.emit(
                        session_id,
                        "agent.tool.failed",
                        "tool",
                        json!({ "name": call.name, "error": msg, "runId": run_id }),
                        Some(run_id.to_string()),
                    );
                    return Ok(format!("已停止：检测到重复工具调用（{}）。", call.name));
                }
                let result = self
                    .dispatch_tool(session_id, run_id, call, &control, depth)
                    .await;
                let content = match result {
                    Ok(out) => out,
                    Err(e) => format!("ERROR[{}]: {}", e.code, e.message),
                };
                messages.push(ChatMessage::tool_result(
                    call.id.clone(),
                    call.name.clone(),
                    content,
                ));
            }
        }
        Ok("（已达到最大工具循环步数，结束本轮。）".to_string())
    }

    async fn dispatch_tool(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        call: &ToolCall,
        control: &Arc<RunControl>,
        depth: usize,
    ) -> ApiResult<String> {
        let args: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_else(|e| {
            tracing::warn!("tool '{}': invalid JSON arguments ({e}); using {{}}", call.name);
            json!({})
        });
        self.emit(
            session_id,
            "agent.tool.invoked",
            "tool",
            json!({ "name": call.name, "args": args, "runId": run_id }),
            Some(run_id.to_string()),
        );

        if !self.permissions.can_use_tool(session_id, &call.name) {
            let err = ApiError::new(
                "TOOL_FORBIDDEN",
                format!("tool '{}' not permitted in current mode", call.name),
            );
            self.emit(
                session_id,
                "agent.tool.failed",
                "tool",
                json!({ "name": call.name, "error": err.message, "runId": run_id }),
                Some(run_id.to_string()),
            );
            return Err(err);
        }

        // Interactive approval for write tools in `auto` mode.
        if self.permissions.requires_approval(session_id, &call.name) {
            let approved = self
                .await_approval(session_id, run_id, &call.name, &args, control)
                .await;
            if !approved {
                let err = ApiError::new(
                    "TOOL_FORBIDDEN",
                    format!("tool '{}' was denied by the user (or timed out)", call.name),
                );
                self.emit(
                    session_id,
                    "agent.tool.failed",
                    "tool",
                    json!({ "name": call.name, "error": err.message, "runId": run_id }),
                    Some(run_id.to_string()),
                );
                return Err(err);
            }
        }

        // `task` delegates to a nested subagent loop (runtime-handled tool).
        if call.name == "task" {
            if depth > 0 {
                return Err(ApiError::new(
                    "TOOL_FORBIDDEN",
                    "nested subagents are not allowed",
                ));
            }
            return self.run_subagent(session_id, run_id, control, &args).await;
        }

        // `todo_write` / `todo_update` are intercepted by the runtime (not
        // registry tools): they manage the session checklist.
        if call.name == "todo_write" || call.name == "write_todos" {
            return Ok(self.handle_write_todos(session_id, run_id, &args));
        }
        if call.name == "todo_update" {
            let out = self.handle_update_todo(session_id, run_id, &args);
            if let Err(e) = &out {
                self.emit(
                    session_id,
                    "agent.tool.failed",
                    "tool",
                    json!({ "name": call.name, "error": e.message, "runId": run_id }),
                    Some(run_id.to_string()),
                );
            }
            return out;
        }

        let ctx = self.tool_ctx(session_id, run_id);
        match self.tools.run(&call.name, args, &ctx).await {
            Ok(out) => {
                self.emit(
                    session_id,
                    "agent.tool.completed",
                    "tool",
                    json!({ "name": call.name, "ok": true, "runId": run_id }),
                    Some(run_id.to_string()),
                );
                Ok(out)
            }
            Err(e) => {
                self.emit(
                    session_id,
                    "agent.tool.failed",
                    "tool",
                    json!({ "name": call.name, "error": e.message, "runId": run_id }),
                    Some(run_id.to_string()),
                );
                Err(e)
            }
        }
    }

    /// Emit `permission.requested`, then suspend until the user approves /
    /// denies via REST, the run is cancelled, or the request times out.
    async fn await_approval(
        &self,
        session_id: &str,
        run_id: &str,
        tool: &str,
        args: &serde_json::Value,
        control: &Arc<RunControl>,
    ) -> bool {
        let (request_id, rx) = self.permissions.create_request();
        self.emit(
            session_id,
            "permission.requested",
            "tool",
            json!({
                "requestId": request_id,
                "tool": tool,
                "args": args,
                "runId": run_id,
                "timeoutSecs": self.approval_timeout_secs,
            }),
            Some(run_id.to_string()),
        );
        let timeout = Duration::from_secs(self.approval_timeout_secs);
        let approved = tokio::select! {
            _ = control.cancel.cancelled() => false,
            r = tokio::time::timeout(timeout, rx) => match r {
                Ok(Ok(decision)) => decision,
                _ => false, // timed out or sender dropped
            },
        };
        self.permissions.discard_request(&request_id);
        self.emit(
            session_id,
            "permission.resolved",
            "tool",
            json!({ "requestId": request_id, "tool": tool, "approved": approved, "runId": run_id }),
            Some(run_id.to_string()),
        );
        approved
    }

    // ---------------- subagent (`task` tool) ----------------

    async fn run_subagent(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        control: &Arc<RunControl>,
        args: &serde_json::Value,
    ) -> ApiResult<String> {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if prompt.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "prompt required"));
        }
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("子任务")
            .to_string();

        let sub_run_id = new_id("subrun");
        let actor = format!("subagent:{sub_run_id}");
        self.emit_actor(
            session_id,
            "subagent.started",
            "subagent",
            &actor,
            json!({ "subRunId": sub_run_id, "parentRunId": run_id, "description": description }),
            Some(run_id.to_string()),
        );

        let model = self
            .sessions
            .get(session_id)
            .ok()
            .and_then(|s| s.selected_model_id)
            .unwrap_or_else(|| "default".to_string());
        let allowed = self.allowed_tools(session_id, "build");
        let system = format!(
            "{}\n你是受主代理委派的子代理，只负责完成下述单一任务，完成后给出简明结果。",
            composer_system_prompt("build", &allowed)
        );
        let messages = vec![ChatMessage::system(system), ChatMessage::user(prompt)];

        // Box the recursive async call (depth 1: no further nesting).
        let result = Box::pin(self.run_react(
            session_id,
            &sub_run_id,
            &model,
            messages,
            &allowed,
            control.clone(),
            1,
        ))
        .await;

        match result {
            Ok(text) => {
                let summary = self.summarize_subagent_result(&model, &text).await;
                self.emit_actor(
                    session_id,
                    "subagent.completed",
                    "subagent",
                    &actor,
                    json!({ "subRunId": sub_run_id, "parentRunId": run_id, "summary": summary }),
                    Some(run_id.to_string()),
                );
                Ok(summary)
            }
            Err(e) => {
                self.emit_actor(
                    session_id,
                    "subagent.failed",
                    "subagent",
                    &actor,
                    json!({ "subRunId": sub_run_id, "parentRunId": run_id, "error": e.message }),
                    Some(run_id.to_string()),
                );
                Err(e)
            }
        }
    }

    /// Long subagent outputs are summarized by the LLM before being fed back
    /// to the main loop (mirrors `summary_manager.py`).
    async fn summarize_subagent_result(&self, model: &str, text: &str) -> String {
        const SUMMARY_THRESHOLD_TOKENS: usize = 1_500;
        if estimate_tokens(text) <= SUMMARY_THRESHOLD_TOKENS {
            return text.to_string();
        }
        let req = ProviderRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage::system(
                    "请把下面这段子任务执行结果压缩为要点摘要（保留关键结论、文件路径、数据），直接输出摘要。",
                ),
                ChatMessage::user(text.chars().take(24_000).collect::<String>()),
            ],
            tools: vec![],
            temperature: Some(0.2),
            stream: false,
            max_tokens: Some(1024),
        };
        let sink = noop_sink();
        match self.providers.execute(&req, &sink, None).await {
            Ok(resp) if !resp.degraded && !resp.text.trim().is_empty() => resp.text,
            _ => text.chars().take(8_000).collect(),
        }
    }

    /// `todo_write`: create a batch of todos. Items support `kind`
    /// (explore/edit) and `dependsOn` (indexes into this batch). Returns the
    /// session checklist (with ids) so the model can reference them later via
    /// `todo_update`.
    pub(crate) fn handle_write_todos(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> String {
        let mut created_ids: Vec<String> = Vec::new();
        if let Some(items) = args.get("todos").and_then(|v| v.as_array()) {
            for item in items {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if title.is_empty() {
                    continue;
                }
                let desc = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("edit");
                let deps: Vec<String> = item
                    .get("dependsOn")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_u64().map(|n| n as usize))
                            .filter(|&n| n < created_ids.len())
                            .map(|n| created_ids[n].clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let todo = self.todos.add(session_id, title, desc, kind, deps.clone());
                self.emit(
                    session_id,
                    "todo.created",
                    "todo",
                    json!({ "id": todo.id, "title": todo.title, "kind": todo.kind, "dependencies": deps }),
                    Some(run_id.to_string()),
                );
                created_ids.push(todo.id);
            }
        }
        format!(
            "recorded {} todos\n{}",
            created_ids.len(),
            self.render_todo_checklist(session_id)
        )
    }

    /// `todo_update`: the model marks a todo in_progress / completed / failed
    /// / cancelled as it works through its checklist; mirrored to the UI via
    /// `todo.*` events. Returns the refreshed checklist.
    pub(crate) fn handle_update_todo(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> ApiResult<String> {
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let status_in = args
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let engine_status = match status_in {
            "in_progress" | "running" | "started" => "running",
            "completed" | "done" => "completed",
            "failed" => "failed",
            "cancelled" | "canceled" => "cancelled",
            other => {
                return Err(ApiError::new(
                    "TOOL_INVALID_ARGS",
                    format!("invalid status: {other} (expected in_progress|completed|failed|cancelled)"),
                ))
            }
        };
        let mut todo = self.todos.get(id)?;
        todo.status = engine_status.to_string();
        if engine_status == "running" {
            todo.related_agent_run_id = Some(run_id.to_string());
        }
        if let Some(summary) = args.get("summary").and_then(|v| v.as_str()) {
            todo.summary = Some(summary.chars().take(500).collect());
        }
        todo.updated_at = now_ts();
        self.todos.save(&todo);

        let etype = match engine_status {
            "running" => "todo.started",
            "completed" => "todo.completed",
            "failed" => "todo.failed",
            _ => "todo.updated",
        };
        self.emit(
            session_id,
            etype,
            "todo",
            json!({ "id": todo.id, "title": todo.title, "kind": todo.kind, "status": todo.status }),
            Some(run_id.to_string()),
        );
        Ok(format!(
            "todo '{}' -> {}\n{}",
            todo.title,
            todo.status,
            self.render_todo_checklist(session_id)
        ))
    }

    /// Conclusions (title + summary) of completed todos, newest-capped, used
    /// to hand exploration results to subsequent edit tasks.
    fn completed_todo_context(&self, session_id: &str) -> String {
        const MAX_ITEMS: usize = 8;
        let todos = self.todos.list_by_session(session_id);
        let mut lines: Vec<String> = todos
            .iter()
            .filter(|t| t.status == "completed")
            .filter_map(|t| {
                t.summary.as_ref().map(|s| {
                    format!("- {}（{}）：{}", t.title, t.kind, s.chars().take(500).collect::<String>())
                })
            })
            .collect();
        if lines.len() > MAX_ITEMS {
            lines = lines.split_off(lines.len() - MAX_ITEMS);
        }
        lines.join("\n")
    }

    /// Plain-text checklist of the session's todos, fed back to the model.
    fn render_todo_checklist(&self, session_id: &str) -> String {
        let todos = self.todos.list_by_session(session_id);
        if todos.is_empty() {
            return "(no todos)".to_string();
        }
        let mut out = String::from("当前任务清单：\n");
        for t in todos {
            let mark = match t.status.as_str() {
                "completed" => "[x]",
                "running" => "[~]",
                "failed" => "[!]",
                "cancelled" | "blocked" => "[-]",
                _ => "[ ]",
            };
            out.push_str(&format!("{mark} {} ({}) id={}\n", t.title, t.kind, t.id));
        }
        out
    }

    // ---------------- plan DAG executor ----------------

    pub async fn run_plan(
        self: &Arc<Self>,
        run_id: &str,
        session_id: &str,
        model: &str,
    ) -> ApiResult<PlanOutcome> {
        let mut run = self.get_run(run_id)?;
        let control = self.register_run(run_id);
        let mut any_failed = false;

        let outcome = loop {
            if control.cancel.is_cancelled() {
                break PlanOutcome::Cancelled;
            }
            let ready = self.todos.ready_todos(session_id);
            if ready.is_empty() {
                break if any_failed {
                    PlanOutcome::PartialFailure
                } else {
                    PlanOutcome::Completed
                };
            }
            // `ready_todos` sorts explore first and holds edits back while
            // exploration is unfinished, so a batch is homogeneous in kind.
            // Explore todos fan out wide (read-only, safe to parallelize);
            // edit todos run with bounded parallelism (default 1: serial) so
            // concurrent agents don't fight over the same files.
            let batch_kind = ready[0].kind.clone();
            let batch_limit = if batch_kind == "explore" {
                self.parallel_limit
            } else {
                self.edit_parallel
            };
            let batch: Vec<_> = ready
                .into_iter()
                .filter(|t| t.kind == batch_kind)
                .take(batch_limit)
                .collect();
            let mut batch_ids: HashSet<String> = batch.iter().map(|t| t.id.clone()).collect();

            run.active_todo_ids = batch.iter().map(|t| t.id.clone()).collect();
            run.updated_at = now_ts();
            self.save_run(&run);

            // Conclusions from already-finished todos (exploration results,
            // earlier edits) are handed to the next tasks as context.
            let prior_context = self.completed_todo_context(session_id);

            // True parallel execution of the ready batch via JoinSet.
            let mut set = tokio::task::JoinSet::new();
            for todo in batch {
                let this = self.clone();
                let sid = session_id.to_string();
                let mdl = model.to_string();
                let rid = run_id.to_string();
                let ctrl = control.clone();
                let prior = prior_context.clone();
                if let Ok(mut t) = self.todos.get(&todo.id) {
                    t.status = "running".to_string();
                    t.related_agent_run_id = Some(run_id.to_string());
                    t.updated_at = now_ts();
                    self.todos.save(&t);
                }
                this.emit(
                    &sid,
                    "todo.started",
                    "todo",
                    json!({ "id": todo.id, "title": todo.title, "kind": todo.kind }),
                    Some(rid.clone()),
                );
                set.spawn(async move {
                    let is_explore = todo.kind == "explore";
                    // Explore todos only see read-only tools.
                    let mut allowed = this.allowed_tools(&sid, "build");
                    if is_explore {
                        allowed.retain(|n| {
                            !crate::domain::permission::WRITE_TOOLS.contains(&n.as_str())
                        });
                    }
                    let mut prompt = format!("任务：{}\n说明：{}", todo.title, todo.description);
                    if !prior.is_empty() {
                        prompt.push_str("\n\n已完成的前序任务结论：\n");
                        prompt.push_str(&prior);
                    }
                    if is_explore {
                        prompt.push_str(
                            "\n\n这是一个只读调研任务：只收集信息，不要修改任何文件。\
                             最终回复请输出结论要点（关键发现、相关文件路径、对后续实施的建议），不要描述探索过程。",
                        );
                    }
                    let messages = vec![
                        ChatMessage::system(composer_system_prompt("build", &allowed)),
                        ChatMessage::user(prompt),
                    ];
                    let res = this
                        .run_react(&sid, &rid, &mdl, messages, &allowed, ctrl, 0)
                        .await;
                    (todo.id, todo.title, res)
                });
            }

            while let Some(joined) = set.join_next().await {
                match joined {
                    Ok((todo_id, title, res)) => {
                        batch_ids.remove(&todo_id);
                        match res {
                            Ok(summary) => {
                                if let Ok(mut t) = self.todos.get(&todo_id) {
                                    t.status = "completed".to_string();
                                    t.summary = Some(summary.chars().take(500).collect());
                                    t.updated_at = now_ts();
                                    self.todos.save(&t);
                                }
                                run.completed_todo_ids.push(todo_id.clone());
                                self.emit(
                                    session_id,
                                    "todo.completed",
                                    "todo",
                                    json!({ "id": todo_id, "title": title }),
                                    Some(run_id.to_string()),
                                );
                            }
                            Err(e) => {
                                any_failed = true;
                                if let Ok(mut t) = self.todos.get(&todo_id) {
                                    t.status = "failed".to_string();
                                    t.last_error = Some(e.message.clone());
                                    t.updated_at = now_ts();
                                    self.todos.save(&t);
                                }
                                run.failed_todo_ids.push(todo_id.clone());
                                self.emit(
                                    session_id,
                                    "todo.failed",
                                    "todo",
                                    json!({ "id": todo_id, "error": e.message }),
                                    Some(run_id.to_string()),
                                );
                            }
                        }
                    }
                    Err(join_err) => {
                        // A task panicked or was aborted; the affected todo is
                        // resolved below from the unaccounted batch ids.
                        tracing::warn!("plan task join error: {join_err}");
                    }
                }
            }

            // Any todo not joined back (task panic) must not stay `running`.
            for todo_id in batch_ids {
                any_failed = true;
                if let Ok(mut t) = self.todos.get(&todo_id) {
                    t.status = "failed".to_string();
                    t.last_error = Some("task panicked".to_string());
                    t.updated_at = now_ts();
                    self.todos.save(&t);
                }
                run.failed_todo_ids.push(todo_id.clone());
                self.emit(
                    session_id,
                    "todo.failed",
                    "todo",
                    json!({ "id": todo_id, "error": "task panicked" }),
                    Some(run_id.to_string()),
                );
            }

            run.active_todo_ids.clear();
            run.updated_at = now_ts();
            self.save_run(&run);
        };

        // Todos whose dependencies ended in failure can never become ready;
        // mark them blocked instead of leaving them queued forever.
        for todo in self.todos.blocked_todos(session_id) {
            if let Ok(mut t) = self.todos.get(&todo.id) {
                t.status = "blocked".to_string();
                t.last_error = Some("dependency failed".to_string());
                t.updated_at = now_ts();
                self.todos.save(&t);
            }
            self.emit(
                session_id,
                "todo.failed",
                "todo",
                json!({ "id": todo.id, "title": todo.title, "error": "dependency failed", "status": "blocked" }),
                Some(run_id.to_string()),
            );
        }

        run.active_todo_ids.clear();
        run.updated_at = now_ts();
        self.save_run(&run);
        self.controls.lock().unwrap().remove(run_id);
        Ok(outcome)
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

    #[test]
    fn build_prompt_mentions_todo_workflow() {
        let tools = vec!["read_file".to_string(), "write_file".to_string()];
        let p = composer_system_prompt("build", &tools);
        assert!(p.contains("todo_write"));
        assert!(p.contains("todo_update"));
        // Ask mode stays tool-free and workflow-free.
        let p = composer_system_prompt("ask", &[]);
        assert!(!p.contains("todo_write"));
    }
}

/// Tool spec for the runtime-handled `todo_write` (checklist creation) tool.
fn todo_write_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "todo_write".to_string(),
        description: "创建任务清单。kind=explore 为只读调研任务，kind=edit 为实施修改任务；dependsOn 用本批次内的数组下标表达依赖。返回带 id 的当前清单。".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "description": {"type": "string"},
                            "kind": {"type": "string", "enum": ["explore", "edit"]},
                            "dependsOn": {"type": "array", "items": {"type": "integer"}}
                        },
                        "required": ["title"]
                    }
                }
            },
            "required": ["todos"]
        }),
    }
}

/// Tool spec for the runtime-handled `todo_update` (checklist progress) tool.
fn todo_update_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "todo_update".to_string(),
        description: "更新任务清单中某个 todo 的状态。开始处理一个 todo 前先标记 in_progress，完成后标记 completed（可附 summary 结论）。".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "todo 的 id"},
                "status": {"type": "string", "enum": ["in_progress", "completed", "failed", "cancelled"]},
                "summary": {"type": "string", "description": "完成结论要点（completed 时建议提供）"}
            },
            "required": ["id", "status"]
        }),
    }
}

/// Tool spec for the runtime-handled `task` (subagent delegation) tool.
fn task_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "task".to_string(),
        description: "将一个独立的子任务委派给子代理执行（子代理拥有同样的工具），返回其结果摘要。适合可并行或上下文独立的子任务。".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "子任务的简短标题"},
                "prompt": {"type": "string", "description": "给子代理的完整任务指令，需自包含全部上下文"}
            },
            "required": ["prompt"]
        }),
    }
}

/// Composer-mode system prompt assembly (port of `composer_mode_prompts.py`).
pub fn composer_system_prompt(mode: &str, tools: &[String]) -> String {
    let base = "你是 Agent Debug 的 AI 编码助手，运行在 Rust 重写的后端中。请用简体中文回复。";
    let mode_line = match mode {
        "ask" => "当前为 ASK 模式：只读问答，不调用任何工具，不修改文件。",
        "plan" => "当前为 PLAN 模式：先产出清晰的执行计划，再按计划推进。",
        "debug" => "当前为 DEBUG 模式：系统化定位并修复缺陷，必要时使用工具收集证据。",
        "multitask" => "当前为 MULTITASK 模式：可并行推进多个子任务。",
        _ => "当前为 BUILD 模式：实现用户需求，可使用工具读写文件与执行命令。",
    };
    // Explore-then-edit working agreement for tool-enabled modes.
    let workflow_line = if tools.is_empty() || mode == "ask" {
        ""
    } else {
        "工作流程：① 先用只读工具（read_file/list_dir/grep 等）探索并确认现状；\
         ② 对非平凡任务用 todo_write 列出编辑清单（探索结论写进 description）；\
         ③ 逐个执行：开始前用 todo_update 标记 in_progress，完成后标记 completed 并附 summary；\
         ④ 全部完成后做简短总结。简单的一步任务可以跳过清单直接完成。\n"
    };
    let tool_line = if tools.is_empty() {
        "本轮无可用工具。".to_string()
    } else {
        format!(
            "可用工具：{}。当需要时请通过函数调用使用它们。",
            tools.join(", ")
        )
    };
    format!("{base}\n{mode_line}\n{workflow_line}{tool_line}")
}
