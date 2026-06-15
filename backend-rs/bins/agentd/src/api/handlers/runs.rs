//! Run inspection (logs / metrics) and lifecycle control (pause / resume /
//! cancel / rerun).

use std::collections::HashMap;

use serde_json::{json, Value};

use agent_core::runtime::PlanOutcome;
use agent_protocol::events::EventDraft;
use agent_protocol::models::{now_ts, AgentRun};
use agent_protocol::{ApiError, ApiResult};

use crate::api::AppServices;

impl AppServices {
    pub fn get_run(&self, run_id: &str) -> ApiResult<Value> {
        Ok(json!({ "run": self.runtime.get_run(run_id)? }))
    }

    pub fn get_run_logs(&self, run_id: &str) -> ApiResult<Value> {
        let run = self.runtime.get_run(run_id)?;
        let entries: Vec<Value> = self
            .bus
            .snapshot(&run.session_id)
            .into_iter()
            .filter(|e| e.correlation_id.as_deref() == Some(run_id))
            .map(|e| e.to_wire())
            .collect();
        let mut by_type: HashMap<String, usize> = HashMap::new();
        for e in &entries {
            if let Some(t) = e.get("type").and_then(|t| t.as_str()) {
                *by_type.entry(t.to_string()).or_insert(0) += 1;
            }
        }
        Ok(json!({
            "runId": run_id,
            "entries": entries,
            "summary": {
                "total": entries.len(),
                "byType": by_type,
                "status": run.status,
            },
        }))
    }

    pub fn get_run_metrics(&self, run_id: &str) -> ApiResult<Value> {
        let run = self.runtime.get_run(run_id)?;
        let related: Vec<_> = self
            .bus
            .snapshot(&run.session_id)
            .into_iter()
            .filter(|e| e.correlation_id.as_deref() == Some(run_id))
            .collect();
        let tokens_total: i64 = related
            .iter()
            .filter_map(|e| {
                e.payload
                    .pointer("/tokenUsage/output")
                    .and_then(|v| v.as_i64())
            })
            .sum();
        // Precise usage from `agent.usage` events (one per provider call).
        let sum_usage = |key: &str| -> i64 {
            related
                .iter()
                .filter(|e| e.event_type == "agent.usage")
                .filter_map(|e| e.payload.get(key).and_then(|v| v.as_i64()))
                .sum()
        };
        let tool_calls = related
            .iter()
            .filter(|e| e.event_type == "agent.tool.invoked")
            .count();
        let tool_failures = related
            .iter()
            .filter(|e| e.event_type == "agent.tool.failed")
            .count();
        let compactions = related
            .iter()
            .filter(|e| e.event_type == "agent.context.compacted")
            .count();
        let steps = related
            .iter()
            .filter(|e| e.event_type == "agent.step")
            .count();
        // Per-tool wall-clock totals from `durationMs` on tool result events.
        let mut tool_duration_ms: i64 = 0;
        let mut by_tool: HashMap<String, Value> = HashMap::new();
        for e in related.iter().filter(|e| {
            e.event_type == "agent.tool.completed" || e.event_type == "agent.tool.failed"
        }) {
            let ms = e
                .payload
                .get("durationMs")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            tool_duration_ms += ms;
            let name = e
                .payload
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let entry = by_tool
                .entry(name.to_string())
                .or_insert_with(|| json!({ "calls": 0, "durationMs": 0 }));
            entry["calls"] = json!(entry["calls"].as_i64().unwrap_or(0) + 1);
            entry["durationMs"] = json!(entry["durationMs"].as_i64().unwrap_or(0) + ms);
        }
        Ok(json!({
            "runId": run.id,
            "status": run.status,
            "completedTodoIds": run.completed_todo_ids,
            "failedTodoIds": run.failed_todo_ids,
            "tokensTotal": tokens_total,
            "usage": {
                "promptTokens": sum_usage("promptTokens"),
                "completionTokens": sum_usage("completionTokens"),
                "totalTokens": sum_usage("totalTokens"),
                "cacheReadTokens": sum_usage("cacheReadTokens"),
                "providerCalls": related.iter().filter(|e| e.event_type == "agent.usage").count(),
            },
            "stepsTotal": steps,
            "toolCallsTotal": tool_calls,
            "toolFailuresTotal": tool_failures,
            "toolDurationMsTotal": tool_duration_ms,
            "toolsByName": by_tool,
            "compactionsTotal": compactions,
            "storeWriteFailures": agent_store::write_failure_count(),
            "events": related.len(),
        }))
    }

