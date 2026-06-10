//! Durable entity store backed by `redb` (pure-Rust embedded ACID KV store).
//!
//! Replaces the Python `InMemoryTable` (volatile, lost on restart) and the
//! scattered JSON files. `redb` gives crash-safe single-writer / multi-reader
//! transactions (MVCC) — the same durability + concurrent-read property we
//! originally wanted from SQLite WAL, without any C toolchain dependency.

use std::path::Path;
use std::sync::Arc;

use redb::{
    Database, MultimapTableDefinition, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

pub const T_SESSIONS: &str = "sessions";
pub const T_PLANS: &str = "plans";
pub const T_TODOS: &str = "todos";
pub const T_RUNS: &str = "runs";
pub const T_CHANNELS: &str = "channels";
pub const T_USERS: &str = "users";
pub const T_CHECKPOINTS: &str = "checkpoints";
pub const T_KV: &str = "kv";

/// Secondary (multimap) indexes: `session_id -> entity_id`, so per-session
/// listings are range reads instead of full-table scans.
pub const IDX_TODOS_BY_SESSION: &str = "idx_todos_by_session";
pub const IDX_CHECKPOINTS_BY_SESSION: &str = "idx_checkpoints_by_session";

const ALL_TABLES: &[&str] = &[
    T_SESSIONS,
    T_PLANS,
    T_TODOS,
    T_RUNS,
    T_CHANNELS,
    T_USERS,
    T_CHECKPOINTS,
    T_KV,
];

const ALL_INDEXES: &[&str] = &[IDX_TODOS_BY_SESSION, IDX_CHECKPOINTS_BY_SESSION];

fn table_def(name: &str) -> TableDefinition<'_, &'static str, &'static [u8]> {
    TableDefinition::new(name)
}

fn index_def(name: &str) -> MultimapTableDefinition<'_, &'static str, &'static str> {
    MultimapTableDefinition::new(name)
}

#[derive(Clone)]
pub struct Store {
    db: Arc<Database>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = Database::create(path)?;
        // Pre-create all tables so read transactions never fail on a fresh DB.
        let wtx = db.begin_write()?;
        {
            for name in ALL_TABLES {
                let _ = wtx.open_table(table_def(name))?;
            }
            for name in ALL_INDEXES {
                let _ = wtx.open_multimap_table(index_def(name))?;
            }
        }
        wtx.commit()?;
        Ok(Store { db: Arc::new(db) })
    }

    pub fn put<T: Serialize>(&self, table: &str, id: &str, value: &T) -> anyhow::Result<()> {
        let result = self.put_inner(table, id, value);
        if let Err(e) = &result {
            tracing::warn!("store.put {table}/{id} failed: {e}");
        }
        result
    }

    fn put_inner<T: Serialize>(&self, table: &str, id: &str, value: &T) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(value)?;
        let wtx = self.db.begin_write()?;
        {
            let mut t = wtx.open_table(table_def(table))?;
            t.insert(id, bytes.as_slice())?;
        }
        wtx.commit()?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(&self, table: &str, id: &str) -> anyhow::Result<Option<T>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(table_def(table))?;
        match t.get(id)? {
            Some(guard) => {
                let value: T = serde_json::from_slice(guard.value())?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    pub fn list<T: DeserializeOwned>(&self, table: &str) -> anyhow::Result<Vec<T>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(table_def(table))?;
        let mut out = Vec::new();
        for row in t.iter()? {
            let (_k, v) = row?;
            if let Ok(item) = serde_json::from_slice::<T>(v.value()) {
                out.push(item);
            }
        }
        Ok(out)
    }

    pub fn delete(&self, table: &str, id: &str) -> anyhow::Result<bool> {
        let result = self.delete_inner(table, id);
        if let Err(e) = &result {
            tracing::warn!("store.delete {table}/{id} failed: {e}");
        }
        result
    }

    fn delete_inner(&self, table: &str, id: &str) -> anyhow::Result<bool> {
        let wtx = self.db.begin_write()?;
        let existed;
        {
            let mut t = wtx.open_table(table_def(table))?;
            existed = t.remove(id)?.is_some();
        }
        wtx.commit()?;
        Ok(existed)
    }

    // ---- secondary index helpers (multimap: key -> many ids) ----

    pub fn index_add(&self, index: &str, key: &str, id: &str) -> anyhow::Result<()> {
        let result: anyhow::Result<()> = (|| {
            let wtx = self.db.begin_write()?;
            {
                let mut t = wtx.open_multimap_table(index_def(index))?;
                t.insert(key, id)?;
            }
            wtx.commit()?;
            Ok(())
        })();
        if let Err(e) = &result {
            tracing::warn!("store.index_add {index}/{key} failed: {e}");
        }
        result
    }

    pub fn index_remove(&self, index: &str, key: &str, id: &str) -> anyhow::Result<()> {
        let result: anyhow::Result<()> = (|| {
            let wtx = self.db.begin_write()?;
            {
                let mut t = wtx.open_multimap_table(index_def(index))?;
                t.remove(key, id)?;
            }
            wtx.commit()?;
            Ok(())
        })();
        if let Err(e) = &result {
            tracing::warn!("store.index_remove {index}/{key} failed: {e}");
        }
        result
    }

    pub fn index_values(&self, index: &str, key: &str) -> Vec<String> {
        let read = || -> anyhow::Result<Vec<String>> {
            let rtx = self.db.begin_read()?;
            let t = rtx.open_multimap_table(index_def(index))?;
            let mut out = Vec::new();
            for v in t.get(key)? {
                out.push(v?.value().to_string());
            }
            Ok(out)
        };
        read().unwrap_or_default()
    }

    pub fn count(&self, table: &str) -> anyhow::Result<u64> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(table_def(table))?;
        Ok(t.len()?)
    }

    // ---- KV helpers (string values) for misc config ----

    pub fn kv_get(&self, key: &str) -> Option<String> {
        let rtx = self.db.begin_read().ok()?;
        let t = rtx.open_table(table_def(T_KV)).ok()?;
        let guard = t.get(key).ok()??;
        Some(String::from_utf8_lossy(guard.value()).to_string())
    }

    pub fn kv_put(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let result: anyhow::Result<()> = (|| {
            let wtx = self.db.begin_write()?;
            {
                let mut t = wtx.open_table(table_def(T_KV))?;
                t.insert(key, value.as_bytes())?;
            }
            wtx.commit()?;
            Ok(())
        })();
        if let Err(e) = &result {
            tracing::warn!("store.kv_put {key} failed: {e}");
        }
        result
    }
}
