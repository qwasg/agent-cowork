//! OpenAPI 3.1 document for every REST route, served at
//! `GET /api/agent-debug/openapi.json`.
//!
//! Handlers in this service exchange dynamic JSON (`serde_json::Value`), so
//! instead of deriving schemas per handler the document is assembled from a
//! route table using `utoipa`'s builder API. The table below is the single
//! place to update when routes change; a test in `tests.rs` cross-checks it
//! against the axum router.

use std::collections::BTreeMap;

use serde_json::Value;
use utoipa::openapi::path::{HttpMethod, OperationBuilder, ParameterBuilder, ParameterIn};
use utoipa::openapi::{InfoBuilder, OpenApiBuilder, PathsBuilder, Required, ResponseBuilder};

use crate::api::AppServices;

/// `(method, path, tag, summary)` — one row per operation.
pub const ROUTES: &[(&str, &str, &str, &str)] = &[
    (
        "get",
        "/health",
        "meta",
        "Liveness probe + store write-failure counter",
    ),
    // auth
    (
        "post",
        "/api/agent-debug/auth/register",
        "auth",
        "Register a new user",
    ),
    (
        "post",
        "/api/agent-debug/auth/login",
        "auth",
        "Login, returns bearer token",
    ),
    (
        "get",
        "/api/agent-debug/auth/me",
        "auth",
        "Current user from bearer token",
    ),
    (
        "patch",
        "/api/agent-debug/auth/profile",
        "auth",
        "Update profile fields",
    ),
    (
        "post",
        "/api/agent-debug/auth/logout",
        "auth",
        "Logout (stateless ack)",
    ),
    // sessions
    (
        "get",
        "/api/agent-debug/sessions",
        "sessions",
        "List sessions",
    ),
    (
        "post",
        "/api/agent-debug/sessions",
        "sessions",
        "Create a session",
    ),
    (
        "get",
        "/api/agent-debug/design-snapshot",
        "sessions",
        "Aggregated UI snapshot",
    ),
    (
        "get",
        "/api/agent-debug/sessions/{id}",
        "sessions",
        "Get a session",
    ),
    (
        "patch",
        "/api/agent-debug/sessions/{id}",
        "sessions",
        "Patch session fields",
    ),
    (
        "delete",
        "/api/agent-debug/sessions/{id}",
        "sessions",
        "Delete a session",
    ),
    (
        "post",
        "/api/agent-debug/sessions/{id}",
        "sessions",
        "Session action (`{id}:fork` / `{id}:revert`)",
    ),
    (
        "put",
        "/api/agent-debug/sessions/{id}/model",
        "sessions",
        "Select session model",
    ),
    // models / providers
    (
        "get",
        "/api/agent-debug/models",
        "models",
        "List available models",
    ),
    (
        "get",
        "/api/agent-debug/provider-status",
        "models",
        "Provider circuit-breaker status",
    ),
    (
        "get",
        "/api/agent-debug/search-config",
        "models",
        "Get web-search config",
    ),
    (
        "put",
        "/api/agent-debug/search-config",
        "models",
        "Set web-search config",
    ),
    (
        "get",
        "/api/agent-debug/model-preferences",
        "models",
        "Get model preferences",
    ),
    (
        "put",
        "/api/agent-debug/model-preferences",
        "models",
        "Set model preferences",
    ),
    // channels
    (
        "get",
        "/api/agent-debug/provider-types",
        "channels",
        "List provider types",
    ),
    (
        "get",
        "/api/agent-debug/channels",
        "channels",
        "List provider channels",
    ),
    (
        "post",
        "/api/agent-debug/channels",
        "channels",
        "Create a provider channel",
    ),
    (
        "post",
        "/api/agent-debug/channels:fetch-models",
        "channels",
        "Fetch models from a channel",
    ),
    (
        "put",
        "/api/agent-debug/channels/{id}",
        "channels",
        "Update a channel",
    ),
    (
        "delete",
        "/api/agent-debug/channels/{id}",
        "channels",
        "Delete a channel",
    ),
    // permissions
    (
        "get",
        "/api/agent-debug/sessions/{id}/permission-mode",
        "permissions",
        "Get session permission mode",
    ),
    (
        "put",
        "/api/agent-debug/sessions/{id}/permission-mode",
        "permissions",
        "Set session permission mode",
    ),
    (
        "get",
        "/api/agent-debug/permissions/rules",
        "permissions",
        "Get allow/deny/ask rule list",
    ),
    (
        "put",
        "/api/agent-debug/permissions/rules",
        "permissions",
        "Replace allow/deny/ask rule list",
    ),
    (
        "post",
        "/api/agent-debug/permissions/{seg}",
        "permissions",
        "Resolve approval (`{id}:approve` / `{id}:deny`)",
    ),
    // hooks / shells
    (
        "get",
        "/api/agent-debug/hooks",
        "hooks",
        "Parsed hooks.json configuration",
    ),
    (
        "get",
        "/api/agent-debug/shells",
        "shells",
        "List background shell jobs",
    ),
    (
        "get",
        "/api/agent-debug/shells/{id}/output",
        "shells",
        "Read shell output from byte offset",
    ),
    (
        "post",
        "/api/agent-debug/shells/{seg}",
        "shells",
        "Shell action (`{id}:kill`)",
    ),
    // meta
    (
        "get",
        "/api/agent-debug/openapi.json",
        "meta",
        "This document",
    ),
    (
        "get",
        "/api/agent-debug/subagents",
        "subagents",
        "List subagent profiles",
    ),
    ("get", "/api/agent-debug/skills", "skills", "List skills"),
    (
        "get",
        "/api/agent-debug/skills/{name}",
        "skills",
        "Read a skill body",
    ),
    // checkpoints
    (
        "get",
        "/api/agent-debug/sessions/{id}/checkpoints",
        "checkpoints",
        "List checkpoints",
    ),
    (
        "post",
        "/api/agent-debug/sessions/{id}/checkpoints",
        "checkpoints",
        "Create a checkpoint",
    ),
    (
        "post",
        "/api/agent-debug/checkpoints/{id}",
        "checkpoints",
        "Checkpoint action (`{id}:rewind`)",
    ),
    // events
    (
        "get",
        "/api/agent-debug/sessions/{id}/events/stream",
        "events",
        "SSE event stream (`?fromSeq=`)",
    ),
    (
        "get",
        "/api/agent-debug/replay/{id}",
        "events",
        "Full event replay for a session",
    ),
    (
        "get",
        "/api/agent-debug/replay/{id}/since",
        "events",
        "Replay after `?fromSeq=` (gap-aware)",
    ),
    // mcp
    (
        "get",
        "/api/agent-debug/mcp/demo/status",
        "mcp",
        "Demo MCP server status",
    ),
    (
        "post",
        "/api/agent-debug/mcp/demo/call",
        "mcp",
        "Invoke a demo MCP tool",
    ),
    (
        "get",
        "/api/agent-debug/mcp/servers",
        "mcp",
        "List configured MCP servers",
    ),
    (
        "put",
        "/api/agent-debug/mcp/servers",
        "mcp",
        "Replace MCP server config",
    ),
    (
        "post",
        "/api/agent-debug/mcp/servers/{seg}",
        "mcp",
        "Server action (`{name}:reload`)",
    ),
    // chat / plans
    (
        "post",
        "/api/agent-debug/sessions/{id}/plan:generate",
        "plans",
        "Generate a plan from a goal",
    ),
    (
        "post",
        "/api/agent-debug/sessions/{id}/ask:execute",
        "plans",
        "Run a chat / agent turn",
    ),
    (
        "get",
        "/api/agent-debug/plans/{id}",
        "plans",
        "Get a plan bundle",
    ),
    (
        "patch",
        "/api/agent-debug/plans/{id}",
        "plans",
        "Patch plan stages/tasks",
    ),
    (
        "post",
        "/api/agent-debug/plans/{id}",
        "plans",
        "Plan action (`:confirm` / `:replan` / `:execute`)",
    ),
    // todos
    (
        "get",
        "/api/agent-debug/sessions/{id}/todos",
        "todos",
        "List session todos",
    ),
    ("post", "/api/agent-debug/todos", "todos", "Add a todo"),
    (
        "post",
        "/api/agent-debug/todos:batch-rerun",
        "todos",
        "Requeue several todos",
    ),
    (
        "patch",
        "/api/agent-debug/todos/{id}",
        "todos",
        "Patch a todo",
    ),
    // runs
    ("get", "/api/agent-debug/runs/{id}", "runs", "Get run state"),
    (
        "post",
        "/api/agent-debug/runs/{id}",
        "runs",
        "Run action (`:pause` / `:resume` / `:cancel` / `:steer`)",
    ),
    (
        "get",
        "/api/agent-debug/runs/{id}/logs",
        "runs",
        "Events correlated to a run",
    ),
    (
        "get",
        "/api/agent-debug/runs/{id}/metrics",
        "runs",
        "Usage / tool / compaction metrics",
    ),
    (
        "post",
        "/api/agent-debug/runs/{id}/todos/{seg}",
        "runs",
        "Rerun a todo within the run",
    ),
    (
        "post",
        "/api/agent-debug/runs/{id}/nodes/{seg}",
        "runs",
        "Rerun a plan node within the run",
    ),
    // proposals
    (
        "get",
        "/api/agent-debug/proposals",
        "proposals",
        "List edit proposals (`?sessionId=`)",
    ),
    (
        "post",
        "/api/agent-debug/proposals/{id}",
        "proposals",
        "Proposal action (`:apply` / `:discard`)",
    ),
    // swarm
    (
        "get",
        "/api/agent-debug/swarm/state",
        "swarm",
        "Swarm node graph state",
    ),
    (
        "post",
        "/api/agent-debug/swarm/seed-demo",
        "swarm",
        "Seed demo swarm data",
    ),
    // workspace
    (
        "get",
        "/api/agent-debug/workspace/info",
        "workspace",
        "Workspace root info",
    ),
    (
        "get",
        "/api/agent-debug/workspace/tree",
        "workspace",
        "Directory tree (`?path=`)",
    ),
    (
        "get",
        "/api/agent-debug/workspace/browse",
        "workspace",
        "Browse directories for the root picker",
    ),
    (
        "post",
        "/api/agent-debug/workspace/root",
        "workspace",
        "Change workspace root",
    ),
    (
        "get",
        "/api/agent-debug/workspace/file",
        "workspace",
        "Read a workspace file (`?path=`)",
    ),
    (
        "post",
        "/api/agent-debug/workspace/file",
        "workspace",
        "Write a workspace file",
    ),
    (
        "get",
        "/api/agent-debug/workspace/document",
        "workspace",
        "Read a workspace document IR/text (docx/pptx/pdf, `?path=`)",
    ),
    (
        "put",
        "/api/agent-debug/workspace/document",
        "workspace",
        "Write a workspace document from DocForge IR (docx/pptx)",
    ),
    (
        "post",
        "/api/agent-debug/workspace/revert",
        "workspace",
        "Revert a workspace file",
    ),
    (
        "get",
        "/api/agent-debug/local-file",
        "workspace",
        "Read a local file (if enabled)",
    ),
    (
        "post",
        "/api/agent-debug/local-file",
        "workspace",
        "Write a local file (if enabled)",
    ),
    // tools
    (
        "get",
        "/api/agent-debug/tools",
        "tools",
        "List registered tools",
    ),
];

