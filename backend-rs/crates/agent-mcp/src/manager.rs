//! Generic MCP client manager: long-lived connections to servers declared in
//! `data/mcp.json`, stdio and streamable-HTTP transports, and dynamic tool
//! discovery exposed to the agent loop as `mcp__{server}__{tool}`.
//!
//! ```json
//! {
//!   "servers": {
//!     "demo":  { "command": "path/to/mcp-demo-server", "args": [], "env": {} },
//!     "remote": { "url": "http://127.0.0.1:9000/mcp" }
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{McpStdioClient, PROTOCOL_VERSION};

/// One server entry in `mcp.json`. Exactly one of `command` / `url` is used.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for McpServerConfig {
    fn default() -> Self {
        McpServerConfig {
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tool_count: usize,
    pub transport: String,
}

enum Transport {
    Stdio(Mutex<McpStdioClient>),
    Http(McpHttpClient),
}

struct Connection {
    transport: Transport,
    /// Raw MCP tool descriptors from `tools/list`.
    tools: Vec<Value>,
}

pub struct McpManager {
    config_path: PathBuf,
    connections: RwLock<HashMap<String, Arc<Connection>>>,
}

impl McpManager {
    pub fn new(config_path: PathBuf) -> Arc<Self> {
        Arc::new(McpManager {
            config_path,
            connections: RwLock::new(HashMap::new()),
        })
    }

    /// Read `mcp.json` (missing file ⇒ empty config).
    pub fn load_config(&self) -> McpConfig {
        let Ok(raw) = std::fs::read_to_string(&self.config_path) else {
            return McpConfig::default();
        };
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(
                "mcp: failed to parse {}: {e} — ignoring config",
                self.config_path.display()
            );
            McpConfig::default()
        })
    }

    /// Persist a new config (used by `PUT /mcp/servers`).
    pub fn save_config(&self, config: &McpConfig) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, raw).map_err(|e| e.to_string())
    }

    /// Bootstrap: when no `mcp.json` exists and the in-repo demo server
    /// binary is available, write a default config exposing it as `demo`
    /// (keeps the legacy `mcp__demo__*` tool names working).
    pub fn ensure_default_config(&self) {
        if self.config_path.exists() {
            return;
        }
        let Some((cmd, args)) = crate::demo_command() else {
            return;
        };
        let mut servers = HashMap::new();
        servers.insert(
            "demo".to_string(),
            McpServerConfig {
                command: Some(cmd),
                args,
                ..Default::default()
            },
        );
        if let Err(e) = self.save_config(&McpConfig { servers }) {
            tracing::warn!("mcp: failed to write default config: {e}");
        }
    }

    /// (Re)connect every enabled server in the config; disconnect servers
    /// that were removed. Returns per-server statuses.
    pub async fn reload(self: &Arc<Self>) -> Vec<ServerStatus> {
        let config = self.load_config();
        // Drop connections for servers no longer configured/enabled.
        {
            let mut conns = self.connections.write().unwrap();
            conns.retain(|name, _| config.servers.get(name).is_some_and(|c| c.enabled));
        }
        let mut statuses = Vec::new();
        for (name, cfg) in &config.servers {
            if !cfg.enabled {
                statuses.push(ServerStatus {
                    name: name.clone(),
                    ok: false,
                    error: Some("disabled".to_string()),
                    tool_count: 0,
                    transport: transport_kind(cfg),
                });
                continue;
            }
            statuses.push(self.reload_server(name).await);
        }
        statuses
    }

    /// (Re)connect a single server by name.
    pub async fn reload_server(self: &Arc<Self>, name: &str) -> ServerStatus {
        let config = self.load_config();
        let Some(cfg) = config.servers.get(name).cloned() else {
            return ServerStatus {
                name: name.to_string(),
                ok: false,
                error: Some("server not in mcp.json".to_string()),
                tool_count: 0,
                transport: "unknown".to_string(),
            };
        };
        let transport = transport_kind(&cfg);
        match connect(&cfg).await {
            Ok(conn) => {
                let count = conn.tools.len();
                self.connections
                    .write()
                    .unwrap()
                    .insert(name.to_string(), Arc::new(conn));
                ServerStatus {
                    name: name.to_string(),
                    ok: true,
                    error: None,
                    tool_count: count,
                    transport,
                }
            }
            Err(e) => {
                self.connections.write().unwrap().remove(name);
                ServerStatus {
                    name: name.to_string(),
                    ok: false,
                    error: Some(e),
                    tool_count: 0,
                    transport,
                }
            }
        }
    }

    /// All discovered tools as provider `ToolSpec`s (`mcp__{server}__{tool}`).
    pub fn tool_specs(&self) -> Vec<agent_providers::types::ToolSpec> {
        let conns = self.connections.read().unwrap();
        let mut specs = Vec::new();
        for (server, conn) in conns.iter() {
            for t in &conn.tools {
                let Some(tool) = t.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                specs.push(agent_providers::types::ToolSpec {
                    name: format!("mcp__{server}__{tool}"),
                    description: format!(
                        "[MCP {server}] {}",
                        t.get("description").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    parameters: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(json!({ "type": "object", "properties": {} })),
                });
            }
        }
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    /// Connection/tool overview for `GET /mcp/servers`.
    pub fn status(&self) -> Value {
        let config = self.load_config();
        let conns = self.connections.read().unwrap();
        let servers: Vec<Value> = config
            .servers
            .iter()
            .map(|(name, cfg)| {
                let conn = conns.get(name);
                json!({
                    "name": name,
                    "transport": transport_kind(cfg),
                    "enabled": cfg.enabled,
                    "connected": conn.is_some(),
                    "tools": conn.map(|c| c.tools.clone()).unwrap_or_default(),
                })
            })
            .collect();
        json!({ "servers": servers })
    }

    /// Call `tool` on `server`; reconnects and retries once when the
    /// long-lived connection went stale.
    pub async fn call_tool(
        self: &Arc<Self>,
        server: &str,
        tool: &str,
        args: Value,
    ) -> Result<String, String> {
        let conn = self
            .connections
            .read()
            .unwrap()
            .get(server)
            .cloned()
            .ok_or_else(|| format!("MCP server `{server}` 未连接"))?;
        match call_on(&conn, tool, args.clone()).await {
            Ok(text) => Ok(text),
            Err(first_err) => {
                tracing::warn!("mcp: call {server}/{tool} failed ({first_err}); reconnecting");
                let status = self.reload_server(server).await;
                if !status.ok {
                    return Err(first_err);
                }
                let conn = self
                    .connections
                    .read()
                    .unwrap()
                    .get(server)
                    .cloned()
                    .ok_or(first_err)?;
                call_on(&conn, tool, args).await
            }
        }
    }
}

fn transport_kind(cfg: &McpServerConfig) -> String {
    if cfg.url.is_some() {
        "http".to_string()
    } else {
        "stdio".to_string()
    }
}

async fn connect(cfg: &McpServerConfig) -> Result<Connection, String> {
    if let Some(url) = &cfg.url {
        let client = McpHttpClient::connect(url).await?;
        let tools = client.list_tools().await?;
        return Ok(Connection {
            transport: Transport::Http(client),
            tools,
        });
    }
    let Some(command) = cfg.command.clone() else {
        return Err("server config needs either `command` or `url`".to_string());
    };
    let args = cfg.args.clone();
    let env = cfg.env.clone();
    tokio::task::spawn_blocking(move || {
        let mut client = McpStdioClient::connect_with_env(&command, &args, &env)?;
        let tools = client.list_tools()?;
        Ok(Connection {
            transport: Transport::Stdio(Mutex::new(client)),
            tools,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn call_on(conn: &Arc<Connection>, tool: &str, args: Value) -> Result<String, String> {
    let result = match &conn.transport {
        Transport::Http(client) => client.call_tool(tool, args).await?,
        Transport::Stdio(_) => {
            let conn = conn.clone();
            let tool = tool.to_string();
            tokio::task::spawn_blocking(move || {
                let Transport::Stdio(client) = &conn.transport else {
                    unreachable!()
                };
                client.lock().unwrap().call_tool(&tool, args)
            })
            .await
            .map_err(|e| e.to_string())??
        }
    };
    extract_text(&result)
}

/// Flatten an MCP `tools/call` result into plain text (errors via `isError`).
fn extract_text(result: &Value) -> Result<String, String> {
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Err(if text.is_empty() {
            "MCP tool error".to_string()
        } else {
            text
        })
    } else {
        Ok(text)
    }
}

// ----------------------------------------------------------- HTTP transport

/// Minimal streamable-HTTP MCP client: JSON-RPC over POST; accepts both
/// `application/json` and `text/event-stream` responses; keeps the
/// `Mcp-Session-Id` header across requests.
pub struct McpHttpClient {
    url: String,
    http: reqwest::Client,
    next_id: AtomicI64,
    session_id: Mutex<Option<String>>,
}

impl McpHttpClient {
    pub async fn connect(url: &str) -> Result<Self, String> {
        let client = McpHttpClient {
            url: url.to_string(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())?,
            next_id: AtomicI64::new(0),
            session_id: Mutex::new(None),
        };
        let result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "agentd", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
            .await?;
        if result.get("protocolVersion").is_none() {
            return Err("MCP initialize 响应缺少 protocolVersion".to_string());
        }
        client
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    pub async fn list_tools(&self) -> Result<Vec<Value>, String> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default())
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
        .await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.post(&body).await.map(|_| ())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let resp = self.post(&body).await?;
        let msg = find_response(&resp, id)
            .ok_or_else(|| format!("MCP `{method}`: 响应中没有 id={id} 的结果"))?;
        if let Some(err) = msg.get("error") {
            return Err(err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("MCP error")
                .to_string());
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    /// POST the JSON-RPC body; returns all JSON-RPC messages found in the
    /// response (single object for JSON, one per `data:` line for SSE).
    async fn post(&self, body: &Value) -> Result<Vec<Value>, String> {
        let mut req = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(body);
        if let Some(sid) = self.session_id.lock().unwrap().clone() {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("MCP HTTP 请求失败: {e}"))?;
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().unwrap() = Some(sid.to_string());
        }
        let status = resp.status();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "MCP HTTP {status}: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        if content_type.contains("text/event-stream") {
            Ok(text
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
                .collect())
        } else if text.trim().is_empty() {
            Ok(Vec::new()) // 202 Accepted for notifications
        } else {
            serde_json::from_str::<Value>(&text)
                .map(|v| vec![v])
                .map_err(|e| format!("MCP 响应不是 JSON: {e}"))
        }
    }
}

fn find_response(messages: &[Value], id: i64) -> Option<&Value> {
    messages
        .iter()
        .find(|m| m.get("id").and_then(|v| v.as_i64()) == Some(id))
}
