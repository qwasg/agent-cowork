//! Typed configuration loaded from environment + optional `.env.local` / `.env`.
//!
//! Replaces the scattered `os.getenv` calls in the Python backend and the
//! hard-coded interpreter path in `apps/agent-ide/scripts/dev-full.mjs`.

use std::path::PathBuf;

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_u16(key: &str, default: u16) -> u16 {
    env_opt(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env_opt(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env_opt(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_bool(key: &str) -> bool {
    matches!(
        env_opt(key).as_deref(),
        Some("1") | Some("true") | Some("True") | Some("yes")
    )
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub http_port: u16,
    pub data_dir: PathBuf,
    pub session_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub event_buffer_cap: Option<usize>,
    pub persist_events: bool,
    pub parallel_limit: usize,
    /// Max concurrent `edit`-kind todos in plan execution (default 1: edits
    /// run serially to avoid write conflicts; explore todos use
    /// `parallel_limit`).
    pub edit_parallel: usize,
    pub tool_loop_steps: usize,
    pub repeated_tool_limit: usize,
    pub stream: bool,
    pub history_turns: usize,
    /// Total per-request timeout for LLM provider HTTP calls (seconds).
    pub provider_timeout_secs: u64,
    /// Token budget for in-loop context compaction.
    pub context_budget_tokens: usize,
    /// How long a pending tool-permission request waits before being denied.
    pub approval_timeout_secs: u64,
    /// Allow `/local-file` and `/workspace/browse` to escape the workspace root.
    pub allow_local_fs: bool,
    /// Allow `web_fetch` to reach private / internal addresses.
    pub web_fetch_allow_private: bool,

    // LLM provider env
    pub openai_api_key: Option<String>,
    pub openai_base_url: String,
    pub openai_model: String,
    pub anthropic_api_key: Option<String>,
    pub anthropic_base_url: String,

    // Tavily web search
    pub tavily_api_key: Option<String>,
    pub tavily_base_url: String,
    pub web_fetch_max_chars: usize,
}

impl Config {
    pub fn load() -> Self {
        // Load .env.local first (personal), then .env (team), never overriding
        // an already-set variable (matches dev_llm_defaults.setdefault semantics).
        let _ = dotenvy::from_filename(".env.local");
        let _ = dotenvy::dotenv();

        let data_dir = env_opt("AGENT_DEBUG_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let session_dir = env_opt("AGENT_DEBUG_SESSION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("agent-sessions"));
        let workspace_root = env_opt("AGENT_DEBUG_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let event_buffer_cap = match env_opt("AGENT_DEBUG_EVENT_BUFFER").as_deref() {
            None => Some(4096), // bounded by default to avoid unbounded OOM (tech-debt fix)
            Some("0") | Some("unlimited") | Some("none") | Some("inf") => None,
            Some(v) => v.parse::<usize>().ok().map(|n| n.max(64)).or(Some(4096)),
        };

        Config {
            host: env_str("AGENT_DEBUG_HOST", "127.0.0.1"),
            http_port: env_u16("AGENT_DEBUG_HTTP_PORT", 8002),
            data_dir,
            session_dir,
            workspace_root,
            event_buffer_cap,
            persist_events: env_opt("AGENT_DEBUG_PERSIST_EVENTS")
                .map(|v| v != "0" && v != "false")
                .unwrap_or(true),
            parallel_limit: env_usize("AGENT_DEBUG_PARALLEL_LIMIT", 4).max(1),
            edit_parallel: env_usize("AGENT_DEBUG_EDIT_PARALLEL", 1).max(1),
            tool_loop_steps: env_usize("AGENT_DEBUG_TOOL_LOOP_STEPS", 24),
            repeated_tool_limit: env_usize("AGENT_DEBUG_REPEATED_TOOL_LIMIT", 8).max(1),
            stream: env_opt("AGENT_DEBUG_STREAM")
                .map(|v| v != "0")
                .unwrap_or(true),
            history_turns: env_usize("AGENT_DEBUG_HISTORY_TURNS", 40).max(1),
            provider_timeout_secs: env_u64("AGENT_DEBUG_PROVIDER_TIMEOUT_SECS", 300).max(10),
            context_budget_tokens: env_usize("AGENT_DEBUG_CONTEXT_BUDGET_TOKENS", 96_000)
                .max(4_096),
            approval_timeout_secs: env_u64("AGENT_DEBUG_APPROVAL_TIMEOUT_SECS", 120).max(5),
            allow_local_fs: env_bool("AGENT_DEBUG_ALLOW_LOCAL_FS"),
            web_fetch_allow_private: env_bool("AGENT_DEBUG_WEB_FETCH_ALLOW_PRIVATE"),

            openai_api_key: env_opt("OPENAI_API_KEY"),
            openai_base_url: env_str("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            openai_model: env_str("OPENAI_MODEL", "gpt-4o-mini"),
            anthropic_api_key: env_opt("ANTHROPIC_API_KEY"),
            anthropic_base_url: env_str("ANTHROPIC_BASE_URL", "https://api.anthropic.com"),

            tavily_api_key: env_opt("TAVILY_API_KEY")
                .or_else(|| env_opt("AGENT_DEBUG_WEB_SEARCH_API_KEY")),
            tavily_base_url: env_str("AGENT_DEBUG_TAVILY_BASE_URL", "https://api.tavily.com"),
            web_fetch_max_chars: env_usize("AGENT_DEBUG_WEB_FETCH_MAX_CHARS", 20000),
        }
    }
}
