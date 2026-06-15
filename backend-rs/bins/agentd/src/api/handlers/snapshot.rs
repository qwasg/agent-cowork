//! Event replay and the aggregated design snapshot.

use serde_json::{json, Value};

use crate::api::AppServices;

impl AppServices {
    pub fn get_replay(&self, session_id: &str) -> Value {
        let events: Vec<Value> = self
            .bus
            .snapshot(session_id)
            .into_iter()
            .map(|e| e.to_wire())
            .collect();
        json!({ "sessionId": session_id, "events": events })
    }

    pub fn replay_since(&self, session_id: &str, from_seq: i64, limit: Option<usize>) -> Value {
        let (events, gap) = self.bus.replay_since(session_id, from_seq, limit);
        let wire: Vec<Value> = events.into_iter().map(|e| e.to_wire()).collect();
        json!({ "sessionId": session_id, "events": wire, "gap": gap, "latestSeq": self.bus.latest_seq(session_id) })
    }

    /// Full design-snapshot aggregation (port of Python `get_design_snapshot`):
    /// sessions / activeSession / planBundle / todos / events / run / swarm /
    /// diffs / proposals / metrics / contextWindow / models / latestSeq.
    pub fn get_design_snapshot(&self, session_id: Option<&str>) -> Value {
        let sessions = self.sessions.list();
        let active_session = match session_id.filter(|s| !s.is_empty()) {
            Some(sid) => self.sessions.get(sid).ok(),
            None => sessions.first().cloned(),
        };

        let mut plan_bundle = Value::Null;
        let mut todos: Vec<Value> = Vec::new();
        let mut events: Vec<Value> = Vec::new();
        let mut run = Value::Null;
        let mut context_window = Value::Null;
        let mut proposals: Vec<Value> = Vec::new();
        let mut latest_seq = 0i64;

        if let Some(session) = &active_session {
            context_window = self.context_window_for(&session.id).unwrap_or(Value::Null);
            if let Some(plan_id) = &session.active_plan_id {
                if let Ok(plan) = self.plan_engine.get(plan_id) {
                    plan_bundle = render_plan_snapshot(&plan);
                }
            }
            todos = self
                .todos
                .list_by_session(&session.id)
                .into_iter()
                .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
                .collect();
            events = self
                .bus
                .snapshot(&session.id)
                .into_iter()
                .map(|e| e.to_wire())
                .collect();
            if let Some(run_id) = &session.active_run_id {
                if let Ok(r) = self.runtime.get_run(run_id) {
                    run = serde_json::to_value(r).unwrap_or(Value::Null);
                }
            }
            proposals = self
                .proposals
                .list_for_session(&session.id)
                .into_iter()
                .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
                .collect();
            latest_seq = self.bus.latest_seq(&session.id);
        }

        let swarm_state = self.swarm.state();
        let swarm_nodes = swarm_state["nodes"].as_array().cloned().unwrap_or_default();
        let diffs = extract_design_diffs(&events);
        let metrics = build_design_metrics(
            &plan_bundle,
            &todos,
            &events,
            swarm_nodes.len(),
            &context_window,
            diffs.len(),
        );

        json!({
            "sessions": sessions,
            "activeSession": active_session,
            "planBundle": plan_bundle,
            "todos": todos,
            "events": events,
            "run": run,
            "swarm": swarm_state,
            "diffs": diffs,
            "proposals": proposals,
            "metrics": metrics,
            "contextWindow": context_window,
            "models": self.list_models(),
            "latestSeq": latest_seq,
        })
    }
}

/// Plan snapshot bundle for the design snapshot (Python `_render_plan_snapshot`
/// shape: `{plan, stages, tasks, steps, versions}`).
fn render_plan_snapshot(plan: &agent_protocol::models::Plan) -> Value {
    let stages: Vec<Value> = plan
        .stages
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "planId": s.plan_id,
                "title": s.title,
                "order": s.order,
                "status": s.status,
            })
        })
        .collect();
    let tasks: Vec<Value> = plan
        .stages
        .iter()
        .flat_map(|s| s.tasks.iter())
        .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
        .collect();
    json!({
        "plan": {
            "id": plan.id,
            "sessionId": plan.session_id,
            "objective": plan.objective,
            "status": plan.status,
            "currentVersionId": plan.current_version_id,
            "latestExecutionId": plan.latest_execution_id,
            "createdAt": plan.created_at,
            "updatedAt": plan.updated_at,
        },
        "stages": stages,
        "tasks": tasks,
        "steps": [],
        "versions": [],
    })
}

