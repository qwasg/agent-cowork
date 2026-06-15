//! Tool-call dispatch: permission gating, interactive approval, runtime
//! tools (task / todo / MCP) and registry tools.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use agent_protocol::models::ToolCall;
use agent_protocol::{ApiError, ApiResult};

use crate::hooks::PreToolVerdict;
use crate::permission::RuleDecision;

use super::control::RunControl;
use super::Runtime;

impl Runtime {
    pub(crate) async fn dispatch_tool(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        call: &ToolCall,
        control: &Arc<RunControl>,
        depth: usize,
    ) -> ApiResult<String> {
        // Tool lifecycle events (`agent.tool.invoked/completed/failed`) are
        // emitted by the turn recorder in `react.rs` — dispatch only returns
        // results / errors.
        let mut args: serde_json::Value =
            serde_json::from_str(&call.arguments).unwrap_or_else(|e| {
                tracing::warn!(
                    "tool '{}': invalid JSON arguments ({e}); using {{}}",
                    call.name
                );
                json!({})
            });

        // `plan` mode's read-only invariant is absolute (rules can't override).
        if !self.permissions.can_use_tool(session_id, &call.name) {
            return Err(ApiError::new(
                "TOOL_FORBIDDEN",
                format!("tool '{}' not permitted in current mode", call.name),
            ));
        }

        // Rule layer: first matching allow/deny/ask rule overrides the mode
        // default (`auto` ⇒ approval for write tools, `bypass` ⇒ allow all).
        let needs_approval = match self.permissions.evaluate(&call.name, &args) {
            RuleDecision::Deny => {
                return Err(ApiError::new(
                    "TOOL_FORBIDDEN",
                    format!("tool '{}' denied by permission rule", call.name),
                ));
            }
            RuleDecision::Allow => false,
            RuleDecision::Ask => true,
            RuleDecision::Default => self.permissions.requires_approval(session_id, &call.name),
        };
        if needs_approval {
            let approved = self
                .await_approval(session_id, run_id, &call.name, &args, control)
                .await;
            if !approved {
                return Err(ApiError::new(
                    "TOOL_FORBIDDEN",
                    format!("tool '{}' was denied by the user (or timed out)", call.name),
                ));
            }
        }

        // PreToolUse hooks may rewrite the arguments or block the call.
        match self.hooks.pre_tool_use(session_id, &call.name, &args).await {
            PreToolVerdict::Block(reason) => {
                return Err(ApiError::new(
                    "TOOL_FORBIDDEN",
                    format!("tool '{}' blocked by PreToolUse hook: {reason}", call.name),
                ));
            }
            PreToolVerdict::Continue(Some(rewritten)) => args = rewritten,
            PreToolVerdict::Continue(None) => {}
        }

        let result = self
            .dispatch_inner(session_id, run_id, call, &args, control, depth)
            .await;

        // PostToolUse hooks observe the outcome (success or failure).
        match &result {
            Ok(out) => {
                self.hooks
                    .post_tool_use(session_id, &call.name, &args, out, true)
                    .await
            }
            Err(e) => {
                self.hooks
                    .post_tool_use(session_id, &call.name, &args, &e.message, false)
                    .await
            }
        }
        result
    }

    async fn dispatch_inner(
        self: &Arc<Self>,
        session_id: &str,
        run_id: &str,
        call: &ToolCall,
        args: &serde_json::Value,
        control: &Arc<RunControl>,
        depth: usize,
    ) -> ApiResult<String> {
        // `task` delegates to a nested subagent loop (runtime-handled tool).
        if call.name == "task" {
            if depth >= self.subagent_max_depth {
                return Err(ApiError::new(
                    "TOOL_FORBIDDEN",
                    format!(
                        "subagent nesting limit reached (max depth {})",
                        self.subagent_max_depth
                    ),
                ));
            }
            return self
                .run_subagent(session_id, run_id, &call.id, control, args, depth)
                .await;
        }

        // MCP tools: `mcp__{server}__{tool}` routes through the manager's
        // long-lived connection (stdio or HTTP).
        if let Some(rest) = call.name.strip_prefix("mcp__") {
            if let Some((server, tool)) = rest.split_once("__") {
                return self
                    .mcp
                    .call_tool(server, tool, args.clone())
                    .await
                    .map_err(|message| ApiError::new("MCP_TOOL_ERROR", message));
            }
            return Err(ApiError::new(
                "MCP_TOOL_ERROR",
                format!("invalid MCP tool name: {}", call.name),
            ));
        }

        // Structured edit tools are runtime-handled: they write the
        // workspace and record an applied proposal.
        if call.name == "str_replace_edit" || call.name == "apply_patch" {
            return self
                .handle_edit_tool(session_id, run_id, &call.name, args)
                .await;
        }

        // `todo_write` / `plan_write` / `todo_update` are intercepted by the
        // runtime (not registry tools): they manage the session checklist/plan.
        if call.name == "plan_write" {
            return Ok(self.handle_plan_write(session_id, run_id, args));
        }
        if call.name == "todo_write" || call.name == "write_todos" {
            return Ok(self.handle_write_todos(session_id, run_id, args));
        }
        if call.name == "todo_update" {
            return self.handle_update_todo(session_id, run_id, args);
        }

        // `memory_*` are runtime-handled: durable cross-session memory.
        if matches!(
            call.name.as_str(),
            "memory_write" | "memory_search" | "memory_delete"
        ) {
            return self.handle_memory_tool(session_id, run_id, &call.name, args);
        }

        let ctx = self.tool_ctx(session_id, run_id);
        let outcome = self.tools.run(&call.name, args.clone(), &ctx).await?;
        Ok(outcome.content)
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
}