fn http_method(m: &str) -> HttpMethod {
    match m {
        "get" => HttpMethod::Get,
        "put" => HttpMethod::Put,
        "post" => HttpMethod::Post,
        "delete" => HttpMethod::Delete,
        "patch" => HttpMethod::Patch,
        other => unreachable!("unsupported method in ROUTES table: {other}"),
    }
}

/// Path template params (`{id}`, `{seg}`, ...) extracted from the route path.
fn path_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(str::to_string)
        .collect()
}

impl AppServices {
    pub fn openapi_document(&self) -> Value {
        let mut paths = PathsBuilder::new();
        let mut grouped: BTreeMap<&str, Vec<&(&str, &str, &str, &str)>> = BTreeMap::new();
        for row in ROUTES {
            grouped.entry(row.1).or_default().push(row);
        }
        for (path, rows) in grouped {
            let mut item = utoipa::openapi::path::PathItemBuilder::new();
            for (method, _, tag, summary) in rows {
                let mut op = OperationBuilder::new()
                    .tag(*tag)
                    .summary(Some(*summary))
                    .response(
                        "200",
                        ResponseBuilder::new().description("JSON response").build(),
                    );
                for name in path_params(path) {
                    op = op.parameter(
                        ParameterBuilder::new()
                            .name(name)
                            .parameter_in(ParameterIn::Path)
                            .required(Required::True),
                    );
                }
                item = item.operation(http_method(method), op.build());
            }
            paths = paths.path(path, item.build());
        }
        let doc = OpenApiBuilder::new()
            .info(
                InfoBuilder::new()
                    .title("agentd")
                    .description(Some(
                        "Agent debug backend REST API. All bodies and responses are JSON; \
                         `{seg}` path segments encode `id:action` pairs.",
                    ))
                    .version(env!("CARGO_PKG_VERSION"))
                    .build(),
            )
            .paths(paths.build())
            .build();
        serde_json::to_value(doc).unwrap_or_default()
    }
}
