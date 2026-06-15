//! Command execution tools backed by [`crate::shell::ShellManager`]:
//! `run_command` (timeout → background, named sessions), `shell_output`
//! (resume reading a job's spooled output) and `stop_command`.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{AgentTool, ToolContext};
use agent_protocol::{ApiError, ApiResult};

/// Hard ceiling for the foreground wait (10 minutes); the command itself
/// keeps running in the background after this.
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
        "在工作区执行一条 shell 命令（Windows 用 PowerShell，其它平台用 bash），用于构建、测试、git、\
         包管理等命令行操作。返回 exit_code 与合并的 stdout/stderr。超过等待时间（timeoutMs，默认 60 秒）\
         不会杀掉进程，而是转入后台并返回 shellId，可用 shell_output 续读输出；dev server 等常驻进程请\
         直接设 background=true。指定 session 名可在多次调用间保持工作目录（cd 会生效）。\
         读写、搜索文件请用专用工具而不是 cat/ls/grep。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "要执行的命令行"},
                "timeoutMs": {"type": "integer", "description": "前台等待毫秒数（默认 60000，上限 600000）；超时后命令转入后台继续运行"},
                "background": {"type": "boolean", "description": "true 时立即后台执行并返回 shellId（适合 dev server / watcher）"},
                "session": {"type": "string", "description": "具名持久 shell 会话：同名调用共享工作目录（cd 持久化）"}
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
        let background = args
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let wait_ms = if background {
            0
        } else {
            clamp_timeout_ms(args.get("timeoutMs").and_then(|v| v.as_u64()))
        };
        let session = args
            .get("session")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let outcome = ctx
            .shell
            .run(&command, &ctx.workspace_root, wait_ms, session)
            .await
            .map_err(|e| ApiError::new("COMMAND_ERROR", e.to_string()))?;

        match outcome.exit_code {
            Some(code) => Ok(format!("exit_code={code}\n{}", outcome.output)),
            None => Ok(format!(
                "命令仍在运行（已转入后台）。shellId={} 输出日志={}\n\
                 用 shell_output 工具（shellId + offset）续读输出；用 stop_command 终止。\n\
                 目前的输出：\n{}",
                outcome.job.id,
                outcome.job.output_path.display(),
                outcome.output
            )),
        }
    }
}

pub struct ShellOutput;

#[async_trait]
impl AgentTool for ShellOutput {
    fn name(&self) -> &str {
        "shell_output"
    }
    fn description(&self) -> &str {
        "读取一个后台 shell 任务（run_command 返回的 shellId）的最新输出。\
         传上次返回的 nextOffset 可只读增量；任务结束时会带 exit_code。"
    }
    fn read_only(&self) -> bool {
        true
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "shellId": {"type": "string", "description": "run_command 返回的 shellId"},
                "offset": {"type": "integer", "description": "续读的字节偏移（默认 0 = 从头）"}
            },
            "required": ["shellId"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let id = args
            .get("shellId")
            .or_else(|| args.get("shell_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        let (chunk, next, code) = ctx
            .shell
            .read_output(id, offset)
            .ok_or_else(|| ApiError::new("SHELL_NOT_FOUND", format!("unknown shellId: {id}")))?;
        let status = match code {
            Some(c) => format!("exited, exit_code={c}"),
            None => "running".to_string(),
        };
        Ok(format!(
            "status={status} nextOffset={next}\n{}",
            if chunk.is_empty() {
                "(无新输出)"
            } else {
                &chunk
            }
        ))
    }
}

pub struct StopCommand;

#[async_trait]
impl AgentTool for StopCommand {
    fn name(&self) -> &str {
        "stop_command"
    }
    fn description(&self) -> &str {
        "终止一个后台 shell 任务（run_command 返回的 shellId）。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "shellId": {"type": "string", "description": "要终止的 shellId"}
            },
            "required": ["shellId"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let id = args
            .get("shellId")
            .or_else(|| args.get("shell_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if ctx.shell.kill(id) {
            Ok(format!("已请求终止 {id}"))
        } else {
            Err(ApiError::new(
                "SHELL_NOT_FOUND",
                format!("没有正在运行的任务: {id}"),
            ))
        }
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
