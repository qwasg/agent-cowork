//! In-repo stdio MCP demo server (Rust rewrite of
//! `backend/tools/mcp_demo_server.py`).
//!
//! Speaks newline-delimited JSON-RPC 2.0 over stdio and exposes three
//! deterministic demo tools (`add` / `echo` / `reverse`) so the agent runtime
//! and the Settings → Tools panel can exercise the full MCP path without any
//! Python dependency.
//!
//! Run standalone for debugging:
//!
//! ```text
//! cargo run --bin mcp-demo-server
//! ```

use std::io::{BufRead, Write};

use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "agent-debug-demo";

fn tools_catalog() -> Value {
    json!([
        {
            "name": "add",
            "description": "Add two numbers and return the sum.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "a": { "type": "number" },
                    "b": { "type": "number" }
                },
                "required": ["a", "b"]
            }
        },
        {
            "name": "echo",
            "description": "Echo the provided text back to the caller.",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }
        },
        {
            "name": "reverse",
            "description": "Reverse the provided text (useful for verifying tool round-trips).",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }
        }
    ])
}

fn text_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": is_error,
    })
}

fn call_tool(name: &str, args: &Value) -> Value {
    match name {
        "add" => {
            let (Some(a), Some(b)) = (
                args.get("a").and_then(Value::as_f64),
                args.get("b").and_then(Value::as_f64),
            ) else {
                return text_result("add requires numeric `a` and `b`", true);
            };
            let total = a + b;
            // Render integers without trailing .0 for nicer display.
            let rendered = if total.fract() == 0.0 && total.abs() < i64::MAX as f64 {
                format!("{}", total as i64)
            } else {
                format!("{total}")
            };
            text_result(format!("{a} + {b} = {rendered}"), false)
        }
        "echo" => match args.get("text").and_then(Value::as_str) {
            Some(text) => text_result(text, false),
            None => text_result("echo requires string `text`", true),
        },
        "reverse" => match args.get("text").and_then(Value::as_str) {
            Some(text) => text_result(text.chars().rev().collect::<String>(), false),
            None => text_result("reverse requires string `text`", true),
        },
        other => text_result(format!("unknown tool: {other}"), true),
    }
}

fn handle_request(method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools_catalog() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let default_args = json!({});
            let args = params.get("arguments").unwrap_or(&default_args);
            Ok(call_tool(name, args))
        }
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        // Notifications (no id) get no response.
        let Some(id) = id else {
            continue;
        };
        let default_params = json!({});
        let params = msg.get("params").unwrap_or(&default_params);
        let response = match handle_request(method, params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            }),
        };
        let mut out = stdout.lock();
        if writeln!(out, "{response}")
            .and_then(|_| out.flush())
            .is_err()
        {
            break;
        }
    }
}
