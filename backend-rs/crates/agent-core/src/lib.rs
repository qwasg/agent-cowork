//! Domain layer: sessions, auth, permissions, plan/todo engines, runtime.

pub mod auth;
pub mod checkpoint;
pub mod code_edit;
pub mod context;
pub mod engine;
pub mod hooks;
pub mod memory;
pub mod permission;
pub mod plan;
pub mod profile;
pub mod prompts;
pub mod session;

/// Legacy module path: the runtime was split into [`engine`] submodules.
pub use engine as runtime;
pub mod subagents;
pub mod swarm;
pub mod todo;
pub mod workspace;

pub use auth::AuthService;
pub use checkpoint::CheckpointService;
pub use code_edit::ProposalRegistry;
pub use engine::{PlanOutcome, RunControl, Runtime};
pub use memory::MemoryService;
pub use permission::PermissionService;
pub use plan::PlanEngine;
pub use profile::{AgentKind, AgentProfile};
pub use session::SessionService;

// Search config now lives in `agent-tools` (the web tools consume it); keep the
// historical `agent_core::search_config` / `SearchConfigService` paths working.
pub use agent_tools::search_config;
pub use agent_tools::search_config::SearchConfigService;
pub use swarm::SwarmCoordinator;
pub use todo::TodoEngine;
