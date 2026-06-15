//! Async facade over [`Store`] backed by a dedicated writer thread.
//!
//! `redb` write transactions block on fsync at commit. Calling them directly
//! from async handlers stalls tokio worker threads. `AsyncStore` funnels all
//! writes through one dedicated OS thread (redb is single-writer anyway, so
//! this adds no contention) and exposes:
//!
//! - awaitable writes (`put` / `delete` / `kv_put` / ...) that resolve with the
//!   real result — used by REST mutations that must report `STORE_ERROR`;
//! - fire-and-forget writes (`spawn`) for background persistence where the
//!   store layer's warn-log + failure counter is the observable signal.
//!
//! Reads stay synchronous on the caller's thread: redb reads are MVCC and
//! never block behind the writer.

use std::sync::{mpsc, Arc};

use serde::Serialize;

use crate::store::Store;

type Job = Box<dyn FnOnce(&Store) + Send + 'static>;

pub struct AsyncStore {
    store: Arc<Store>,
    tx: mpsc::Sender<Job>,
}

impl AsyncStore {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let worker_store = store.clone();
        std::thread::Builder::new()
            .name("store-writer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&worker_store);
                }
            })
            .expect("failed to spawn store-writer thread");
        Arc::new(AsyncStore { store, tx })
    }

    /// Synchronous handle for reads (MVCC, non-blocking w.r.t. the writer).
    pub fn sync(&self) -> &Arc<Store> {
        &self.store
    }

    /// Queue a write without awaiting its result. Failures are logged and
    /// counted by the store layer (see [`crate::store::write_failure_count`]).
    pub fn spawn(&self, f: impl FnOnce(&Store) + Send + 'static) {
        if self.tx.send(Box::new(f)).is_err() {
            tracing::warn!("async_store: writer thread is gone, write dropped");
        }
    }

    async fn submit<R, F>(&self, f: F) -> anyhow::Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Store) -> anyhow::Result<R> + Send + 'static,
    {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |s| {
                let _ = otx.send(f(s));
            }))
            .map_err(|_| anyhow::anyhow!("store writer thread is gone"))?;
        orx.await
            .map_err(|_| anyhow::anyhow!("store writer dropped the job"))?
    }

    pub async fn put<T: Serialize>(
        &self,
        table: &'static str,
        id: &str,
        value: &T,
    ) -> anyhow::Result<()> {
        let id = id.to_string();
        // Serialize on the caller side so `T` needs no Send + 'static bounds.
        let value = serde_json::to_value(value)?;
        self.submit(move |s| s.put(table, &id, &value)).await
    }

    pub async fn delete(&self, table: &'static str, id: &str) -> anyhow::Result<bool> {
        let id = id.to_string();
        self.submit(move |s| s.delete(table, &id)).await
    }

    pub async fn kv_put(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        self.submit(move |s| s.kv_put(&key, &value)).await
    }

    pub async fn kv_delete(&self, key: &str) -> anyhow::Result<()> {
        let key = key.to_string();
        self.submit(move |s| s.kv_delete(&key)).await
    }

    pub async fn index_add(&self, index: &'static str, key: &str, id: &str) -> anyhow::Result<()> {
        let key = key.to_string();
        let id = id.to_string();
        self.submit(move |s| s.index_add(index, &key, &id)).await
    }

    pub async fn index_remove(
        &self,
        index: &'static str,
        key: &str,
        id: &str,
    ) -> anyhow::Result<()> {
        let key = key.to_string();
        let id = id.to_string();
        self.submit(move |s| s.index_remove(index, &key, &id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::T_KV;

    fn tmp_store(name: &str) -> Arc<Store> {
        let dir = std::env::temp_dir().join("agentd-async-store-tests");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{name}-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Arc::new(Store::open(path).unwrap())
    }

    #[tokio::test]
    async fn awaited_writes_roundtrip() {
        let astore = AsyncStore::new(tmp_store("roundtrip"));
        astore.kv_put("k1", "v1").await.unwrap();
        assert_eq!(astore.sync().kv_get("k1").as_deref(), Some("v1"));

        astore
            .put(T_KV, "row", &serde_json::json!({ "a": 1 }))
            .await
            .unwrap();
        let got: Option<serde_json::Value> = astore.sync().get(T_KV, "row").unwrap();
        assert_eq!(got.unwrap()["a"], 1);

        assert!(astore.delete(T_KV, "row").await.unwrap());
        assert!(!astore.delete(T_KV, "row").await.unwrap());

        astore.kv_delete("k1").await.unwrap();
        assert_eq!(astore.sync().kv_get("k1"), None);
    }

    #[tokio::test]
    async fn spawned_writes_apply_in_order() {
        let astore = AsyncStore::new(tmp_store("spawn-order"));
        for i in 0..10 {
            astore.spawn(move |s| {
                let _ = s.kv_put("counter", &i.to_string());
            });
        }
        // An awaited write behind the queue acts as a barrier.
        astore.kv_put("done", "yes").await.unwrap();
        assert_eq!(astore.sync().kv_get("counter").as_deref(), Some("9"));
    }
}
