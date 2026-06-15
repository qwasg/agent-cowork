//! Chat (`ask:execute`) and plan generation / lifecycle / execution.

use serde_json::{json, Value};

use agent_core::code_edit;
use agent_core::runtime::PlanOutcome;
use agent_core::{AgentKind, AgentProfile};
use agent_protocol::events::EventDraft;
use agent_protocol::models::{new_id, now_ts, AgentRun};
use agent_protocol::ApiResult;

use crate::api::AppServices;

impl AppServices {
    pub async fn ask_execute(
        &self,
        session_id: &str,
        user_input: &str,
        context_window: Option<&Value>,
        composer_mode: Option<&str>,
    ) -> ApiResult<Value> {
        self.remember_context_window(session_id, context_window);
        self.sessions.auto_title_from_input(session_id, user_input);

        // Resolve the agent profile: non-coding agents (general / document) are
        // conversational, support only ask/build, and never run the plan engine.
        let kind = self.sessions.get(session_id)?;
        let kind = AgentKind::from_str(&kind.agent_kind);
        let runtime_mode = normalize_composer_mode(composer_mode);
        if kind != AgentKind::Coding {
            let profile = AgentProfile::new(kind);
            let mode = profile.normalize_mode(composer_mode.unwrap_or("build"));
            let cw = self.context_window_for(session_id);
            return self
                .runtime
                .run_composer_chat(session_id, user_input, &mode, cw.as_ref())
                .await;
        }

        if composer_mode.is_some() && runtime_mode != "plan" {
            // Use the remembered window so a turn without a fresh payload
            // still sees the latest editor context.
            let cw = self.context_window_for(session_id);
            return self
                .runtime
                .run_composer_chat(session_id, user_input, &runtime_mode, cw.as_ref())
                .await;
        }
        if composer_mode.is_some() {
            // Composer plan mode: agentic read-only research where the model
            // writes the todo list itself via `todo_write`. The todos are then
            // wrapped into a `ready` plan awaiting user confirmation — nothing
            // executes until the user triggers `plan:execute`.
            return self.run_plan_mode(session_id, user_input).await;
        }
        // Legacy REST path (no composer mode): one-shot plan + auto-execute.
        let bundle = self.plan_engine.generate(session_id, user_input).await?;
        let plan_id = bundle
            .get("plan")
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let run = self.execute_plan(&plan_id).await?;
        let mut out = bundle;
        if let Value::Object(ref mut map) = out {
            map.insert(
                "run".to_string(),
                run.get("run").cloned().unwrap_or(json!(null)),
            );
        }
        Ok(out)
    }

    /// Composer plan mode: run a read-only planning turn, then assemble the
    /// todos the model created during it into a reviewable plan (status
    /// `ready`). Execution waits for explicit user confirmation.
    async fn run_plan_mode(&self, session_id: &str, user_input: &str) -> ApiResult<Value> {
        let cw = self.context_window_for(session_id);
        let before: std::collections::HashSet<String> = self
            .todos
            .list_by_session(session_id)
            .into_iter()
            .map(|t| t.id)
            .collect();
        let mut out = self
            .runtime
            .run_composer_chat(session_id, user_input, "plan", cw.as_ref())
            .await?;
        let created: Vec<_> = self
            .todos
            .list_by_session(session_id)
            .into_iter()
            .filter(|t| !before.contains(&t.id))
            .collect();
        if !created.is_empty() {
            let plan = self
                .plan_engine
                .from_todos(session_id, user_input, &created);
            if let Value::Object(ref mut map) = out {
                map.insert("plan".to_string(), json!(plan));
            }
        }
        Ok(out)
    }

    pub async fn generate_plan(
        &self,
        session_id: &str,
        user_input: &str,
        ctx: Option<&Value>,
    ) -> ApiResult<Value> {
        self.remember_context_window(session_id, ctx);
        self.sessions.auto_title_from_input(session_id, user_input);
        self.plan_engine.generate(session_id, user_input).await
    }

    pub fn get_plan(&self, plan_id: &str) -> ApiResult<Value> {
        Ok(json!({ "plan": self.plan_engine.get(plan_id)? }))
    }

