//! Claude-Code-style lifecycle hooks: `PreToolUse`, `PostToolUse`, `TurnEnd`.
//!
//! Hooks are external commands configured in `data/hooks.json`:
//!
//! ```json
//! {
//!   "hooks": [
//!     { "event": "PreToolUse", "matcher": "run_command", "command": "python check.py", "timeoutMs": 5000 }
//!   ]
//! }
//! ```
//!
//! The hook command receives a JSON payload on stdin (`event`, `tool`,
//! `args`, plus `result` for PostToolUse). Exit-code semantics:
//! - `0` — continue; a PreToolUse hook may print `{"args": {...}}` on stdout
//!   to rewrite the tool arguments.
//! - `2` — block the tool call; stderr (or stdout) is the reason.
//! - anything else — logged as a warning and ignored (fail-open).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    /// `PreToolUse` | `PostToolUse` | `TurnEnd`
    pub event: String,
    /// Tool-name glob (`*` supported); `TurnEnd` hooks ignore it. Optional —
    /// missing means "match everything".
    #[serde(default)]
    pub matcher: Option<String>,
    /// Shell command executed via PowerShell / bash.
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct HookConfig {
    #[serde(default)]
    hooks: Vec<HookDef>,
}

/// Result of running the PreToolUse chain for one tool call.
pub enum PreToolVerdict {
    Continue(Option<Value>),
    Block(String),
}

pub struct HookRunner {
    config_path: PathBuf,
    cache: Mutex<Option<(SystemTime, Vec<HookDef>)>>,
}

impl HookRunner {
    pub fn new(config_path: PathBuf) -> Self {
        HookRunner {
            config_path,
            cache: Mutex::new(None),
        }
    }

    /// Load hooks, re-reading the file only when its mtime changes.
    pub fn hooks(&self) -> Vec<HookDef> {
        let mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        let Some(mtime) = mtime else {
            return Vec::new(); // no config file → no hooks
        };
        let mut cache = self.cache.lock().unwrap();
        if let Some((cached_mtime, hooks)) = cache.as_ref() {
            if *cached_mtime == mtime {
                return hooks.clone();
            }
        }
        let hooks = std::fs::read_to_string(&self.config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<HookConfig>(&raw).ok())
            .map(|c| c.hooks)
            .unwrap_or_else(|| {
                tracing::warn!(
                    "hooks: failed to parse {} — ignoring all hooks",
                    self.config_path.display()
                );
                Vec::new()
            });
        *cache = Some((mtime, hooks.clone()));
        hooks
    }

    fn matching(&self, event: &str, tool: &str) -> Vec<HookDef> {
        self.hooks()
            .into_iter()
            .filter(|h| h.event == event)
            .filter(|h| match &h.matcher {
                Some(m) => crate::permission::glob_match(m, tool),
                None => true,
            })
            .collect()
    }

    /// Run PreToolUse hooks in order. The first blocking hook wins; arg
    /// rewrites chain (each hook sees the previous hook's rewrite).
    pub async fn pre_tool_use(&self, session_id: &str, tool: &str, args: &Value) -> PreToolVerdict {
        let mut current = args.clone();
        let mut rewritten = false;
        for hook in self.matching("PreToolUse", tool) {
            let payload = json!({
                "event": "PreToolUse",
                "sessionId": session_id,
                "tool": tool,
                "args": current,
            });
            match run_hook_command(&hook, &payload).await {
                HookExit::Ok(stdout) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&stdout) {
                        if let Some(new_args) = v.get("args") {
                            current = new_args.clone();
                            rewritten = true;
                        }
                    }
                }
                HookExit::Block(reason) => return PreToolVerdict::Block(reason),
                HookExit::Failed => {} // fail-open, already logged
            }
        }
        PreToolVerdict::Continue(rewritten.then_some(current))
    }

    /// Run PostToolUse hooks (observational; output ignored).
    pub async fn post_tool_use(
        &self,
        session_id: &str,
        tool: &str,
        args: &Value,
        result: &str,
        ok: bool,
    ) {
        for hook in self.matching("PostToolUse", tool) {
            let payload = json!({
                "event": "PostToolUse",
                "sessionId": session_id,
                "tool": tool,
                "args": args,
                "ok": ok,
                "result": result.chars().take(8_000).collect::<String>(),
            });
            let _ = run_hook_command(&hook, &payload).await;
        }
    }

    /// Run TurnEnd hooks (fire-and-forget semantics, but awaited so tests
    /// and shutdown stay deterministic).
    pub async fn turn_end(&self, session_id: &str, run_id: &str, summary: &str) {
        for hook in self.matching("TurnEnd", "*") {
            let payload = json!({
                "event": "TurnEnd",
                "sessionId": session_id,
                "runId": run_id,
                "summary": summary.chars().take(8_000).collect::<String>(),
            });
            let _ = run_hook_command(&hook, &payload).await;
        }
    }
}

enum HookExit {
    Ok(String),
    Block(String),
    Failed,
}

async fn run_hook_command(hook: &HookDef, payload: &Value) -> HookExit {
    use tokio::io::AsyncWriteExt;

    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("powershell");
        c.arg("-NoProfile").arg("-Command").arg(&hook.command);
        c
    } else {
        let mut c = tokio::process::Command::new("bash");
        c.arg("-lc").arg(&hook.command);
        c
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("hook '{}' failed to spawn: {e}", hook.command);
            return HookExit::Failed;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let body = payload.to_string();
        let _ = stdin.write_all(body.as_bytes()).await;
        drop(stdin); // close stdin so the hook sees EOF
    }
    let timeout = Duration::from_millis(hook.timeout_ms.unwrap_or(10_000).clamp(500, 120_000));
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::warn!("hook '{}' failed: {e}", hook.command);
            return HookExit::Failed;
        }
        Err(_) => {
            tracing::warn!("hook '{}' timed out after {timeout:?}", hook.command);
            return HookExit::Failed;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    match output.status.code() {
        Some(0) => HookExit::Ok(stdout),
        Some(2) => HookExit::Block(if stderr.is_empty() { stdout } else { stderr }),
        code => {
            tracing::warn!(
                "hook '{}' exited with {:?} (ignored): {stderr}",
                hook.command,
                code
            );
            HookExit::Failed
        }
    }
}
