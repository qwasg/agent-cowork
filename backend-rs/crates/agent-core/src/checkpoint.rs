//! Checkpoint service with real on-disk persistence (tech-debt fix: the Python
//! `checkpoint_service.py` only kept snapshots in memory). Snapshots store file
//! contents so a `rewind` can restore them.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agent_protocol::models::{new_id, now_ts};
use agent_protocol::{ApiError, ApiResult};
use agent_store::store::{IDX_CHECKPOINTS_BY_SESSION, T_CHECKPOINTS};
use agent_store::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileSnapshot {
    path: String,
    content: String,
    existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    id: String,
    session_id: String,
    label: String,
    files: Vec<FileSnapshot>,
    /// Event-bus seq at creation time; rewind truncates the session's event
    /// stream back to this point (parity with the plan's rewind semantics).
    #[serde(default)]
    event_seq: i64,
    created_at: String,
}

pub struct CheckpointService {
    store: Arc<Store>,
    default_workspace_root: PathBuf,
}

impl CheckpointService {
    pub fn new(store: Arc<Store>, workspace_root: PathBuf) -> Self {
        CheckpointService {
            store,
            default_workspace_root: workspace_root,
        }
    }

    /// Resolve the workspace root dynamically (it can be changed at runtime
    /// via `POST /workspace/root`); previously this was snapshotted at boot,
    /// so a rewind could write into the wrong directory.
    fn workspace_root(&self) -> PathBuf {
        self.store
            .kv_get("workspace_root")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_workspace_root.clone())
    }

    pub fn list(&self, session_id: &str) -> Value {
        let ids = self
            .store
            .index_values(IDX_CHECKPOINTS_BY_SESSION, session_id);
        let checkpoints: Vec<Checkpoint> = if ids.is_empty() {
            self.store
                .list::<Checkpoint>(T_CHECKPOINTS)
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.session_id == session_id)
                .collect()
        } else {
            ids.iter()
                .filter_map(|id| {
                    self.store
                        .get::<Checkpoint>(T_CHECKPOINTS, id)
                        .ok()
                        .flatten()
                })
                .collect()
        };
        let items: Vec<Value> = checkpoints
            .into_iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "sessionId": c.session_id,
                    "label": c.label,
                    "fileCount": c.files.len(),
                    "eventSeq": c.event_seq,
                    "createdAt": c.created_at,
                })
            })
            .collect();
        json!({ "checkpoints": items })
    }

    pub fn create(
        &self,
        session_id: &str,
        paths: Option<Vec<String>>,
        label: &str,
        event_seq: i64,
    ) -> Value {
        let root = self.workspace_root();
        let mut files = Vec::new();
        for rel in paths.unwrap_or_default() {
            let abs = root.join(&rel);
            let (content, existed) = match std::fs::read_to_string(&abs) {
                Ok(c) => (c, true),
                Err(_) => (String::new(), false),
            };
            files.push(FileSnapshot {
                path: rel,
                content,
                existed,
            });
        }
        let cp = Checkpoint {
            id: new_id("ckpt"),
            session_id: session_id.to_string(),
            label: label.to_string(),
            files,
            event_seq,
            created_at: now_ts(),
        };
        let _ = self.store.put(T_CHECKPOINTS, &cp.id, &cp);
        let _ = self
            .store
            .index_add(IDX_CHECKPOINTS_BY_SESSION, session_id, &cp.id);
        json!({
            "checkpoint": {
                "id": cp.id,
                "sessionId": cp.session_id,
                "label": cp.label,
                "fileCount": cp.files.len(),
                "eventSeq": cp.event_seq,
                "createdAt": cp.created_at,
            }
        })
    }

    /// Restore the snapshotted files and report the session/seq the caller
    /// should rewind the event stream to.
    pub fn rewind(&self, checkpoint_id: &str) -> ApiResult<Value> {
        let cp = self
            .store
            .get::<Checkpoint>(T_CHECKPOINTS, checkpoint_id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::new("CHECKPOINT_NOT_FOUND", "checkpoint not found"))?;
        let root = self.workspace_root();
        let mut restored = Vec::new();
        for f in &cp.files {
            let abs = root.join(&f.path);
            if f.existed {
                if let Some(parent) = abs.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&abs, &f.content)
                    .map_err(|e| ApiError::filesystem(format!("restore {}: {e}", f.path)))?;
            } else {
                let _ = std::fs::remove_file(&abs);
            }
            restored.push(f.path.clone());
        }
        Ok(json!({
            "ok": true,
            "restored": restored,
            "checkpointId": checkpoint_id,
            "sessionId": cp.session_id,
            "eventSeq": cp.event_seq,
        }))
    }
}
