//! Tool catalog + MCP endpoints (generic servers + legacy demo).

use serde_json::{json, Value};

use agent_mcp::McpConfig;
use agent_protocol::{ApiError, ApiResult};

use crate::api::AppServices;

impl AppServices {
    pub fn list_tools(&self) -> Value {
        let mut items = self.tools.describe();
        for spec in self.runtime.mcp.tool_specs() {
            items.push(json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.parameters,
            }));
        }
        json!({ "items": items.clone(), "tools": items })
    }

    /// `GET /mcp/servers` — config + connection + tool overview.
    pub fn mcp_servers(&self) -> Value {
        self.runtime.mcp.status()
    }

    /// `PUT /mcp/servers` — replace `mcp.json` and reconnect everything.
    pub async fn mcp_set_servers(&self, body: &Value) -> ApiResult<Value> {
        let config: McpConfig = serde_json::from_value(body.clone())
            .map_err(|e| ApiError::new("MCP_INVALID_CONFIG", format!("invalid mcp.json: {e}")))?;
        self.runtime
            .mcp
            .save_config(&config)
            .map_err(|e| ApiError::new("MCP_INVALID_CONFIG", e))?;
        let statuses = self.runtime.mcp.reload().await;
        Ok(json!({ "ok": true, "statuses": statuses }))
    }

    /// `POST /mcp/servers/{name}:reload` — reconnect one server.
    pub async fn mcp_reload_server(&self, name: &str) -> Value {
        let status = self.runtime.mcp.reload_server(name).await;
        json!({ "ok": status.ok, "status": status })
    }

    pub async fn mcp_demo_status(&self) -> Value {
        tokio::task::spawn_blocking(agent_mcp::demo_status)
            .await
            .unwrap_or_else(|e| {
                json!({ "ok": false, "error": { "code": "MCP_DEMO_FAILED", "message": e.to_string() } })
            })
    }

    pub async fn mcp_demo_call(&self, name: &str, args: Option<&Value>) -> Value {
        let name = name.to_string();
        let arguments = args.cloned().unwrap_or(json!({}));
        tokio::task::spawn_blocking(move || agent_mcp::demo_call(&name, arguments))
            .await
            .unwrap_or_else(|e| {
                json!({ "ok": false, "error": { "code": "MCP_TOOL_ERROR", "message": e.to_string() } })
            })
    }
}
