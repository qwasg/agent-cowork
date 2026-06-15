//! The `task` tool: delegate a subtask to a nested agent loop (subagent).

use std::sync::Arc;

use serde_json::json;

use agent_protocol::models::{new_id, ChatMessage};
use agent_protocol::{ApiError, ApiResult};
use agent_providers::tokens::estimate_tokens;
use agent_providers::types::{noop_sink, ProviderRequest, ToolSpec};

use super::control::RunControl;
use super::Runtime;

impl Runtime {
    pub(crate) async fn run_subagent(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        call_id: &str,
        control: &Arc<RunControl>,
        args: &serde_json::Value,
        depth: usize,
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
        // Builtin subagent profile selection (port of `builtin_subagents.py`):
        // unknown types fall back to the general profile.
        let requested_type = args
            .get("subagent_type")
            .or_else(|| args.get("subagentType"))
            .and_then(|v| v.as_str())
            .unwrap_or("general")
            .trim()
            .to_string();
        let profile = self
            .subagents
            .get(&requested_type)
            .or_else(|| crate::subagents::get_builtin("general"))
            .expect("general subagent profile exists");

        // Use the parent `task` tool-call id as the subagent's run/correlation
        // id so `subagent.*` events line up exactly with the subagent card the
        // frontend created from `agent.tool.invoked` (keyed by `toolCallId`).
        // This keeps multiple parallel subagents (Multitask) from collapsing
        // onto a single card. Fall back to a fresh id if the provider omitted
        // the call id.
        let sub_run_id = if call_id.is_empty() {
            new_id("subrun")
        } else {
            call_id.to_string()
        };
        let actor = format!("subagent:{sub_run_id}");
        self.emit_actor(
            session_id,
            "subagent.started",
            "subagent",
            &actor,
            json!({
                "subRunId": sub_run_id,
                "parentRunId": run_id,
                "description": description,
                "subagentType": profile.name,
            }),
            Some(run_id.to_string()),
        );

        // Profile model override beats the session's selected model.
        let model = profile.model.clone().unwrap_or_else(|| {
            self.sessions
                .get(session_id)
                .ok()
                .and_then(|s| s.selected_model_id)
                .unwrap_or_else(|| "default".to_string())
        });
        // Intersect the session's allowed tools with the profile's tool list
        // (read-only profiles like `explorer` never see write tools).
        let allowed: Vec<String> = self
            .allowed_tools(session_id, "build")
            .into_iter()
            .filter(|n| profile.allowed_tools.iter().any(|t| t == n))
            .collect();
        let root = self.workspace_root();
        let system = crate::prompts::build_subagent_system_prompt(
            &profile,
            &allowed,
            Some(root.as_path()),
            crate::workspace::read_branch(&root),
        );
        let messages = vec![ChatMessage::system(system), ChatMessage::user(prompt)];

        // Resource boundary: the profile's step cap (when smaller) bounds the
        // nested loop; depth is gated by `subagent_max_depth`.
        let max_steps = profile
            .max_steps
            .map(|m| m.min(self.tool_loop_steps))
            .unwrap_or(self.tool_loop_steps);

        // Box the recursive async call (depth-gated by `subagent_max_depth`).
        let result = Box::pin(self.run_react_bounded(
            session_id,
            &sub_run_id,
            &model,
            messages,
            &allowed,
            control.clone(),
            depth + 1,
            max_steps,
            None,
        ))
        .await
        .map(|outcome| outcome.text);

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
                ChatMessage::system(crate::prompts::SUBAGENT_SUMMARY_SYSTEM_PROMPT),
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
}

impl Runtime {
    /// Tool spec for the runtime-handled `task` (subagent delegation) tool.
    /// Built per turn so disk-defined profiles show up without a restart.
    pub(crate) fn task_tool_spec(&self) -> ToolSpec {
        let types = self.subagents.type_names();
        ToolSpec {
            name: "task".to_string(),
            description: format!(
                "将一个独立的子任务委派给子代理执行，返回其结果摘要。适合上下文相互独立、可并行的子任务\
（同一轮发出多个 task 调用会并行执行）；几步内能完成的简单操作请直接自己做。\
子代理看不到当前对话，prompt 必须自包含全部背景与期望产出；子代理不能再委派。\n{}",
                self.subagents.render_prompt()
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "子任务的简短标题（展示给用户）"},
                    "prompt": {"type": "string", "description": "给子代理的完整任务指令：背景、目标、范围、期望的产出格式，需自包含"},
                    "subagent_type": {
                        "type": "string",
                        "enum": types,
                        "description": "子代理类型（默认 general=可读写的通用执行）"
                    }
                },
                "required": ["prompt"]
            }),
        }
    }
}
