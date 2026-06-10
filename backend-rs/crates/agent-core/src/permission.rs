//! Per-session tool permission mode (port of `permission_service.py`).
//!
//! Modes (matching the Python semantics):
//! - `bypass` (default): every tool runs without confirmation
//! - `plan`: write tools are blocked entirely (read-only)
//! - `auto`: write tools require interactive approval — the run emits a
//!   `permission.requested` event and suspends on a oneshot channel until
//!   `POST /permissions/{id}:approve|deny` resolves it (or it times out).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::contracts::models::new_id;
use crate::contracts::{ApiError, ApiResult};
use crate::infra::Store;

/// Tools with side effects (file writes / command execution). Public so the
/// runtime can gate `explore`-kind todos down to a read-only tool set.
pub const WRITE_TOOLS: &[&str] = &[
    "write_file",
    "create_document",
    "delete_file",
    "run_command",
    "stop_command",
];

pub struct PermissionService {
    store: Arc<Store>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl PermissionService {
    pub fn new(store: Arc<Store>) -> Self {
        PermissionService {
            store,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn mode(&self, session_id: &str) -> String {
        self.store
            .kv_get(&key(session_id))
            .unwrap_or_else(|| "bypass".to_string())
    }

    pub fn set_mode(&self, session_id: &str, mode: &str) -> ApiResult<String> {
        if !matches!(mode, "auto" | "plan" | "bypass") {
            return Err(ApiError::new(
                "AUTH_INVALID_INPUT",
                "invalid permission mode",
            ));
        }
        let _ = self.store.kv_put(&key(session_id), mode);
        Ok(mode.to_string())
    }

    pub fn can_use_tool(&self, session_id: &str, tool: &str) -> bool {
        match self.mode(session_id).as_str() {
            "plan" => !WRITE_TOOLS.contains(&tool),
            _ => true,
        }
    }

    /// Whether the tool needs interactive user approval in the current mode.
    pub fn requires_approval(&self, session_id: &str, tool: &str) -> bool {
        self.mode(session_id) == "auto" && WRITE_TOOLS.contains(&tool)
    }

    // ---- interactive approval requests ----

    /// Register a pending approval; the caller awaits the receiver.
    pub fn create_request(&self) -> (String, oneshot::Receiver<bool>) {
        let id = new_id("perm");
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        (id, rx)
    }

    /// Resolve a pending request. Returns false if it doesn't exist (already
    /// resolved, timed out, or unknown).
    pub fn resolve(&self, request_id: &str, approved: bool) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some(tx) => tx.send(approved).is_ok(),
            None => false,
        }
    }

    /// Drop a request that timed out or was cancelled.
    pub fn discard_request(&self, request_id: &str) {
        self.pending.lock().unwrap().remove(request_id);
    }
}

fn key(session_id: &str) -> String {
    format!("perm:{session_id}")
}
