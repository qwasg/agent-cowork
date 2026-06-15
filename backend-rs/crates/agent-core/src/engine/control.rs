//! Run lifecycle control: registration, pause / resume / cancel, persistence.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use agent_protocol::models::AgentRun;
use agent_protocol::{ApiError, ApiResult};
use agent_store::store::T_RUNS;

use super::Runtime;

pub struct RunControl {
    pub cancel: CancellationToken,
    pub paused: AtomicBool,
    pub resume: Notify,
    /// User messages queued mid-run (steering); drained by the tool loop
    /// between LLM steps and injected as fresh user messages.
    pub steering: Mutex<Vec<String>>,
}

impl RunControl {
    pub fn drain_steering(&self) -> Vec<String> {
        std::mem::take(&mut *self.steering.lock().unwrap())
    }
}

impl Runtime {
    pub fn register_run(&self, run_id: &str) -> Arc<RunControl> {
        let control = Arc::new(RunControl {
            cancel: CancellationToken::new(),
            paused: AtomicBool::new(false),
            resume: Notify::new(),
            steering: Mutex::new(Vec::new()),
        });
        self.controls
            .lock()
            .unwrap()
            .insert(run_id.to_string(), control.clone());
        control
    }

    pub fn unregister_run(&self, run_id: &str) {
        self.controls.lock().unwrap().remove(run_id);
    }

    fn control(&self, run_id: &str) -> Option<Arc<RunControl>> {
        self.controls.lock().unwrap().get(run_id).cloned()
    }

    pub fn pause_run(&self, run_id: &str) -> bool {
        if let Some(c) = self.control(run_id) {
            c.paused.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn resume_run(&self, run_id: &str) -> bool {
        if let Some(c) = self.control(run_id) {
            c.paused.store(false, Ordering::SeqCst);
            c.resume.notify_waiters();
            true
        } else {
            false
        }
    }

    pub fn cancel_run(&self, run_id: &str) -> bool {
        if let Some(c) = self.control(run_id) {
            c.cancel.cancel();
            c.resume.notify_waiters();
            true
        } else {
            false
        }
    }

    /// Queue a user message into a running agent loop (Claude Code style
    /// steering). Returns false when the run isn't active.
    pub fn steer_run(&self, run_id: &str, text: &str) -> bool {
        let Some(c) = self.control(run_id) else {
            return false;
        };
        c.steering.lock().unwrap().push(text.to_string());
        // Wake a paused loop so the steer is picked up promptly.
        c.resume.notify_waiters();
        if let Ok(run) = self.get_run(run_id) {
            self.emit(
                &run.session_id,
                "agent.steered",
                "agent",
                json!({ "runId": run_id, "text": text, "phase": "queued" }),
                Some(run_id.to_string()),
            );
        }
        true
    }

    /// Cancel every active run (graceful shutdown).
    pub fn cancel_all(&self) {
        let controls = self.controls.lock().unwrap();
        for c in controls.values() {
            c.cancel.cancel();
            c.resume.notify_waiters();
        }
    }

    /// Persist run state via the store writer thread. Awaiting (instead of
    /// fire-and-forget) keeps read-after-write ordering for callers that
    /// `get_run` right after; failures are logged + counted by the store.
    pub async fn save_run(&self, run: &AgentRun) {
        let _ = self.astore.put(T_RUNS, &run.id, run).await;
    }

    pub fn get_run(&self, run_id: &str) -> ApiResult<AgentRun> {
        self.store
            .get::<AgentRun>(T_RUNS, run_id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::run_not_found(run_id))
    }
}