    pub fn pause_run(&self, run_id: &str) -> Value {
        json!({ "ok": self.runtime.pause_run(run_id), "runId": run_id })
    }
    pub fn resume_run(&self, run_id: &str) -> Value {
        json!({ "ok": self.runtime.resume_run(run_id), "runId": run_id })
    }
    pub fn cancel_run(&self, run_id: &str) -> Value {
        json!({ "ok": self.runtime.cancel_run(run_id), "runId": run_id })
    }

    /// Queue a steering message into an active run (`POST /runs/{id}:steer`).
    pub fn steer_run(&self, run_id: &str, payload: &Value) -> ApiResult<Value> {
        let text = payload
            .get("text")
            .or_else(|| payload.get("message"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if text.is_empty() {
            return Err(ApiError::new("INVALID_INPUT", "text required"));
        }
        if !self.runtime.steer_run(run_id, text) {
            return Err(ApiError::new(
                "RUN_NOT_ACTIVE",
                format!("run '{run_id}' is not running"),
            ));
        }
        Ok(json!({ "ok": true, "runId": run_id, "queued": text }))
    }

    /// Requeue a todo and re-drive the session's plan DAG (port of Python
    /// `rerun_todo`, which actually re-executes instead of just flipping
    /// status).
    pub async fn rerun_todo(&self, run_id: &str, todo_id: &str) -> ApiResult<Value> {
        let run = self.runtime.get_run(run_id)?;
        let mut todo = self.todos.get(todo_id)?;
        todo.status = "queued".to_string();
        todo.retry_count += 1;
        todo.last_error = None;
        todo.updated_at = now_ts();
        self.todos.save(&todo);
        self.bus.emit(
            EventDraft::new(&run.session_id, "todo.updated", "todo")
                .payload(json!({ "id": todo.id, "title": todo.title, "status": todo.status }))
                .correlation(Some(run.id.clone())),
        );
        let outcome = self.redrive_plan_dag(&run).await?;
        Ok(json!({ "accepted": true, "outcome": outcome }))
    }

    /// Real node-level rerun: validate the plan node, requeue its todo (task
    /// ids and todo ids are shared) and re-drive the DAG.
    pub async fn rerun_node(&self, run_id: &str, node_id: &str) -> ApiResult<Value> {
        let run = self.runtime.get_run(run_id)?;
        let plan_id = run
            .plan_id
            .clone()
            .ok_or_else(|| ApiError::new("PLAN_NOT_FOUND", "run has no plan"))?;
        let plan = self.plan_engine.get(&plan_id)?;
        let node_exists = plan
            .stages
            .iter()
            .flat_map(|s| s.tasks.iter())
            .any(|t| t.id == node_id);
        if !node_exists {
            return Err(ApiError::new(
                "PLAN_NODE_NOT_FOUND",
                format!("plan node not found: {node_id}"),
            ));
        }
        let mut todo = self.todos.get(node_id)?;
        todo.status = "queued".to_string();
        todo.retry_count += 1;
        todo.last_error = None;
        todo.updated_at = now_ts();
        self.todos.save(&todo);
        let outcome = self.redrive_plan_dag(&run).await?;
        Ok(json!({ "accepted": true, "nodeId": node_id, "outcome": outcome }))
    }

    /// Re-run the plan DAG for an existing run: ready todos (the requeued
    /// ones plus anything they unblock) execute again under the same run id.
    async fn redrive_plan_dag(&self, run: &AgentRun) -> ApiResult<String> {
        let session = self.sessions.get(&run.session_id)?;
        let model = session
            .selected_model_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let mut active = self.runtime.get_run(&run.id)?;
        active.status = "running".to_string();
        active.updated_at = now_ts();
        self.runtime.save_run(&active).await;
        self.sessions
            .set_active_run(&run.session_id, Some(run.id.clone()), run.plan_id.clone());
        let outcome = self
            .runtime
            .run_plan(&run.id, &run.session_id, &model)
            .await?;
        let status = match outcome {
            PlanOutcome::Completed => "completed",
            PlanOutcome::Cancelled => "cancelled",
            PlanOutcome::PartialFailure => "failed",
        };
        let mut finished = self.runtime.get_run(&run.id)?;
        finished.status = status.to_string();
        finished.updated_at = now_ts();
        self.runtime.save_run(&finished).await;
        self.sessions.set_active_run(&run.session_id, None, None);
        Ok(status.to_string())
    }
}
