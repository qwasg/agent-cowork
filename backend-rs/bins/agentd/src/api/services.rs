//! Dependency-injection container: wires all domain services together.
//! The REST operations live in per-domain `impl AppServices` blocks under
//! [`crate::api::handlers`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use agent_config::Config;
use agent_core::{
    AuthService, CheckpointService, PermissionService, PlanEngine, ProposalRegistry, Runtime,
    SearchConfigService, SessionService, SwarmCoordinator, TodoEngine,
};
use agent_providers::ProviderExecutionService;
use agent_store::{AsyncStore, CryptoStore, EventBus, JsonlStore, RolloutStore, Store};
use agent_tools::ToolRegistry;

pub struct AppServices {
    pub cfg: Config,
    pub store: Arc<Store>,
    /// Async write facade over `store` (dedicated writer thread); async
    /// handlers must use this instead of blocking the runtime on commits.
    pub astore: Arc<AsyncStore>,
    pub bus: Arc<EventBus>,
    pub crypto: Arc<CryptoStore>,
    pub providers: Arc<ProviderExecutionService>,
    pub sessions: Arc<SessionService>,
    pub permissions: Arc<PermissionService>,
    pub todos: Arc<TodoEngine>,
    pub auth: Arc<AuthService>,
    pub checkpoints: Arc<CheckpointService>,
    pub search_config: Arc<SearchConfigService>,
    pub tools: Arc<ToolRegistry>,
    pub runtime: Arc<Runtime>,
    pub plan_engine: Arc<PlanEngine>,
    pub proposals: Arc<ProposalRegistry>,
    pub swarm: Arc<SwarmCoordinator>,
    pub skill_dirs: Vec<PathBuf>,
    /// Latest editor context window per session (`ask:execute` /
    /// `plan:generate` payloads) — feeds proposals + snapshot.
    context_windows: Mutex<HashMap<String, Value>>,
}

impl AppServices {
    pub fn build(cfg: Config) -> anyhow::Result<Arc<Self>> {
        let store = Arc::new(Store::open(cfg.data_dir.join("agentd.redb"))?);
        let astore = AsyncStore::new(store.clone());
        // `persist_events=false` disables the durable JSONL log entirely.
        let jsonl = if cfg.persist_events {
            Some(Arc::new(JsonlStore::new(cfg.session_dir.clone())))
        } else {
            None
        };
        let bus =
            EventBus::with_broadcast_cap(cfg.event_buffer_cap, cfg.event_broadcast_cap, jsonl);
        let crypto = CryptoStore::open(cfg.data_dir.join(".agent_master.key"));
        let providers = ProviderExecutionService::build(&cfg, &store, &crypto);

        let rollout = Arc::new(RolloutStore::new(cfg.session_dir.clone()));
        let sessions = Arc::new(SessionService::new(
            store.clone(),
            rollout,
            cfg.history_turns,
        ));
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

        let skill_dirs = agent_tools::skill::configured_skill_dirs(&[
            workspace_root.clone(),
            cfg.workspace_root.clone(),
        ]);

        let search_config = SearchConfigService::new(store.clone(), crypto.clone(), &cfg);
        let proposals = Arc::new(ProposalRegistry::new(store.clone()));
        let tools = Arc::new(ToolRegistry::build_with_limits(
            true,
            agent_tools::OutputLimits {
                max_chars: cfg.tool_output_max_chars,
                max_lines: cfg.tool_output_max_lines,
            },
        ));
        let runtime = Runtime::new(
            &cfg,
            providers.clone(),
            tools.clone(),
            bus.clone(),
            sessions.clone(),
            permissions.clone(),
            todos.clone(),
            proposals.clone(),
            store.clone(),
            astore.clone(),
            skill_dirs.clone(),
            search_config.clone(),
        );
        let plan_engine = Arc::new(PlanEngine::new(
            providers.clone(),
            store.clone(),
            bus.clone(),
            todos.clone(),
            sessions.clone(),
        ));
        let swarm = Arc::new(SwarmCoordinator::new());

        // MCP bootstrap: seed mcp.json with the demo server when absent,
        // then connect every configured server in the background.
        runtime.mcp.ensure_default_config();
        {
            let mcp = runtime.mcp.clone();
            tokio::spawn(async move {
                for status in mcp.reload().await {
                    if status.ok {
                        tracing::info!(
                            "mcp: connected `{}` ({} tools, {})",
                            status.name,
                            status.tool_count,
                            status.transport
                        );
                    } else {
                        tracing::warn!(
                            "mcp: failed to connect `{}`: {}",
                            status.name,
                            status.error.unwrap_or_default()
                        );
                    }
                }
            });
        }

        Ok(Arc::new(AppServices {
            cfg,
            store,
            astore,
            bus,
            crypto,
            providers,
            sessions,
            permissions,
            todos,
            auth,
            checkpoints,
            search_config,
            tools,
            runtime,
            plan_engine,
            proposals,
            swarm,
            skill_dirs,
            context_windows: Mutex::new(HashMap::new()),
        }))
    }

    /// Remember the latest editor context window for a session.
    pub(crate) fn remember_context_window(&self, session_id: &str, ctx: Option<&Value>) {
        if let Some(ctx) = ctx {
            if ctx.is_object() {
                self.context_windows
                    .lock()
                    .unwrap()
                    .insert(session_id.to_string(), ctx.clone());
            }
        }
    }

    pub(crate) fn context_window_for(&self, session_id: &str) -> Option<Value> {
        self.context_windows
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
    }

    pub(crate) fn forget_context_window(&self, session_id: &str) {
        self.context_windows.lock().unwrap().remove(session_id);
    }
}
