//! Code-edit proposal engine + durable proposal registry (port of
//! `domain/code_edit_engine.py`).
//!
//! Two generation paths, mirroring the Python engine:
//! 1. `maybe_generate` — deterministic heuristic (TS2322 string→number repair)
//!    so offline / mock mode still produces a proposal.
//! 2. `propose_with_llm` — asks the configured LLM for strict-JSON
//!    `{summary, changes:[{path, originalContent, proposedContent, ...}]}`,
//!    falling back to the heuristic when the output is unusable.
//!
//! Proposals are persisted in redb (improvement over the in-memory Python
//! registry) so apply/discard survives a restart.

use std::sync::Arc;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agent_protocol::models::{new_id, now_ts, ChatMessage};
use agent_providers::types::{noop_sink, ProviderRequest};
use agent_providers::ProviderExecutionService;
use agent_store::store::{IDX_PROPOSALS_BY_SESSION, T_PROPOSALS};
use agent_store::Store;

pub const SCHEMA_VERSION: &str = "ai-edit-proposal/v1";

/// Wire/storage shape matches Python `proposal_to_dict` exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeEditProposal {
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub schema_version: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub applied_at: Option<String>,
    #[serde(default)]
    pub discarded_at: Option<String>,
    #[serde(default)]
    pub applied_files: Vec<String>,
    #[serde(default)]
    pub apply_error: Option<String>,
    #[serde(default)]
    pub changes: Vec<Value>,
    #[serde(default)]
    pub created_at: String,
}

/// Durable registry for apply/discard (port of `ProposalRegistry`).
pub struct ProposalRegistry {
    store: Arc<Store>,
}

impl ProposalRegistry {
    pub fn new(store: Arc<Store>) -> Self {
        ProposalRegistry { store }
    }

