//! ShellManager: named persistent shell sessions, timeout-to-background
//! execution, output spooled to disk and resumable reads (`shell_output`).
//!
//! Every command's combined stdout/stderr streams into
//! `data/shell-outputs/{job_id}.log` while it runs. Foreground commands that
//! outlive their timeout are *not* killed — they keep running in the
//! background and the model gets the job id to poll with `shell_output`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use agent_protocol::models::new_id;

/// Sentinel used to recover the working directory after a session command.
const CWD_SENTINEL: &str = "__AGENT_SHELL_CWD__=";
/// Cap for inline output returned to the model (full output is on disk).
const INLINE_OUTPUT_CHARS: usize = 20_000;

pub struct ShellJob {
    pub id: String,
    pub command: String,
    pub output_path: PathBuf,
    /// `None` while running, `Some(exit_code)` once finished.
    status: watch::Receiver<Option<i32>>,
    kill: CancellationToken,
}

impl ShellJob {
    pub fn exit_code(&self) -> Option<i32> {
        *self.status.borrow()
    }
    pub fn is_running(&self) -> bool {
        self.exit_code().is_none()
    }
}

pub struct ShellManager {
    output_dir: PathBuf,
    jobs: Mutex<HashMap<String, Arc<ShellJob>>>,
    /// Named session → current working directory (persisted across commands).
    sessions: Mutex<HashMap<String, PathBuf>>,
}

pub struct RunOutcome {
    pub job: Arc<ShellJob>,
    /// `Some(exit_code)` when the command finished within the wait window.
    pub exit_code: Option<i32>,
    /// Inline (possibly truncated) output captured so far.
    pub output: String,
}

impl ShellManager {
    pub fn new(output_dir: PathBuf) -> Arc<Self> {
        Arc::new(ShellManager {
            output_dir,
            jobs: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
        })
    }

    pub fn job(&self, id: &str) -> Option<Arc<ShellJob>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    /// Snapshot of all known jobs (newest state, unordered).
    pub fn jobs(&self) -> Vec<Arc<ShellJob>> {
        self.jobs.lock().unwrap().values().cloned().collect()
    }

    pub fn kill(&self, id: &str) -> bool {
        match self.job(id) {
            Some(job) if job.is_running() => {
                job.kill.cancel();
                true
            }
            _ => false,
        }
    }

    fn session_cwd(&self, session: &str, default: &PathBuf) -> PathBuf {
        self.sessions
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .unwrap_or_else(|| default.clone())
    }

    /// Spawn `command`; wait up to `wait_ms` (0 = don't wait / background).
    /// On timeout the process keeps running and the caller polls via
    /// [`Self::read_output`].
    pub async fn run(
        self: &Arc<Self>,
        command: &str,
        workspace_root: &PathBuf,
        wait_ms: u64,
        session: Option<&str>,
    ) -> std::io::Result<RunOutcome> {
        let id = new_id("shell");
        let cwd = match session {
            Some(name) => self.session_cwd(name, workspace_root),
            None => workspace_root.clone(),
        };

        // Session commands append a cwd sentinel so `cd` persists.
        let effective = match session {
            Some(_) if cfg!(windows) => {
                format!("{command}; Write-Output (\"{CWD_SENTINEL}\" + (Get-Location).Path)")
            }
            Some(_) => format!("{command}; printf '\\n{CWD_SENTINEL}%s\\n' \"$PWD\""),
            None => command.to_string(),
        };

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("powershell");
            c.arg("-NoProfile").arg("-Command").arg(&effective);
            c
        } else {
            let mut c = tokio::process::Command::new("bash");
            c.arg("-lc").arg(&effective);
            c
        };
        std::fs::create_dir_all(&self.output_dir)?;
        let output_path = self.output_dir.join(format!("{id}.log"));
        cmd.current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;

        let (status_tx, status_rx) = watch::channel(None);
        let kill = CancellationToken::new();
        let job = Arc::new(ShellJob {
            id: id.clone(),
            command: command.to_string(),
            output_path: output_path.clone(),
            status: status_rx,
            kill: kill.clone(),
        });
        self.jobs.lock().unwrap().insert(id.clone(), job.clone());