/// Diff entries derived from `agent.code_edit.proposed` events (Python
/// `_extract_design_diffs`).
fn extract_design_diffs(events: &[Value]) -> Vec<Value> {
    let mut diffs = Vec::new();
    for event in events {
        if event.get("type").and_then(|t| t.as_str()) != Some("agent.code_edit.proposed") {
            continue;
        }
        let Some(payload) = event.get("payload").filter(|p| p.is_object()) else {
            continue;
        };
        let proposal_id = payload.get("proposalId").cloned().unwrap_or(Value::Null);
        for change in payload
            .get("changes")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            if !change.is_object() {
                continue;
            }
            let original = change
                .get("originalContent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let proposed = change
                .get("proposedContent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let orig_lines = original.lines().count() as i64;
            let prop_lines = proposed.lines().count() as i64;
            diffs.push(json!({
                "id": change.get("changeId").cloned().unwrap_or_else(|| proposal_id.clone()),
                "proposalId": proposal_id,
                "path": change.get("path").and_then(|v| v.as_str()).unwrap_or("unknown"),
                "description": change
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| payload.get("summary").and_then(|v| v.as_str()).unwrap_or("")),
                "plus": (prop_lines - orig_lines).max(0),
                "minus": (orig_lines - prop_lines).max(0),
                "state": "pending",
                "at": event.get("ts").cloned().unwrap_or(Value::Null),
                "by": "agent",
                "originalContent": original,
                "proposedContent": proposed,
            }));
        }
    }
    diffs
}

/// Aggregate metrics for the snapshot (Python `_build_design_metrics`).
fn build_design_metrics(
    plan_bundle: &Value,
    todos: &[Value],
    events: &[Value],
    swarm_node_count: usize,
    context_window: &Value,
    files_touched: usize,
) -> Value {
    let token_total: i64 = events
        .iter()
        .filter_map(|e| e.pointer("/payload/tokenUsage/total"))
        .filter_map(|v| v.as_i64())
        .sum();
    let tool_calls = events
        .iter()
        .filter(|e| {
            matches!(
                e.pointer("/source/domain").and_then(|d| d.as_str()),
                Some("tool") | Some("agent")
            )
        })
        .count();
    let completed_todos = todos
        .iter()
        .filter(|t| {
            matches!(
                t.get("status").and_then(|s| s.as_str()),
                Some("completed") | Some("rolledUp") | Some("rolled_up")
            )
        })
        .count();
    let tasks = plan_bundle
        .get("tasks")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    let completed_plan_nodes = tasks
        .iter()
        .filter(|t| {
            matches!(
                t.get("status").and_then(|s| s.as_str()),
                Some("completed") | Some("summarized")
            )
        })
        .count();
    json!({
        "totalTokens": token_total,
        "toolCalls": tool_calls,
        "filesTouched": files_touched,
        "avgLatencyMs": 0,
        "planProgress": { "completed": completed_plan_nodes, "total": tasks.len() },
        "todos": { "completed": completed_todos, "total": todos.len() },
        "subagents": swarm_node_count,
        "contextFillPct": estimate_context_fill_pct(context_window),
    })
}

/// Rough context fill estimate from the editor context window (Python
/// `_estimate_context_fill_pct`).
fn estimate_context_fill_pct(context_window: &Value) -> i64 {
    let Some(obj) = context_window.as_object() else {
        return 0;
    };
    let mut total_chars = 0usize;
    if let Some(content) = obj
        .get("activeFile")
        .and_then(|f| f.get("content"))
        .and_then(|c| c.as_str())
    {
        total_chars += content.len();
    }
    if let Some(sel) = obj
        .get("selection")
        .and_then(|s| s.get("selectedText"))
        .and_then(|t| t.as_str())
    {
        total_chars += sel.len();
    }
    if let Some(term) = obj.get("terminalRecentOutput").and_then(|t| t.as_str()) {
        total_chars += term.len();
    }
    for key in ["relevantLogs", "openFiles"] {
        if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
            for item in arr {
                total_chars += item.as_str().map(|s| s.len()).unwrap_or(0);
            }
        }
    }
    let estimated_tokens = total_chars / 4;
    if estimated_tokens == 0 {
        return 0;
    }
    ((estimated_tokens as f64 / 32_000.0 * 100.0).round() as i64).clamp(1, 100)
}
