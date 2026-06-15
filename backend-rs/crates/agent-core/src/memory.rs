//! Structured cross-session memory.
//!
//! Memories are durable knowledge the agent (or user) chose to remember:
//! user preferences, project facts and conventions. They are stored in redb
//! (`T_MEMORIES`, indexed by `scope`) and retrieved with a dependency-free
//! keyword scorer (ASCII word tokens + CJK character bigrams) so the feature
//! works offline with no embedding provider.
//!
//! Scopes bound where a memory applies:
//! - `global` — everywhere;
//! - `workspace:{root}` — a specific workspace;
//! - `session:{id}` — a single conversation.

use std::collections::HashSet;
use std::sync::Arc;

use agent_protocol::models::{now_ts, MemoryEntry};
use agent_protocol::{ApiError, ApiResult};
use agent_store::store::{IDX_MEMORIES_BY_SCOPE, T_MEMORIES};
use agent_store::Store;

/// Max entries kept per scope; oldest low-hit entries are evicted past this.
const PER_SCOPE_CAP: usize = 200;

pub struct MemoryService {
    store: Arc<Store>,
}

impl MemoryService {
    pub fn new(store: Arc<Store>) -> Self {
        MemoryService { store }
    }

    pub fn get(&self, id: &str) -> ApiResult<MemoryEntry> {
        self.store
            .get::<MemoryEntry>(T_MEMORIES, id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::memory_not_found(id))
    }

    pub fn save(&self, entry: &MemoryEntry) {
        let _ = self.store.put(T_MEMORIES, &entry.id, entry);
        let _ = self
            .store
            .index_add(IDX_MEMORIES_BY_SCOPE, &entry.scope, &entry.id);
    }

    /// All memories in a single scope (index fast-path, full-scan fallback).
    pub fn list_by_scope(&self, scope: &str) -> Vec<MemoryEntry> {
        let ids = self.store.index_values(IDX_MEMORIES_BY_SCOPE, scope);
        let mut out: Vec<MemoryEntry> = if ids.is_empty() {
            self.store
                .list::<MemoryEntry>(T_MEMORIES)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.scope == scope)
                .collect()
        } else {
            ids.iter()
                .filter_map(|id| self.store.get::<MemoryEntry>(T_MEMORIES, id).ok().flatten())
                .collect()
        };
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    /// Every memory across all scopes (newest first). Used by the management
    /// API when no scope filter is supplied.
    pub fn list_all(&self) -> Vec<MemoryEntry> {
        let mut out = self
            .store
            .list::<MemoryEntry>(T_MEMORIES)
            .unwrap_or_default();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    /// Memories spanning several scopes (e.g. global + workspace + session).
    pub fn list_for_scopes(&self, scopes: &[String]) -> Vec<MemoryEntry> {
        let mut out: Vec<MemoryEntry> = scopes.iter().flat_map(|s| self.list_by_scope(s)).collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn delete(&self, id: &str) -> ApiResult<()> {
        let entry = self.get(id)?;
        let _ = self
            .store
            .index_remove(IDX_MEMORIES_BY_SCOPE, &entry.scope, id);
        self.store
            .delete(T_MEMORIES, id)
            .map_err(|e| ApiError::store(e.to_string()))?;
        Ok(())
    }

    /// Insert a memory, deduplicating against near-identical content in the
    /// same scope (same normalized text → refresh + merge tags instead of a
    /// duplicate row). Enforces the per-scope capacity afterwards.
    pub fn upsert(
        &self,
        scope: &str,
        kind: &str,
        content: &str,
        tags: Vec<String>,
        source_session_id: Option<String>,
    ) -> ApiResult<MemoryEntry> {
        let content = content.trim();
        if content.is_empty() {
            return Err(ApiError::new("MEMORY_INVALID", "memory content is empty"));
        }
        let norm = normalize_content(content);
        if let Some(mut existing) = self
            .list_by_scope(scope)
            .into_iter()
            .find(|m| normalize_content(&m.content) == norm)
        {
            existing.kind = kind.to_string();
            existing.content = content.to_string();
            for t in tags {
                if !existing.tags.contains(&t) {
                    existing.tags.push(t);
                }
            }
            existing.updated_at = now_ts();
            self.save(&existing);
            return Ok(existing);
        }
        let entry = MemoryEntry::new(
            scope.to_string(),
            kind.to_string(),
            content.to_string(),
            tags,
            source_session_id,
        );
        self.save(&entry);
        self.enforce_capacity(scope);
        Ok(entry)
    }

    /// Keyword search across the given scopes. Increments the `hits` counter on
    /// returned entries so frequently-useful memories survive eviction and sort
    /// higher over time. Returns up to `limit` entries, best match first.
    pub fn search(&self, query: &str, scopes: &[String], limit: usize) -> Vec<MemoryEntry> {
        let terms = terms(query);
        let mut scored: Vec<(i64, MemoryEntry)> = self
            .list_for_scopes(scopes)
            .into_iter()
            .map(|m| (score(&terms, &m), m))
            .filter(|(s, _)| terms.is_empty() || *s > 0)
            .collect();
        // Best score, then most-used, then most-recent.
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.hits.cmp(&a.1.hits))
                .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
        });
        scored.truncate(limit);
        let hits: Vec<MemoryEntry> = scored.into_iter().map(|(_, m)| m).collect();
        // Record usage (best-effort; never fails the search).
        for m in &hits {
            if let Ok(mut fresh) = self.get(&m.id) {
                fresh.hits += 1;
                let _ = self.store.put(T_MEMORIES, &fresh.id, &fresh);
            }
        }
        hits
    }

    /// Evict the weakest entries (fewest hits, then oldest) once a scope grows
    /// past [`PER_SCOPE_CAP`].
    fn enforce_capacity(&self, scope: &str) {
        let mut entries = self.list_by_scope(scope);
        if entries.len() <= PER_SCOPE_CAP {
            return;
        }
        entries.sort_by(|a, b| {
            a.hits
                .cmp(&b.hits)
                .then_with(|| a.updated_at.cmp(&b.updated_at))
        });
        let excess = entries.len() - PER_SCOPE_CAP;
        for victim in entries.into_iter().take(excess) {
            let _ = self.delete(&victim.id);
        }
    }
}

