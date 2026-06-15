//! Web-search (Tavily) configuration: full model + encrypted persistence.
//!
//! Port of the Python `domain/search_config.py` + `infra/search_config_store.py`
//! pair onto the Rust stack: the whole config is stored as JSON under the redb
//! KV key `search_config`, with `api_key` encrypted via [`CryptoStore`]
//! (AES-256-GCM). Field values are normalized on save exactly like
//! `rest_gateway.set_search_config` (invalid values fall back to the previous /
//! default value instead of erroring).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agent_config::Config;
use agent_protocol::models::now_ts;
use agent_store::{CryptoStore, Store};

const KV_KEY: &str = "search_config";
/// Legacy stub storage (apiKey only) migrated on startup.
const LEGACY_KV_KEY: &str = "search_api_key";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub enabled: bool,
    pub provider: String,
    /// Encrypted at rest (`enc:...`); decrypted copies never persist.
    pub api_key: String,
    pub topic: String,
    pub search_depth: String,
    pub time_range: String,
    pub extract_depth: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            enabled: false,
            provider: "tavily".to_string(),
            api_key: String::new(),
            topic: "general".to_string(),
            search_depth: "basic".to_string(),
            time_range: String::new(),
            extract_depth: "basic".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

fn pick_enum(raw: Option<&str>, allowed: &[&str], fallback: &str) -> String {
    match raw.map(|s| s.trim().to_lowercase()) {
        Some(v) if allowed.contains(&v.as_str()) => v,
        _ => fallback.to_string(),
    }
}

pub struct SearchConfigService {
    store: Arc<Store>,
    crypto: Arc<CryptoStore>,
    env_api_key: Option<String>,
    pub base_url: String,
}

impl SearchConfigService {
    pub fn new(store: Arc<Store>, crypto: Arc<CryptoStore>, cfg: &Config) -> Arc<Self> {
        let svc = SearchConfigService {
            store,
            crypto,
            env_api_key: cfg.tavily_api_key.clone(),
            base_url: cfg.tavily_base_url.clone(),
        };
        svc.migrate_legacy();
        Arc::new(svc)
    }

    /// One-shot migration: the old stub stored only an encrypted apiKey under
    /// `search_api_key`. Fold it into a full config (enabled, since the user
    /// explicitly configured a key) and drop the legacy entry.
    fn migrate_legacy(&self) {
        let Some(legacy_key) = self.store.kv_get(LEGACY_KV_KEY) else {
            return;
        };
        if self.store.kv_get(KV_KEY).is_none() && !legacy_key.is_empty() {
            let now = now_ts();
            let config = SearchConfig {
                enabled: true,
                api_key: legacy_key,
                created_at: now.clone(),
                updated_at: now,
                ..SearchConfig::default()
            };
            self.persist(&config);
            tracing::info!("search-config: migrated legacy search_api_key entry");
        }
        let _ = self.store.kv_delete(LEGACY_KV_KEY);
    }

    fn persist(&self, config: &SearchConfig) {
        match serde_json::to_string(config) {
            Ok(blob) => {
                let _ = self.store.kv_put(KV_KEY, &blob);
            }
            Err(e) => tracing::warn!("search-config: serialize failed: {e}"),
        }
    }

    /// Stored config with the api_key still encrypted (safe to log shape).
    pub fn get_stored(&self) -> SearchConfig {
        self.store
            .kv_get(KV_KEY)
            .and_then(|blob| serde_json::from_str(&blob).ok())
            .unwrap_or_default()
    }

    /// True iff the user has ever saved a config (used to keep pure-env
    /// `TAVILY_API_KEY` workflows working without an explicit enable).
    pub fn has_stored(&self) -> bool {
        self.store.kv_get(KV_KEY).is_some()
    }

    /// Resolve the plaintext API key: stored (decrypted) first, env fallback.
    pub fn resolve_api_key(&self) -> Option<String> {
        let stored = self.get_stored().api_key;
        if !stored.is_empty() {
            let plain = self.crypto.decrypt(&stored);
            if !plain.is_empty() {
                return Some(plain);
            }
        }
        self.env_api_key.clone()
    }

    /// Effective on/off for the `web_search` tool:
    /// - explicit stored config → its `enabled` flag
    /// - no stored config → enabled iff `TAVILY_API_KEY` env is set (legacy)
    pub fn effectively_enabled(&self) -> bool {
        if self.has_stored() {
            self.get_stored().enabled
        } else {
            self.env_api_key.is_some()
        }
    }

    /// Apply a camelCase REST patch with Python-parity normalization and save.
    pub fn save_patch(&self, payload: &Value) -> SearchConfig {
        let existing = self.get_stored();
        let s = |k: &str| payload.get(k).and_then(|v| v.as_str());

        let topic = pick_enum(s("topic"), &["general", "news"], &existing.topic);
        let topic = if topic.is_empty() {
            "general".into()
        } else {
            topic
        };
        let search_depth = pick_enum(
            s("searchDepth"),
            &["basic", "advanced"],
            &existing.search_depth,
        );
        let search_depth = if search_depth.is_empty() {
            "basic".into()
        } else {
            search_depth
        };
        let time_range = pick_enum(
            s("timeRange"),
            &["", "day", "week", "month", "year"],
            &existing.time_range,
        );
        let extract_depth = pick_enum(
            s("extractDepth"),
            &["basic", "advanced"],
            &existing.extract_depth,
        );
        let extract_depth = if extract_depth.is_empty() {
            "basic".into()
        } else {
            extract_depth
        };

        // Empty / missing apiKey keeps the previously stored secret.
        let api_key = match s("apiKey").map(str::trim) {
            Some(key) if !key.is_empty() => self.crypto.encrypt(key),
            _ => existing.api_key.clone(),
        };

        let now = now_ts();
        let config = SearchConfig {
            enabled: payload
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(existing.enabled),
            provider: "tavily".to_string(),
            api_key,
            topic,
            search_depth,
            time_range,
            extract_depth,
            created_at: if existing.created_at.is_empty() {
                now.clone()
            } else {
                existing.created_at.clone()
            },
            updated_at: now,
        };
        self.persist(&config);
        config
    }

    /// REST view, aligned with the Python `_search_config_to_dict` shape.
    pub fn public_view(&self) -> Value {
        let c = self.get_stored();
        let api_key_set = !c.api_key.is_empty() || self.env_api_key.is_some();
        json!({
            "enabled": c.enabled,
            "provider": if c.provider.is_empty() { "tavily".to_string() } else { c.provider },
            "apiKeySet": api_key_set,
            "topic": c.topic,
            "searchDepth": c.search_depth,
            "timeRange": c.time_range,
            "extractDepth": c.extract_depth,
            "createdAt": c.created_at,
            "updatedAt": c.updated_at,
        })
    }
}