    /// Persist a raw proposal payload (the `agent.code_edit.proposed` event
    /// payload shape) as a pending proposal record.
    pub fn record(&self, payload: &Value) -> CodeEditProposal {
        let proposal = CodeEditProposal {
            id: payload
                .get("proposalId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| new_id("edit")),
            session_id: payload
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            schema_version: payload
                .get("schemaVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(SCHEMA_VERSION)
                .to_string(),
            summary: payload
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            source: payload
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("agent-debug")
                .to_string(),
            status: "pending".to_string(),
            applied_at: None,
            discarded_at: None,
            applied_files: Vec::new(),
            apply_error: None,
            changes: payload
                .get("changes")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default(),
            created_at: payload
                .get("createdAt")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(now_ts),
        };
        self.save(&proposal);
        proposal
    }

    pub fn save(&self, p: &CodeEditProposal) {
        let _ = self.store.put(T_PROPOSALS, &p.id, p);
        let _ = self
            .store
            .index_add(IDX_PROPOSALS_BY_SESSION, &p.session_id, &p.id);
    }

    pub fn get(&self, id: &str) -> Option<CodeEditProposal> {
        self.store
            .get::<CodeEditProposal>(T_PROPOSALS, id)
            .ok()
            .flatten()
    }

    pub fn list_for_session(&self, session_id: &str) -> Vec<CodeEditProposal> {
        let ids = self
            .store
            .index_values(IDX_PROPOSALS_BY_SESSION, session_id);
        let mut out: Vec<CodeEditProposal> = if ids.is_empty() {
            self.store
                .list::<CodeEditProposal>(T_PROPOSALS)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.session_id == session_id)
                .collect()
        } else {
            ids.iter().filter_map(|id| self.get(id)).collect()
        };
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        out
    }

    pub fn all(&self) -> Vec<CodeEditProposal> {
        let mut out = self
            .store
            .list::<CodeEditProposal>(T_PROPOSALS)
            .unwrap_or_default();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        out
    }

    pub fn mark_applied(&self, id: &str, applied_files: Vec<String>) -> Option<CodeEditProposal> {
        let mut p = self.get(id)?;
        p.status = "applied".to_string();
        p.applied_at = Some(now_ts());
        p.applied_files = applied_files;
        p.apply_error = None;
        self.save(&p);
        Some(p)
    }

    pub fn mark_apply_failed(&self, id: &str, error: &str) -> Option<CodeEditProposal> {
        let mut p = self.get(id)?;
        p.apply_error = Some(error.to_string());
        self.save(&p);
        Some(p)
    }

    pub fn mark_discarded(&self, id: &str) -> Option<CodeEditProposal> {
        let mut p = self.get(id)?;
        p.status = "discarded".to_string();
        p.discarded_at = Some(now_ts());
        self.save(&p);
        Some(p)
    }
}

// ------------------------------------------------------------------ engine

/// Heuristic path: deterministic TS2322 string→number repair against the
/// active file from the session context window. Returns the proposal payload
/// (camelCase, same shape as the `agent.code_edit.proposed` event).
pub fn maybe_generate(
    session_id: &str,
    user_input: &str,
    context_window: Option<&Value>,
) -> Option<Value> {
    let cw = context_window?;
    let active_file = cw.get("activeFile")?.as_object()?;
    let path = active_file.get("path")?.as_str()?.trim().to_string();
    let language = active_file
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("plaintext")
        .trim()
        .to_string();
    let language = if language.is_empty() {
        "plaintext".to_string()
    } else {
        language
    };
    let original = active_file
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if path.is_empty() || original.is_empty() {
        return None;
    }
    let terminal_output = cw
        .get("terminalRecentOutput")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let proposed = propose_content(&language, &original, &terminal_output, user_input);
    if proposed == original {
        return None;
    }
    Some(json!({
        "schemaVersion": SCHEMA_VERSION,
        "proposalId": new_id("edit"),
        "sessionId": session_id,
        "source": "agent-debug",
        "summary": build_summary(&path, &terminal_output, user_input),
        "createdAt": now_ts(),
        "changes": [{
            "changeId": new_id("change"),
            "path": path,
            "language": language,
            "description": build_change_description(&terminal_output, user_input),
            "strategy": "replace-file",
            "originalContent": original,
            "proposedContent": proposed,
            "selection": normalize_selection(cw.get("selection")),
        }],
    }))
}

fn propose_content(
    language: &str,
    original: &str,
    terminal_output: &str,
    user_input: &str,
) -> String {
    let lang = language.to_lowercase();
    let lowered_output = terminal_output.to_lowercase();
    let lowered_input = user_input.to_lowercase();
    if matches!(lang.as_str(), "typescript" | "javascript" | "tsx" | "jsx") {
        let re = Regex::new(r"(:\s*number\s*=\s*)'(\d+)'").unwrap();
        let replaced = re.replace(original, "$1$2").to_string();
        if replaced != original
            && (lowered_output.contains("ts2322")
                || lowered_output.contains("type 'string' is not assignable to type 'number'")
                || lowered_input.contains("number"))
        {
            return replaced;
        }
    }
    original.to_string()
}

fn build_summary(path: &str, terminal_output: &str, user_input: &str) -> String {
    if terminal_output.contains("TS2322")
        || terminal_output.contains("type 'string' is not assignable to type 'number'")
    {
        return format!("根据 TypeScript 类型报错，为 `{path}` 生成了一个候选修复。");
    }
    if !user_input.trim().is_empty() {
        return format!("根据当前会话请求，为 `{path}` 生成了一个候选修改。");
    }
    format!("为 `{path}` 生成了一个候选修改。")
}

fn build_change_description(terminal_output: &str, user_input: &str) -> String {
    if terminal_output.contains("TS2322")
        || terminal_output.contains("type 'string' is not assignable to type 'number'")
    {
        return "将错误的字符串数字字面量改为 number，以匹配现有类型声明。".to_string();
    }
    let trimmed = user_input.trim();
    if !trimmed.is_empty() {
        return format!("结合当前请求“{trimmed}”生成的修改建议。");
    }
    "根据当前上下文生成的修改建议。".to_string()
}

fn normalize_selection(value: Option<&Value>) -> Value {
    let Some(obj) = value.and_then(|v| v.as_object()) else {
        return Value::Null;
    };
    let int_of = |k: &str| obj.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    json!({
        "start": int_of("start"),
        "end": int_of("end"),
        "selectedText": obj.get("selectedText").and_then(|v| v.as_str()).unwrap_or(""),
        "startLine": int_of("startLine"),
        "endLine": int_of("endLine"),
    })
}

const SYSTEM_DIFF_PROMPT: &str = "You are a senior code-edit reviewer. Given the user objective and the \
current file content, return STRICT JSON with this shape:\n\
{\"summary\": str, \"changes\": [{\"path\": str, \"language\": str, \
\"description\": str, \"originalContent\": str, \"proposedContent\": str}]}\n\
Rules:\n\
- ``proposedContent`` must be the FULL replacement file content, preserving unrelated lines exactly.\n\
- Make the smallest correct change that satisfies the objective.\n\
- If you cannot suggest a safe change, return {\"changes\": []}.";

/// LLM path with heuristic fallback (port of `propose_with_llm`).
pub async fn propose_with_llm(
    providers: &Arc<ProviderExecutionService>,
    session_id: &str,
    user_input: &str,
    context_window: Option<&Value>,
    model: &str,
) -> Option<Value> {
    let cw = context_window?;
    let active_file = cw.get("activeFile")?.as_object()?;
    let path = active_file.get("path")?.as_str()?.trim().to_string();
    let language = active_file
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("plaintext")
        .to_string();
    let original = active_file
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if path.is_empty() || original.is_empty() {
        return None;
    }
    let terminal: String = cw
        .get("terminalRecentOutput")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(1500)
        .collect();
    let objective = if user_input.trim().is_empty() {
        "修复当前文件中的明显问题"
    } else {
        user_input.trim()
    };
    let user_msg = format!(
        "Objective: {objective}\nFile path: {path}\nLanguage: {language}\nTerminal output: {terminal}\n----\nCurrent file content:\n```\n{original}\n```"
    );
    let req = ProviderRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(SYSTEM_DIFF_PROMPT),
            ChatMessage::user(user_msg),
        ],
        tools: vec![],
        temperature: Some(0.1),
        stream: false,
        max_tokens: None,
    };
    let sink = noop_sink();
    let fallback = || maybe_generate(session_id, user_input, context_window);
    let resp = match providers.execute(&req, &sink, None).await {
        Ok(r) if !r.degraded => r,
        _ => return fallback(),
    };
    let Some(payload) = extract_json_object(&resp.text) else {
        return fallback();
    };
    let Some(changes) = payload.get("changes").and_then(|v| v.as_array()) else {
        return fallback();
    };
    let mut normalised: Vec<Value> = Vec::new();
    for change in changes {
        let Some(obj) = change.as_object() else {
            continue;
        };
        let change_path = obj
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(&path)
            .to_string();
        let proposed = obj
            .get("proposedContent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let orig = obj
            .get("originalContent")
            .and_then(|v| v.as_str())
            .unwrap_or(&original)
            .to_string();
        if proposed == orig {
            continue;
        }
        normalised.push(json!({
            "changeId": new_id("change"),
            "path": change_path,
            "language": obj.get("language").and_then(|v| v.as_str()).unwrap_or(&language),
            "description": obj.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "strategy": "replace-file",
            "originalContent": orig,
            "proposedContent": proposed,
            "selection": Value::Null,
        }));
    }
    if normalised.is_empty() {
        return fallback();
    }
    Some(json!({
        "schemaVersion": SCHEMA_VERSION,
        "proposalId": new_id("edit"),
        "sessionId": session_id,
        "source": "agent-debug-llm",
        "summary": payload.get("summary").and_then(|v| v.as_str()).unwrap_or("Agent 提议的代码修改"),
        "createdAt": now_ts(),
        "changes": normalised,
    }))
}

