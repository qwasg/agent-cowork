//! Persistent key/value config store. Native replacement for the browser
//! `localStorage` used by the React app (all `moonlit:*` keys).
//!
//! Backed by a single JSON file in the OS app-data directory. Values are
//! arbitrary JSON, matching how the web app stored JSON-encoded strings.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Well-known config keys, ported 1:1 from the web app so behavior is
/// identical. Not exhaustive, but documents the contract.
pub mod keys {
    pub const API_BASE_URL: &str = "moonlit:apiBaseUrl";
    pub const AUTH_TOKEN: &str = "moonlit:auth:token";
    pub const AUTH_USER: &str = "moonlit:auth:user";
    pub const SELECTED_SESSION: &str = "moonlit:selectedSession:v2";
    pub const THEME: &str = "moonlit:theme";
    pub const MODE: &str = "moonlit:mode";
    pub const PANE_SIZES: &str = "moonlit:paneSizes";
    pub const WORKSPACE_ROOT: &str = "moonlit:workspaceRoot";
    pub const RECENT_WORKSPACES: &str = "moonlit:recentWorkspaces";
    pub const RECENT_FILES: &str = "moonlit:recentFiles";
    pub const AUTO_APPROVE: &str = "moonlit:autoApprove";
    pub const BOTTOM_OPEN: &str = "moonlit:bottomOpen";
    pub const BOTTOM_HEIGHT: &str = "moonlit:bottomH";
}

/// Default backend base URL (Document Compiler copy uses 8002).
pub const DEFAULT_API_BASE_URL: &str = "http://127.0.0.1:8002";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// A JSON-file-backed key/value store, safe to share across threads.
pub struct ConfigStore {
    path: PathBuf,
    data: Mutex<BTreeMap<String, Value>>,
}

impl ConfigStore {
    /// Open (or create) a store at `path`, loading any existing contents.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let data = if path.exists() {
            let bytes = std::fs::read(&path)?;
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    /// Open the default store under the app-data directory for `app_name`.
    pub fn open_for_app(app_name: &str) -> Result<Self, StoreError> {
        let dir = app_data_dir(app_name);
        std::fs::create_dir_all(&dir)?;
        Self::open(dir.join("config.json"))
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.data.lock().unwrap().get(key).cloned()
    }

    /// Read a string value (the web app stored most things as JSON strings).
    pub fn get_string(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::String(s) => Some(s),
            other => Some(other.to_string()),
        }
    }

    pub fn get_string_or(&self, key: &str, default: &str) -> String {
        self.get_string(key).unwrap_or_else(|| default.to_string())
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|v| v.as_bool())
    }

    /// Set a value and persist immediately.
    pub fn set(&self, key: &str, value: Value) -> Result<(), StoreError> {
        {
            let mut data = self.data.lock().unwrap();
            data.insert(key.to_string(), value);
        }
        self.flush()
    }

    pub fn set_string(&self, key: &str, value: impl Into<String>) -> Result<(), StoreError> {
        self.set(key, Value::String(value.into()))
    }

    pub fn remove(&self, key: &str) -> Result<(), StoreError> {
        {
            let mut data = self.data.lock().unwrap();
            data.remove(key);
        }
        self.flush()
    }

    /// Convenience for the most-used setting.
    pub fn api_base_url(&self) -> String {
        self.get_string_or(keys::API_BASE_URL, DEFAULT_API_BASE_URL)
    }

    pub fn auth_token(&self) -> Option<String> {
        self.get_string(keys::AUTH_TOKEN).filter(|s| !s.is_empty())
    }

    fn flush(&self) -> Result<(), StoreError> {
        let data = self.data.lock().unwrap();
        let bytes = serde_json::to_vec_pretty(&*data)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write-rename for atomicity.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Resolve a per-user app-data directory without extra dependencies.
fn app_data_dir(app_name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join(app_name);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(app_name);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(app_name);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join(app_name);
    }
    std::env::temp_dir().join(app_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_get_remove_round_trip() {
        let dir = std::env::temp_dir().join(format!("moonlit-store-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cfg.json");
        let _ = std::fs::remove_file(&path);

        let store = ConfigStore::open(&path).unwrap();
        assert_eq!(store.api_base_url(), DEFAULT_API_BASE_URL);

        store
            .set_string(keys::API_BASE_URL, "http://localhost:9999")
            .unwrap();
        store.set(keys::AUTO_APPROVE, json!(true)).unwrap();

        // Reload from disk to verify persistence.
        let reloaded = ConfigStore::open(&path).unwrap();
        assert_eq!(reloaded.api_base_url(), "http://localhost:9999");
        assert_eq!(reloaded.get_bool(keys::AUTO_APPROVE), Some(true));

        reloaded.remove(keys::API_BASE_URL).unwrap();
        assert_eq!(reloaded.api_base_url(), DEFAULT_API_BASE_URL);
    }
}
