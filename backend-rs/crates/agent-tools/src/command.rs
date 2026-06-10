//! Command execution tool (port of `command_runner.py`). Runs a shell command
//! inside the workspace root with a timeout and bounded captured output.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

use crate::contracts::{ApiError, ApiResult};
use crate::tools::{AgentTool, ToolContext};

/// Hard ceiling for a single command (10 minutes) so an LLM-supplied
/// `timeoutMs` can't pin a worker indefinitely.
pub const MAX_TIMEOUT_MS: u64 = 600_000;

pub(crate) fn clamp_timeout_ms(requested: Option<u64>) -> u64 {
    requested.unwrap_or(60_000).clamp(1_000, MAX_TIMEOUT_MS)
}

pub struct RunCommand;

#[async_trait]
impl AgentTool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run a shell command in the workspace root and return its combined output."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "the command line to execute"},
                "timeoutMs": {"type": "integer", "description": "timeout in ms (default 60000)"}
            },
            "required": ["command"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "command required"));
        }
        let timeout_ms = clamp_timeout_ms(args.get("timeoutMs").and_then(|v| v.as_u64()));

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("powershell");
            c.arg("-NoProfile").arg("-Command").arg(&command);
            c
        } else {
            let mut c = tokio::process::Command::new("bash");
            c.arg("-lc").arg(&command);
            c
        };
        cmd.current_dir(&ctx.workspace_root);
        cmd.kill_on_drop(true);

        let fut = cmd.output();
        let output = match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(ApiError::new("COMMAND_ERROR", e.to_string())),
            Err(_) => return Err(ApiError::new("COMMAND_TIMEOUT", "command timed out")),
        };

        let mut combined = String::new();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            combined.push_str("\n[stderr]\n");
            combined.push_str(&stderr);
        }
        let code = output.status.code().unwrap_or(-1);
        let truncated: String = combined.chars().take(20_000).collect();
        Ok(format!("exit_code={code}\n{truncated}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_timeout() {
        assert_eq!(clamp_timeout_ms(None), 60_000);
        assert_eq!(clamp_timeout_ms(Some(10)), 1_000);
        assert_eq!(clamp_timeout_ms(Some(u64::MAX)), MAX_TIMEOUT_MS);
        assert_eq!(clamp_timeout_ms(Some(5_000)), 5_000);
    }
}