/// Extract the first JSON object from free-form LLM output.
fn extract_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts_context() -> Value {
        json!({
            "activeFile": {
                "path": "src/app.ts",
                "language": "typescript",
                "content": "const total: number = '42';\n",
            },
            "terminalRecentOutput": "error TS2322: Type 'string' is not assignable to type 'number'.",
        })
    }

    #[test]
    fn heuristic_repairs_ts2322() {
        let p = maybe_generate("sess_1", "修复类型错误", Some(&ts_context())).unwrap();
        let change = &p["changes"][0];
        assert_eq!(change["path"], "src/app.ts");
        assert_eq!(change["proposedContent"], "const total: number = 42;\n");
        assert_eq!(p["schemaVersion"], SCHEMA_VERSION);
    }

    #[test]
    fn heuristic_skips_without_active_file() {
        assert!(maybe_generate("s", "fix", Some(&json!({}))).is_none());
        assert!(maybe_generate("s", "fix", None).is_none());
    }

    #[test]
    fn registry_round_trips_and_applies() {
        let path = std::env::temp_dir().join(format!("agentd_prop_{}.redb", new_id("t")));
        let store = Arc::new(Store::open(path).unwrap());
        let reg = ProposalRegistry::new(store);
        let payload = maybe_generate("sess_reg", "fix number", Some(&ts_context())).unwrap();
        let rec = reg.record(&payload);
        assert_eq!(rec.status, "pending");
        assert_eq!(reg.list_for_session("sess_reg").len(), 1);
        let applied = reg
            .mark_applied(&rec.id, vec!["src/app.ts".to_string()])
            .unwrap();
        assert_eq!(applied.status, "applied");
        assert_eq!(applied.applied_files, vec!["src/app.ts"]);
        let discarded = reg.mark_discarded(&rec.id).unwrap();
        assert_eq!(discarded.status, "discarded");
    }
}
