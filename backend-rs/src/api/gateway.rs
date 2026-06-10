//! Business gateway: wires all domain services and implements the operations
//! behind the REST surface (port of `api/rest_gateway.py`, decomposed).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::config::Config;
use crate::contracts::models::{new_id, now_ts, AgentModelOption, AgentRun};
use crate::contracts::{ApiError, ApiResult};
use crate::domain::{
    AuthService, CheckpointService, PermissionService, PlanEngine, Runtime, SessionService,
    TodoEngine,
};
use crate::infra::store::{T_CHANNELS, T_RUNS};
use crate::infra::{CryptoStore, EventBus, JsonlStore, Store};
use crate::provider::channels::{self, Channel, ChannelModel};
use crate::provider::ProviderExecutionService;
use crate::tools::skill::discover_skills;
use crate::tools::ToolRegistry;

pub struct AppServices {
    pub cfg: Config,
    pub store: Arc<Store>,
    pub bus: Arc<EventBus>,
    pub crypto: Arc<CryptoStore>,
    pub providers: Arc<ProviderExecutionService>,
    pub sessions: Arc<SessionService>,
    pub permissions: Arc<PermissionService>,
    pub todos: Arc<TodoEngine>,
    pub auth: Arc<AuthService>,
    pub checkpoints: Arc<CheckpointService>,
    pub tools: Arc<ToolRegistry>,
    pub runtime: Arc<Runtime>,
    pub plan_engine: Arc<PlanEngine>,
    pub skill_dirs: Vec<PathBuf>,
}

impl AppServices {
    pub fn build(cfg: Config) -> anyhow::Result<Arc<Self>> {
        let store = Arc::new(Store::open(cfg.data_dir.join("agentd.redb"))?);
        // `persist_events=false` disables the durable JSONL log entirely.
        let jsonl = if cfg.persist_events {
            Some(Arc::new(JsonlStore::new(cfg.session_dir.clone())))
        } else {
            None
        };
        let bus = EventBus::new(cfg.event_buffer_cap, jsonl);
        let crypto = CryptoStore::open(cfg.data_dir.join(".agent_master.key"));
        let providers = ProviderExecutionService::build(&cfg, &store, &crypto);

        let sessions = Arc::new(SessionService::new(store.clone(), cfg.history_turns));
        let permissions = Arc::new(PermissionService::new(store.clone()));
        let todos = Arc::new(TodoEngine::new(store.clone()));
        let auth = AuthService::new(store.clone(), cfg.data_dir.join(".agent_auth_secret"));

        let workspace_root = store
            .kv_get("workspace_root")
            .map(PathBuf::from)
            .unwrap_or_else(|| cfg.workspace_root.clone());
        let checkpoints = Arc::new(CheckpointService::new(
            store.clone(),
            workspace_root.clone(),
        ));

        let skill_dirs = vec![
            workspace_root.join("skills"),
            workspace_root.join(".cursor").join("skills"),
            cfg.workspace_root.join("skills"),
        ];

        let tools = Arc::new(ToolRegistry::build(true));
        let runtime = Runtime::new(
            &cfg,
            providers.clone(),
            tools.clone(),
            bus.clone(),
            sessions.clone(),
            permissions.clone(),
            todos.clone(),
            store.clone(),
            skill_dirs.clone(),
        );
        let plan_engine = Arc::new(PlanEngine::new(
            providers.clone(),
            store.clone(),
            bus.clone(),
            todos.clone(),
            sessions.clone(),
        ));

        Ok(Arc::new(AppServices {
            cfg,
            store,
            bus,
            crypto,
            providers,
            sessions,
            permissions,
            todos,
            auth,
            checkpoints,
            tools,
            runtime,
            plan_engine,
            skill_dirs,
        }))
    }

    // ---------------- sessions ----------------

    pub fn list_sessions(&self) -> Value {
        json!({ "sessions": self.sessions.list() })
    }

    pub fn create_session(&self, title: &str, model_id: Option<String>, web: bool) -> Value {
        let session = self.sessions.create(title, model_id, web);
        json!({ "session": session })
    }

    pub fn get_session(&self, id: &str) -> ApiResult<Value> {
        Ok(json!({ "session": self.sessions.get(id)? }))
    }