        // Pump stdout + stderr into the log file until the process exits.
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let pump_path = output_path.clone();
        let session_name = session.map(String::from);
        let mgr = Arc::downgrade(self);
        tokio::spawn(async move {
            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&pump_path);
            let mut file = match file {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("shell: cannot open log {}: {e}", pump_path.display());
                    let _ = child.kill().await;
                    let _ = status_tx.send(Some(-1));
                    return;
                }
            };
            let mut out_buf = [0u8; 8192];
            let mut err_buf = [0u8; 8192];
            let mut out_done = stdout.is_none();
            let mut err_done = stderr.is_none();
            let mut tail = String::new();
            loop {
                if out_done && err_done {
                    break;
                }
                tokio::select! {
                    n = async {
                        match &mut stdout {
                            Some(s) => s.read(&mut out_buf).await,
                            None => Ok(0),
                        }
                    }, if !out_done => {
                        match n {
                            Ok(0) | Err(_) => out_done = true,
                            Ok(n) => {
                                let chunk = String::from_utf8_lossy(&out_buf[..n]).to_string();
                                tail.push_str(&chunk);
                                if tail.len() > 4096 { let cut = tail.len() - 4096; tail.drain(..cut); }
                                let _ = file.write_all(chunk.as_bytes());
                            }
                        }
                    }
                    n = async {
                        match &mut stderr {
                            Some(s) => s.read(&mut err_buf).await,
                            None => Ok(0),
                        }
                    }, if !err_done => {
                        match n {
                            Ok(0) | Err(_) => err_done = true,
                            Ok(n) => {
                                let _ = file.write_all(&err_buf[..n]);
                            }
                        }
                    }
                    _ = kill.cancelled() => {
                        let _ = child.kill().await;
                        let _ = file.write_all(b"\n[killed]\n");
                        let _ = status_tx.send(Some(-1));
                        return;
                    }
                }
            }
            let code = match child.wait().await {
                Ok(status) => status.code().unwrap_or(-1),
                Err(_) => -1,
            };
            let _ = file.flush();
            // Persist the session cwd from the sentinel line.
            if let (Some(name), Some(mgr)) = (session_name, mgr.upgrade()) {
                if let Some(pos) = tail.rfind(CWD_SENTINEL) {
                    let cwd_line = tail[pos + CWD_SENTINEL.len()..]
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !cwd_line.is_empty() {
                        mgr.sessions
                            .lock()
                            .unwrap()
                            .insert(name, PathBuf::from(cwd_line));
                    }
                }
            }
            let _ = status_tx.send(Some(code));
        });

        // Wait window (0 = pure background).
        let mut status = job.status.clone();
        if wait_ms > 0 {
            let _ = tokio::time::timeout(Duration::from_millis(wait_ms), async {
                loop {
                    if status.borrow().is_some() {
                        break;
                    }
                    if status.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
        }

        let exit_code = job.exit_code();
        let output = read_log(&output_path, 0).0;
        Ok(RunOutcome {
            job,
            exit_code,
            output,
        })
    }

    /// Resume reading a job's spooled output from `offset` bytes.
    /// Returns `(chunk, next_offset, exit_code)`.
    pub fn read_output(&self, id: &str, offset: u64) -> Option<(String, u64, Option<i32>)> {
        let job = self.job(id)?;
        let (chunk, next) = read_log(&job.output_path, offset);
        Some((chunk, next, job.exit_code()))
    }
}

/// Read the log file from a byte offset, capping the inline chunk size.
fn read_log(path: &PathBuf, offset: u64) -> (String, u64) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return (String::new(), offset);
    };
    if f.seek(SeekFrom::Start(offset)).is_err() {
        return (String::new(), offset);
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return (String::new(), offset);
    }
    let next = offset + buf.len() as u64;
    let mut text = String::from_utf8_lossy(&buf).to_string();
    // Hide the internal cwd sentinel from the model.
    if let Some(pos) = text.rfind(CWD_SENTINEL) {
        let end = text[pos..]
            .find('\n')
            .map(|n| pos + n + 1)
            .unwrap_or(text.len());
        text.replace_range(pos..end, "");
    }
    if text.chars().count() > INLINE_OUTPUT_CHARS {
        let skipped = text.chars().count() - INLINE_OUTPUT_CHARS;
        let tail: String = text.chars().skip(skipped).collect();
        text = format!("[…前 {skipped} 个字符见日志文件…]\n{tail}");
    }
    (text, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> Arc<ShellManager> {
        let dir = std::env::temp_dir()
            .join("agentd-shell-tests")
            .join(new_id("t"));
        ShellManager::new(dir)
    }

    fn ws() -> PathBuf {
        std::env::temp_dir()
    }

    #[tokio::test]
    async fn foreground_command_completes() {
        let m = mgr();
        let out = m
            .run("echo hello-shell", &ws(), 30_000, None)
            .await
            .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(out.output.contains("hello-shell"), "got: {}", out.output);
    }

    #[tokio::test]
    async fn timeout_moves_to_background_and_output_is_resumable() {
        let m = mgr();
        let cmd = if cfg!(windows) {
            "Write-Output begin; Start-Sleep -Seconds 3; Write-Output done"
        } else {
            "echo begin; sleep 3; echo done"
        };
        let out = m.run(cmd, &ws(), 500, None).await.unwrap();
        assert_eq!(out.exit_code, None, "should still be running");
        let id = out.job.id.clone();

        // Poll until it finishes.
        let mut waited = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            waited += 300;
            let (_, _, code) = m.read_output(&id, 0).unwrap();
            if code.is_some() {
                break;
            }
            assert!(waited < 15_000, "command never finished");
        }
        let (chunk, next, code) = m.read_output(&id, 0).unwrap();
        assert_eq!(code, Some(0));
        assert!(chunk.contains("done"), "got: {chunk}");
        assert!(next > 0);
        // Offset continuation returns nothing new at EOF.
        let (rest, _, _) = m.read_output(&id, next).unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn session_persists_cwd() {
        let m = mgr();
        let root = std::env::temp_dir();
        let sub = root.join("agentd-shell-cwd-test");
        let _ = std::fs::create_dir_all(&sub);
        let cd = format!("cd {}", sub.display());
        let out = m.run(&cd, &root, 30_000, Some("s1")).await.unwrap();
        assert_eq!(out.exit_code, Some(0));
        let pwd_cmd = if cfg!(windows) {
            "(Get-Location).Path"
        } else {
            "pwd"
        };
        let out = m.run(pwd_cmd, &root, 30_000, Some("s1")).await.unwrap();
        assert!(
            out.output.to_lowercase().contains("agentd-shell-cwd-test"),
            "cwd not persisted: {}",
            out.output
        );
    }

    #[tokio::test]
    async fn kill_stops_background_job() {
        let m = mgr();
        let cmd = if cfg!(windows) {
            "Start-Sleep -Seconds 60"
        } else {
            "sleep 60"
        };
        let out = m.run(cmd, &ws(), 0, None).await.unwrap();
        let id = out.job.id.clone();
        assert!(m.kill(&id));
        // Wait for the kill to land.
        let mut status = out.job.status.clone();
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while status.borrow().is_none() {
                if status.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        assert_eq!(out.job.exit_code(), Some(-1));
        assert!(!m.kill(&id), "killing a dead job reports false");
    }
}