    pub fn patch_plan(&self, plan_id: &str, patch: &Value) -> ApiResult<Value> {
        let mut plan = self.plan_engine.get(plan_id)?;
        if let Some(obj) = patch.get("objective").and_then(|v| v.as_str()) {
            plan.objective = obj.to_string();
        }
        if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
            plan.status = status.to_string();
        }
        plan.updated_at = now_ts();
        self.plan_engine.save(&plan);
        Ok(json!({ "plan": plan }))
    }

    pub fn confirm_plan(&self, plan_id: &str) -> ApiResult<Value> {
        let mut plan = self.plan_engine.get(plan_id)?;
        plan.status = "confirmed".to_string();
        plan.updated_at = now_ts();
        self.plan_engine.save(&plan);
        Ok(json!({ "plan": plan }))
    }

    pub async fn replan(&self, plan_id: &str, _payload: &Value) -> ApiResult<Value> {
        let plan = self.plan_engine.get(plan_id)?;
        self.plan_engine
            .generate(&plan.session_id, &plan.objective)
            .await
    }

    pub async fn execute_plan(&self, plan_id: &str) -> ApiResult<Value> {
        let mut plan = self.plan_engine.get(plan_id)?;
        let session = self.sessions.get(&plan.session_id)?;
        let model = session
            .selected_model_id
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let run = AgentRun {
            id: new_id("run"),
            session_id: plan.session_id.clone(),
            plan_id: Some(plan_id.to_string()),
            trigger: "plan".to_string(),
            status: "running".to_string(),
            active_todo_ids: vec![],
            completed_todo_ids: vec![],
            failed_todo_ids: vec![],
            parallel_limit: self.cfg.parallel_limit as i64,
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        self.runtime.save_run(&run).await;
        plan.latest_execution_id = Some(run.id.clone());
        plan.status = "executing".to_string();
        self.plan_engine.save(&plan);
        self.sessions.set_active_run(
            &plan.session_id,
            Some(run.id.clone()),
            Some(plan_id.to_string()),
        );

        self.bus.emit(
            EventDraft::new(&plan.session_id, "plan.execution.started", "plan")
                .payload(json!({ "planId": plan_id, "runId": run.id })),
        );

        // Run the DAG to completion (parallel batches).
        let outcome = self
            .runtime
            .run_plan(&run.id, &plan.session_id, &model)
            .await?;

        // Reflect the real outcome instead of always claiming success.
        let (run_status, plan_status) = match outcome {
            PlanOutcome::Completed => ("completed", "completed"),
            PlanOutcome::Cancelled => ("cancelled", "cancelled"),
            PlanOutcome::PartialFailure => ("failed", "failed"),
        };
        let mut finished = self.runtime.get_run(&run.id).unwrap_or(run);
        finished.status = run_status.to_string();
        finished.updated_at = now_ts();
        self.runtime.save_run(&finished).await;
        plan.status = plan_status.to_string();
        self.plan_engine.save(&plan);
        self.sessions.set_active_run(&plan.session_id, None, None);

        // After a successful run, try to produce a code-edit proposal from the
        // session's editor context (heuristic first, LLM fallback) — port of
        // Python `_maybe_propose_code_edit`.
        if run_status == "completed" {
            if let Some(payload) = self
                .maybe_propose_code_edit(&plan.session_id, &plan.objective)
                .await
            {
                self.proposals.record(&payload);
                self.bus.emit(
                    EventDraft::new(&plan.session_id, "agent.code_edit.proposed", "agent")
                        .payload(payload)
                        .correlation(Some(finished.id.clone())),
                );
            }
        }

        self.bus.emit(
            EventDraft::new(&plan.session_id, "plan.execution.completed", "plan").payload(json!({
                "planId": plan_id,
                "runId": finished.id,
                "outcome": run_status,
                "completedTodos": finished.completed_todo_ids.len(),
                "failedTodos": finished.failed_todo_ids.len(),
            })),
        );
        Ok(json!({ "run": finished, "plan": plan }))
    }

    /// Heuristic first, LLM fallback when an active file is present.
    async fn maybe_propose_code_edit(&self, session_id: &str, objective: &str) -> Option<Value> {
        let context_window = self.context_window_for(session_id);
        if let Some(payload) =
            code_edit::maybe_generate(session_id, objective, context_window.as_ref())
        {
            return Some(payload);
        }
        let cw = context_window.as_ref()?;
        cw.get("activeFile").filter(|f| f.is_object())?;
        let model = self
            .sessions
            .get(session_id)
            .ok()
            .and_then(|s| s.selected_model_id)
            .unwrap_or_else(|| "default".to_string());
        code_edit::propose_with_llm(&self.providers, session_id, objective, Some(cw), &model).await
    }
}

/// Maps composer mode to a runtime mode (mirrors `normalize_composer_runtime_mode`).
pub fn normalize_composer_mode(mode: Option<&str>) -> String {
    match mode {
        Some("build") | Some("debug") | Some("ask") | Some("multitask") => {
            mode.unwrap().to_string()
        }
        Some("plan") => "plan".to_string(),
        _ => "plan".to_string(),
    }
}