    pub fn patch_session(&self, id: &str, patch: &Value) -> ApiResult<Value> {
        Ok(json!({ "session": self.sessions.patch(id, patch)? }))
    }

    pub fn delete_session(&self, id: &str) -> ApiResult<Value> {
        self.sessions.delete(id)?;
        self.bus.purge_session(id);
        Ok(json!({ "ok": true }))
    }

    pub fn fork_session(&self, id: &str) -> ApiResult<Value> {
        let src = self.sessions.get(id)?;
        let forked = self.sessions.create(
            &format!("{} (fork)", src.title),
            src.selected_model_id.clone(),
            src.web_search_enabled,
        );
        Ok(json!({ "session": forked }))
    }

    pub fn revert_session(&self, id: &str, _message_id: Option<&str>) -> ApiResult<Value> {
        // Minimal revert: clears the active run pointer.
        let mut session = self.sessions.get(id)?;
        session.active_run_id = None;
        session.touch();
        self.sessions.save(&session);
        Ok(json!({ "session": session }))
    }

    pub fn set_session_model(&self, id: &str, model_id: Option<String>) -> ApiResult<Value> {
        Ok(json!({ "session": self.sessions.set_model(id, model_id)? }))
    }

    // ---------------- models / providers ----------------

    pub fn list_models(&self) -> Value {
        let mut models: Vec<AgentModelOption> = Vec::new();
        // Env OpenAI-compatible default.
        if self.cfg.openai_api_key.is_some() {
            models.push(AgentModelOption {
                id: self.cfg.openai_model.clone(),
                label: self.cfg.openai_model.clone(),
                provider: "openai".to_string(),
                source: "env".to_string(),
                tier: "balanced".to_string(),
                supports_reasoning: false,
                context_window_tokens: Some(128_000),
                availability: "available".to_string(),
                is_default: true,
            });
        }
        // Channel models.
        if let Ok(list) = self.store.list::<Channel>(T_CHANNELS) {
            for ch in list.into_iter().filter(|c| c.enabled) {
                for m in &ch.models {
                    models.push(AgentModelOption {
                        id: format!("channel:{}:{}", ch.id, m.id),
                        label: format!("{} · {}", ch.label, m.label),
                        provider: ch.provider_type.clone(),
                        source: "channel".to_string(),
                        tier: "balanced".to_string(),
                        supports_reasoning: m.supports_reasoning,
                        context_window_tokens: Some(128_000),
                        availability: if ch.has_key() {
                            "available"
                        } else {
                            "needs-key"
                        }
                        .to_string(),
                        is_default: false,
                    });
                }
            }
        }
        if models.is_empty() {
            models.push(AgentModelOption {
                id: "mock".to_string(),
                label: "离线模拟模型 (mock)".to_string(),
                provider: "mock".to_string(),
                source: "builtin".to_string(),
                tier: "fast".to_string(),
                supports_reasoning: false,
                context_window_tokens: Some(32_000),
                availability: "available".to_string(),
                is_default: true,
            });
        }
        let default_id = models.iter().find(|m| m.is_default).map(|m| m.id.clone());
        json!({ "models": models, "defaultModelId": default_id })
    }

    pub fn provider_status(&self) -> Value {
        json!({
            "providers": self.providers.provider_names(),
            "hasRealProvider": self.providers.has_real_provider(),
        })
    }

    pub fn get_search_config(&self) -> Value {
        let configured =
            self.cfg.tavily_api_key.is_some() || self.store.kv_get("search_api_key").is_some();
        json!({ "searchConfig": { "provider": "tavily", "configured": configured } })
    }

    pub fn set_search_config(&self, payload: &Value) -> Value {
        if let Some(key) = payload.get("apiKey").and_then(|v| v.as_str()) {
            let enc = self.crypto.encrypt(key);
            let _ = self.store.kv_put("search_api_key", &enc);
        }
        json!({ "ok": true })
    }

    pub fn get_model_preferences(&self) -> Value {
        json!({ "preferences": { "globalDefaultModelId": self.store.kv_get("default_model") } })
    }

