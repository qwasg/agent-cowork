//! Minimal stdio MCP (Model Context Protocol) client.
//!
//! Speaks newline-delimited JSON-RPC 2.0 over a child process's stdio —
//! enough protocol (`initialize` → `notifications/initialized` →
//! `tools/list` / `tools/call`) to drive the in-repo demo server (a Rust
//! rewrite of `backend/tools/mcp_demo_server.py`) and any other compliant
//! stdio server.
//!
//! Like the Python `demo_service`, each operation spawns a fresh server
//! process and tears it down afterwards: the demo server is tiny and this
//! keeps the client free of connection-lifetime state.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

pub mod manager;
pub use manager::{McpConfig, McpHttpClient, McpManager, McpServerConfig, ServerStatus};

pub(crate) const PROTOCOL_VERSION: &str = "2024-11-05";
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

pub struct McpStdioClient {
    child: Child,
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<Value>,
    next_id: i64,
}

impl McpStdioClient {
    /// Spawn `command args…` and run the MCP initialize handshake.
    pub fn connect(command: &str, args: &[String]) -> Result<Self, String> {
        Self::connect_with_env(command, args, &std::collections::HashMap::new())
    }

    /// Like [`Self::connect`] but with extra environment variables.
    pub fn connect_with_env(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("无法启动 MCP server `{command}`: {e}"))?;
        let stdin = child.stdin.take().ok_or("无法获取 MCP server stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取 MCP server stdout")?;

        // Reader thread: parse one JSON value per line, forward over channel.
        let (tx, rx) = mpsc::channel::<Value>();
        std::thread::Builder::new()
            .name("mcp-stdio-reader".to_string())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        if tx.send(v).is_err() {
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("无法创建 MCP 读取线程: {e}"))?;

        let mut client = McpStdioClient {
            child,
            stdin,
            rx,
            next_id: 0,
        };
        client.initialize()?;
        Ok(client)
    }

    fn initialize(&mut self) -> Result<(), String> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "agentd", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        if result.get("protocolVersion").is_none() {
            return Err("MCP initialize 响应缺少 protocolVersion".to_string());
        }
        self.notify("notifications/initialized", json!({}))?;
        Ok(())
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        let line = format!("{value}\n");
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("MCP 写入失败: {e}"))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Issue a request and block (with timeout) for the matching response.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = std::time::Instant::now() + RPC_TIMEOUT;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| format!("MCP `{method}` 超时"))?;
            let msg = self
                .rx
                .recv_timeout(remaining)
                .map_err(|_| format!("MCP `{method}` 超时或连接中断"))?;
            // Skip notifications / unrelated responses.
            if msg.get("id").and_then(|v| v.as_i64()) != Some(id) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                let message = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("MCP error");
                return Err(message.to_string());
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    pub fn list_tools(&mut self) -> Result<Vec<Value>, String> {
        let result = self.request("tools/list", json!({}))?;
        Ok(result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default())
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ------------------------------------------------------------------ demo

/// Resolve the demo server launch command: `AGENT_DEBUG_MCP_DEMO_CMD`
/// (whitespace-split) wins; otherwise look for the `mcp-demo-server` binary
/// next to the current executable (cargo places both in the same dir).
pub fn demo_command() -> Option<(String, Vec<String>)> {
    if let Ok(raw) = std::env::var("AGENT_DEBUG_MCP_DEMO_CMD") {
        let mut parts = raw.split_whitespace().map(String::from);
        let cmd = parts.next()?;
        return Some((cmd, parts.collect()));
    }
    let exe = std::env::current_exe().ok()?;
    let file = format!("mcp-demo-server{}", std::env::consts::EXE_SUFFIX);
    let candidates: Vec<PathBuf> = [
        exe.parent().map(|d| d.join(&file)),
        exe.parent().and_then(|d| d.parent()).map(|d| d.join(&file)),
    ]
    .into_iter()
    .flatten()
    .collect();
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| (p.to_string_lossy().to_string(), Vec::new()))
}

fn err_payload(code: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": { "code": code, "message": message.into() } })
}

/// `GET /mcp/demo/status` payload (Python `mcp_demo_status` shape).
pub fn demo_status() -> Value {
    let Some((cmd, args)) = demo_command() else {
        return err_payload(
            "MCP_DEMO_SERVER_MISSING",
            "Demo MCP server binary not found (build `mcp-demo-server` or set AGENT_DEBUG_MCP_DEMO_CMD)",
        );
    };
    match McpStdioClient::connect(&cmd, &args).and_then(|mut c| c.list_tools()) {
        Ok(tools) => {
            let tools: Vec<Value> = tools
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "inputSchema": t.get("inputSchema").cloned().unwrap_or(json!({})),
                    })
                })
                .collect();
            json!({ "ok": true, "tools": tools })
        }
        Err(e) => err_payload("MCP_DEMO_FAILED", e),
    }
}

/// `POST /mcp/demo/call` payload (Python `mcp_demo_call` shape).
pub fn demo_call(name: &str, arguments: Value) -> Value {
    let name = name.trim();
    if name.is_empty() {
        return err_payload("MCP_INVALID_REQUEST", "tool name is required");
    }
    let Some((cmd, args)) = demo_command() else {
        return err_payload(
            "MCP_DEMO_SERVER_MISSING",
            "Demo MCP server binary not found (build `mcp-demo-server` or set AGENT_DEBUG_MCP_DEMO_CMD)",
        );
    };
    let result =
        McpStdioClient::connect(&cmd, &args).and_then(|mut c| c.call_tool(name, arguments));
    match result {
        Ok(res) => {
            let parts: Vec<Value> = res
                .get("content")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|block| {
                            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                json!({
                                    "type": "text",
                                    "text": block.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                                })
                            } else {
                                json!({
                                    "type": block.get("type").and_then(|t| t.as_str()).unwrap_or("unknown"),
                                    "repr": block.to_string(),
                                })
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            if res
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                let msg = parts
                    .first()
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("tool error")
                    .to_string();
                return err_payload("MCP_TOOL_ERROR", msg);
            }
            let mut payload = json!({ "ok": true, "result": { "content": parts } });
            if let Some(structured) = res.get("structuredContent") {
                if !structured.is_null() {
                    payload["result"]["structured"] = structured.clone();
                }
            }
            payload
        }
        Err(e) => err_payload("MCP_TOOL_ERROR", e),
    }
}