fn normalize_content(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_cjk(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

/// Search terms from a string: lowercase ASCII word runs (len >= 2) plus CJK
/// character bigrams (so Chinese queries match without a tokenizer).
fn terms(s: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    let mut out: HashSet<String> = HashSet::new();
    let mut cur = String::new();
    let mut cjk: Vec<char> = Vec::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch);
        } else {
            if cur.chars().count() >= 2 {
                out.insert(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
        if is_cjk(ch) {
            cjk.push(ch);
        }
    }
    if cur.chars().count() >= 2 {
        out.insert(cur);
    }
    if cjk.len() == 1 {
        out.insert(cjk[0].to_string());
    }
    for w in cjk.windows(2) {
        out.insert(w.iter().collect());
    }
    out.into_iter().collect()
}

/// Score a memory against query terms: +2 per term found in a tag, +1 per term
/// found in the content.
fn score(terms: &[String], m: &MemoryEntry) -> i64 {
    if terms.is_empty() {
        return 0;
    }
    let content = m.content.to_lowercase();
    let tags = m.tags.join(" ").to_lowercase();
    let mut s = 0i64;
    for t in terms {
        if tags.contains(t) {
            s += 2;
        } else if content.contains(t) {
            s += 1;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::models::new_id;

    fn service() -> MemoryService {
        let path = std::env::temp_dir().join(format!("agentd_mem_{}.redb", new_id("t")));
        MemoryService::new(Arc::new(Store::open(path).unwrap()))
    }

    #[test]
    fn upsert_dedupes_same_scope() {
        let svc = service();
        let a = svc
            .upsert("global", "preference", "用户偏好简体中文回复", vec![], None)
            .unwrap();
        let b = svc
            .upsert(
                "global",
                "preference",
                "用户偏好简体中文回复",
                vec!["lang".into()],
                None,
            )
            .unwrap();
        assert_eq!(
            a.id, b.id,
            "identical content updates instead of duplicating"
        );
        assert_eq!(svc.list_by_scope("global").len(), 1);
        assert!(b.tags.contains(&"lang".to_string()));
    }

    #[test]
    fn search_ranks_keyword_matches() {
        let svc = service();
        svc.upsert("global", "fact", "项目使用 redb 作为存储引擎", vec![], None)
            .unwrap();
        svc.upsert("global", "fact", "前端使用 GPUI 框架", vec![], None)
            .unwrap();
        let hits = svc.search("redb 存储", &["global".to_string()], 5);
        assert!(!hits.is_empty());
        assert!(hits[0].content.contains("redb"));
    }

    #[test]
    fn search_scopes_are_isolated() {
        let svc = service();
        svc.upsert("session:s1", "fact", "alpha detail", vec![], None)
            .unwrap();
        svc.upsert("session:s2", "fact", "beta detail", vec![], None)
            .unwrap();
        let hits = svc.search("detail", &["session:s1".to_string()], 5);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("alpha"));
    }

    #[test]
    fn delete_removes_entry() {
        let svc = service();
        let m = svc
            .upsert("global", "fact", "to be deleted", vec![], None)
            .unwrap();
        assert!(svc.delete(&m.id).is_ok());
        assert!(svc.get(&m.id).is_err());
    }
}