    pub fn set_model_preferences(&self, model_id: &str) -> ApiResult<Value> {
        if model_id.trim().is_empty() {
            return Err(ApiError::new("MODEL_NOT_FOUND", "modelId required"));
        }
        let _ = self.store.kv_put("default_model", model_id);
        Ok(json!({ "ok": true, "globalDefaultModelId": model_id }))
    }

    // ---------------- channels ----------------

    pub fn list_provider_types(&self) -> Value {
        json!({ "providerTypes": channels::PROVIDER_TYPES })
    }

    pub fn list_channels(&self) -> Value {
        let list = self.store.list::<Channel>(T_CHANNELS).unwrap_or_default();
        json!({ "channels": list })
    }

    pub fn upsert_channel(&self, payload: &Value) -> ApiResult<Value> {
        let provider_type = payload
            .get("providerType")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::new("AUTH_INVALID_INPUT", "providerType required"))?;
        let info = channels::provider_type(provider_type)
            .ok_or_else(|| ApiError::new("AUTH_INVALID_INPUT", "unknown providerType"))?;
        let id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| new_id("ch"));
        let existing = self.store.get::<Channel>(T_CHANNELS, &id).ok().flatten();
        let api_key_enc = match payload.get("apiKey").and_then(|v| v.as_str()) {
            Some(k) if !k.is_empty() => self.crypto.encrypt(k),
            _ => existing
                .as_ref()
                .map(|c| c.api_key_enc.clone())
                .unwrap_or_default(),
        };
        let models: Vec<ChannelModel> = payload
            .get("models")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .or_else(|| existing.as_ref().map(|c| c.models.clone()))
            .unwrap_or_default();
        let channel = Channel {
            id: id.clone(),
            provider_type: provider_type.to_string(),
            label: payload
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(info.label)
                .to_string(),
            base_url: payload
                .get("baseUrl")
                .and_then(|v| v.as_str())
                .unwrap_or(info.default_base_url)
                .to_string(),
            api_key_enc,
            models,
            enabled: payload
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            created_at: existing.map(|c| c.created_at).unwrap_or_else(now_ts),
        };
        let _ = self.store.put(T_CHANNELS, &id, &channel);
        Ok(json!({ "channel": channel }))
    }

    pub fn delete_channel(&self, id: &str) -> ApiResult<Value> {
        let existed = self.store.delete(T_CHANNELS, id).unwrap_or(false);
        if !existed {
            return Err(ApiError::new("MODEL_NOT_FOUND", "channel not found"));
        }
        Ok(json!({ "ok": true }))
    }

    pub async fn fetch_channel_models(&self, payload: &Value) -> ApiResult<Value> {
        // Best-effort: query the OpenAI-compatible /models endpoint.
        let base = payload
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let key = payload.get("apiKey").and_then(|v| v.as_str()).unwrap_or("");
        if base.is_empty() {
            return Ok(json!({ "models": [] }));
        }
        let url = format!("{}/models", base.trim_end_matches('/'));
        let resp = crate::infra::http::shared_client()
            .get(url)
            .timeout(std::time::Duration::from_secs(15))
            .bearer_auth(key)
            .send()
            .await;
        match resp {
            Ok(r) => {
                let body: Value = r.json().await.unwrap_or(json!({}));
                let models: Vec<Value> = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                            .map(|id| json!({ "id": id, "label": id }))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(json!({ "models": models }))
            }
            Err(e) => Err(ApiError::new("PROVIDER_HTTP_ERROR", e.to_string())),
        }
    }

    // ---------------- permissions / subagents / skills ----------------

    pub fn get_permission_mode(&self, session_id: &str) -> Value {
        json!({ "mode": self.permissions.mode(session_id) })
    }

    pub fn set_permission_mode(&self, session_id: &str, mode: &str) -> ApiResult<Value> {
        Ok(json!({ "mode": self.permissions.set_mode(session_id, mode)? }))
    }

    pub fn list_builtin_subagents(&self) -> Value {
        json!({ "subagents": [
            { "id": "general", "name": "通用子代理", "description": "执行单个委派任务" },
            { "id": "explore", "name": "探索子代理", "description": "只读探索代码库" },
        ] })
    }

    /// Resolve a pending interactive tool-permission request.
    pub fn resolve_permission(&self, request_id: &str, approved: bool) -> ApiResult<Value> {
        if self.permissions.resolve(request_id, approved) {
            Ok(json!({ "ok": true, "requestId": request_id, "approved": approved }))
        } else {
            Err(ApiError::new(
                "PERMISSION_REQUEST_NOT_FOUND",
                format!("permission request not found or already resolved: {request_id}"),
            ))
        }
    }

    pub fn list_skills(&self) -> Value {
        json!({ "skills": discover_skills(&self.skill_dirs) })
    }

    pub fn read_skill_content(&self, name: &str) -> ApiResult<Value> {
        if !crate::tools::skill::is_safe_skill_name(name) {
            return Err(ApiError::new(
                "SKILL_NOT_FOUND",
                format!("invalid skill name: {name}"),
            ));
        }
        for dir in &self.skill_dirs {
            let p = dir.join(name).join("SKILL.md");
            if let Ok(content) = std::fs::read_to_string(&p) {
                return Ok(json!({ "name": name, "content": content }));
            }
        }
        Err(ApiError::new(
            "SKILL_NOT_FOUND",
            format!("skill not found: {name}"),
        ))
    }

    // ---------------- checkpoints ----------------

    pub fn list_checkpoints(&self, session_id: &str) -> Value {
        self.checkpoints.list(session_id)
    }

    pub fn create_checkpoint(
        &self,
        session_id: &str,
        paths: Option<Vec<String>>,
        label: &str,
    ) -> Value {
        self.checkpoints.create(session_id, paths, label)
    }

    pub fn rewind_checkpoint(&self, id: &str) -> ApiResult<Value> {
        self.checkpoints.rewind(id)
    }

    // ---------------- chat / plan ----------------

    pub async fn ask_execute(
        &self,
        session_id: &str,
        user_input: &str,
        _context_window: Option<&Value>,
        composer_mode: Option<&str>,
    ) -> ApiResult<Value> {
        self.sessions.auto_title_from_input(session_id, user_input);
        let runtime_mode = normalize_composer_mode(composer_mode);
        if composer_mode.is_some() && runtime_mode != "plan" {
            return self
                .runtime
                .run_composer_chat(session_id, user_input, &runtime_mode)
                .await;
        }
        // plan + execute
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

    pub async fn generate_plan(
        &self,
        session_id: &str,
        user_input: &str,
        _ctx: Option<&Value>,
    ) -> ApiResult<Value> {
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
        self.runtime.save_run(&run);
        plan.latest_execution_id = Some(run.id.clone());
        plan.status = "executing".to_string();
        self.plan_engine.save(&plan);
        self.sessions.set_active_run(
            &plan.session_id,
            Some(run.id.clone()),
            Some(plan_id.to_string()),
        );

        self.bus.emit(
            crate::contracts::events::EventDraft::new(
                &plan.session_id,
                "plan.execution.started",
                "plan",
            )
            .payload(json!({ "planId": plan_id, "runId": run.id })),
        );

        // Run the DAG to completion (parallel batches).
        let outcome = self
            .runtime
            .run_plan(&run.id, &plan.session_id, &model)
            .await?;

        // Reflect the real outcome instead of always claiming success.
        use crate::domain::runtime::PlanOutcome;
        let (run_status, plan_status) = match outcome {
            PlanOutcome::Completed => ("completed", "completed"),
            PlanOutcome::Cancelled => ("cancelled", "cancelled"),
            PlanOutcome::PartialFailure => ("failed", "failed"),
        };
        let mut finished = self.runtime.get_run(&run.id).unwrap_or(run);
        finished.status = run_status.to_string();
        finished.updated_at = now_ts();
        self.runtime.save_run(&finished);
        plan.status = plan_status.to_string();
        self.plan_engine.save(&plan);
        self.sessions.set_active_run(&plan.session_id, None, None);

        self.bus.emit(
            crate::contracts::events::EventDraft::new(
                &plan.session_id,
                "plan.execution.completed",
                "plan",
            )
            .payload(json!({
                "planId": plan_id,
                "runId": finished.id,
                "outcome": run_status,
                "completedTodos": finished.completed_todo_ids.len(),
                "failedTodos": finished.failed_todo_ids.len(),
            })),
        );
        Ok(json!({ "run": finished, "plan": plan }))
    }

    // ---------------- todos ----------------

    pub fn get_todos(&self, session_id: &str) -> Value {
        json!({ "todos": self.todos.list_by_session(session_id) })
    }

    pub fn add_todo(&self, payload: &Value) -> ApiResult<Value> {
        Ok(json!({ "todo": self.todos.add_from_payload(payload)? }))
    }

    pub fn patch_todo(&self, id: &str, patch: &Value) -> ApiResult<Value> {
        let todo = self.todos.patch(id, patch)?;
        // Manual REST edits surface in the event stream too, so the checklist
        // UI stays consistent with agent-driven updates.
        self.bus.emit(
            crate::contracts::events::EventDraft::new(&todo.session_id, "todo.updated", "todo")
                .payload(
                    json!({ "id": todo.id, "title": todo.title, "kind": todo.kind, "status": todo.status }),
                ),
        );
        Ok(json!({ "todo": todo }))
    }

    pub fn batch_rerun_todos(&self, payload: &Value) -> ApiResult<Value> {
        let ids = payload
            .get("todoIds")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in &ids {
            self.todos.set_status(id, "queued");
        }
        Ok(json!({ "ok": true, "requeued": ids }))
    }

    // ---------------- runs ----------------

    pub fn get_run(&self, run_id: &str) -> ApiResult<Value> {
        Ok(json!({ "run": self.runtime.get_run(run_id)? }))
    }

    pub fn get_run_logs(&self, run_id: &str) -> ApiResult<Value> {
        let run = self.runtime.get_run(run_id)?;
        let events = self.bus.snapshot(&run.session_id);
        let logs: Vec<Value> = events
            .into_iter()
            .filter(|e| e.correlation_id.as_deref() == Some(run_id))
            .map(|e| e.to_wire())
            .collect();
        Ok(json!({ "logs": logs }))
    }

    pub fn get_run_metrics(&self, run_id: &str) -> ApiResult<Value> {
        let run = self.runtime.get_run(run_id)?;
        Ok(json!({ "metrics": {
            "runId": run.id,
            "status": run.status,
            "completedTodos": run.completed_todo_ids.len(),
            "failedTodos": run.failed_todo_ids.len(),
        } }))
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

    pub fn rerun_todo(&self, _run_id: &str, todo_id: &str) -> ApiResult<Value> {
        self.todos.set_status(todo_id, "queued");
        Ok(json!({ "ok": true, "todoId": todo_id }))
    }

    pub fn rerun_node(&self, _run_id: &str, node_id: &str) -> ApiResult<Value> {
        Ok(json!({ "ok": true, "nodeId": node_id }))
    }

    // ---------------- proposals (not modelled separately; empty) ----------------

    pub fn list_proposals(&self, _session_id: &str) -> ApiResult<Value> {
        Ok(json!({ "proposals": [] }))
    }
    pub fn apply_proposal(&self, id: &str) -> ApiResult<Value> {
        Err(ApiError::new(
            "PROPOSAL_NOT_FOUND",
            format!("proposal not found: {id}"),
        ))
    }
    pub fn discard_proposal(&self, id: &str) -> ApiResult<Value> {
        Err(ApiError::new(
            "PROPOSAL_NOT_FOUND",
            format!("proposal not found: {id}"),
        ))
    }

    // ---------------- replay / swarm / workspace / tools ----------------

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

    pub fn get_design_snapshot(&self, session_id: Option<&str>) -> Value {
        match session_id {
            Some(sid) => {
                let events: Vec<Value> = self
                    .bus
                    .snapshot(sid)
                    .into_iter()
                    .map(|e| e.to_wire())
                    .collect();
                json!({ "sessionId": sid, "events": events, "latestSeq": self.bus.latest_seq(sid) })
            }
            None => json!({ "sessions": self.sessions.list() }),
        }
    }

    pub fn list_swarm_state(&self) -> Value {
        json!({ "nodes": [], "shards": [] })
    }
    pub fn seed_swarm_demo(&self) -> Value {
        json!({ "ok": true, "seeded": 0 })
    }

    pub fn get_workspace_info(&self) -> Value {
        let root = self.runtime.workspace_root();
        json!({ "workspaceRoot": root.to_string_lossy(), "exists": root.exists() })
    }

    pub fn get_workspace_tree(&self, path: &str, _show_heavy: bool) -> ApiResult<Value> {
        let root = self.runtime.workspace_root();
        let base = if path.trim().is_empty() {
            root.clone()
        } else {
            crate::tools::resolve_in_root(&root, path)?
        };
        let mut entries = Vec::new();
        let rd = std::fs::read_dir(&base).map_err(|_| ApiError::path_not_found(path))?;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(json!({ "name": name, "type": if is_dir { "dir" } else { "file" } }));
        }
        Ok(json!({ "path": path, "entries": entries }))
    }

    /// Resolve a caller-supplied filesystem path. Unless
    /// `AGENT_DEBUG_ALLOW_LOCAL_FS` is set, paths are confined to the
    /// workspace root (closes the arbitrary read/write hole).
    fn local_fs_target(&self, path: &str) -> ApiResult<PathBuf> {
        if self.cfg.allow_local_fs {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid_path(path));
            }
            Ok(PathBuf::from(trimmed))
        } else {
            crate::tools::resolve_in_root(&self.runtime.workspace_root(), path)
        }
    }

    pub fn browse_directories(&self, path: &str) -> ApiResult<Value> {
        let base = if path.trim().is_empty() {
            self.runtime.workspace_root()
        } else {
            self.local_fs_target(path)?
        };
        let rd = std::fs::read_dir(&base).map_err(|_| ApiError::path_not_found(path))?;
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                dirs.push(e.file_name().to_string_lossy().to_string());
            }
        }
        dirs.sort();
        Ok(json!({ "path": base.to_string_lossy(), "directories": dirs }))
    }

    pub fn set_workspace_root(&self, path: &str) -> ApiResult<Value> {
        let p = PathBuf::from(path.trim());
        if !p.is_dir() {
            return Err(ApiError::new("PATH_NOT_DIRECTORY", "not a directory"));
        }
        let _ = self.store.kv_put("workspace_root", path.trim());
        Ok(json!({ "workspaceRoot": path.trim() }))
    }

    pub fn read_workspace_file(&self, path: &str) -> ApiResult<Value> {
        let abs = crate::tools::resolve_in_root(&self.runtime.workspace_root(), path)?;
        let content = std::fs::read_to_string(&abs).map_err(|_| ApiError::path_not_found(path))?;
        Ok(json!({ "path": path, "content": content }))
    }

    pub fn write_workspace_file(&self, path: &str, content: &str) -> ApiResult<Value> {
        let abs = crate::tools::resolve_in_root(&self.runtime.workspace_root(), path)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(json!({ "ok": true, "path": path }))
    }

    pub fn read_local_file(&self, path: &str) -> ApiResult<Value> {
        let abs = self.local_fs_target(path)?;
        let content =
            std::fs::read_to_string(&abs).map_err(|_| ApiError::path_not_found(path))?;
        Ok(json!({ "path": path, "content": content }))
    }

    pub fn write_local_file(&self, path: &str, content: &str) -> ApiResult<Value> {
        let abs = self.local_fs_target(path)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(json!({ "ok": true, "path": path }))
    }

    pub fn revert_workspace_file(&self, path: &str) -> ApiResult<Value> {
        // Without VCS integration we report not-a-git-repo (matches error contract).
        Err(ApiError::new(
            "NOT_A_GIT_REPO",
            format!("cannot revert {path}: no VCS integration"),
        ))
    }

    pub fn list_tools(&self) -> Value {
        json!({ "tools": self.tools.describe() })
    }

    pub fn mcp_demo_status(&self) -> Value {
        json!({ "ok": false, "error": { "code": "MCP_NOT_INSTALLED", "message": "MCP demo not available in the Rust backend" } })
    }

    pub fn mcp_demo_call(&self, _name: &str, _args: Option<&Value>) -> Value {
        json!({ "ok": false, "error": { "code": "MCP_NOT_INSTALLED", "message": "MCP demo not available in the Rust backend" } })
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

// Keep T_RUNS referenced for clarity in this module.
const _: &str = T_RUNS;
