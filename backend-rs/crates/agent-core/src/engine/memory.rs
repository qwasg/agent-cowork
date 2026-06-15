//! Runtime-handled memory tools: `memory_write` / `memory_search` /
//! `memory_delete`. These let the agent build up and consult durable
//! cross-session knowledge via [`crate::memory::MemoryService`].

use serde_json::json;

use agent_protocol::models::MemoryEntry;
use agent_protocol::{ApiError, ApiResult};
use agent_providers::types::ToolSpec;

use super::Runtime;

impl Runtime {
    /// Scopes a session can read/write, broadest to narrowest:
    /// `global`, the current `workspace:{root}`, and `session:{id}`.
    pub(crate) fn memory_scopes(&self, session_id: &str) -> Vec<String> {
        vec![
            "global".to_string(),
            format!("workspace:{}", self.workspace_root().display()),
            format!("session:{session_id}"),
        ]
    }

    /// Resolve a user-facing scope keyword (`global` / `workspace` / `session`)
    /// to a concrete scope string.
    fn resolve_scope(&self, session_id: &str, keyword: &str) -> String {
        match keyword.trim().to_ascii_lowercase().as_str() {
            "global" => "global".to_string(),
            "session" => format!("session:{session_id}"),
            _ => format!("workspace:{}", self.workspace_root().display()),
        }
    }

    /// Retrieve relevant memory lines for the system prompt, given the current
    /// user input as the query.
    pub(crate) fn retrieve_memory_lines(&self, session_id: &str, query: &str) -> Vec<String> {
        let scopes = self.memory_scopes(session_id);
        self.memory
            .search(query, &scopes, 10)
            .iter()
            .map(render_memory_line)
            .collect()
    }

    pub(crate) fn handle_memory_tool(
        &self,
        session_id: &str,
        run_id: &str,
        name: &str,
        args: &serde_json::Value,
    ) -> ApiResult<String> {
        match name {
            "memory_write" => self.handle_memory_write(session_id, run_id, args),
            "memory_search" => self.handle_memory_search(session_id, args),
            "memory_delete" => self.handle_memory_delete(args),
            other => Err(ApiError::new(
                "TOOL_NOT_FOUND",
                format!("unknown memory tool: {other}"),
            )),
        }
    }

    fn handle_memory_write(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> ApiResult<String> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "content is required"));
        }
        let kind = match args.get("kind").and_then(|v| v.as_str()).unwrap_or("fact") {
            "preference" => "preference",
            "convention" => "convention",
            _ => "fact",
        };
        let scope_kw = args
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("workspace");
        let scope = self.resolve_scope(session_id, scope_kw);
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let entry =
            self.memory
                .upsert(&scope, kind, content, tags, Some(session_id.to_string()))?;
        self.emit(
            session_id,
            "agent.memory.created",
            "memory",
            json!({
                "id": entry.id,
                "scope": entry.scope,
                "kind": entry.kind,
                "content": entry.content,
                "tags": entry.tags,
            }),
            Some(run_id.to_string()),
        );
        Ok(format!(
            "已记忆（{}，scope={}）：{}",
            entry.kind,
            scope_label(&entry.scope),
            entry.content
        ))
    }

    fn handle_memory_search(
        &self,
        session_id: &str,
        args: &serde_json::Value,
    ) -> ApiResult<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "query is required"));
        }
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, 20))
            .unwrap_or(8);
        let scopes = self.memory_scopes(session_id);
        let hits = self.memory.search(query, &scopes, limit);
        if hits.is_empty() {
            return Ok("（没有匹配的记忆）".to_string());
        }
        let lines: Vec<String> = hits
            .iter()
            .map(|m| format!("- id={} {}", m.id, render_memory_line(m)))
            .collect();
        Ok(format!(
            "匹配到 {} 条记忆：\n{}",
            hits.len(),
            lines.join("\n")
        ))
    }

    fn handle_memory_delete(&self, args: &serde_json::Value) -> ApiResult<String> {
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if id.is_empty() {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "id is required"));
        }
        self.memory.delete(id)?;
        Ok(format!("已删除记忆 {id}"))
    }
}

/// Human-readable scope label for tool output.
fn scope_label(scope: &str) -> &str {
    if scope == "global" {
        "global"
    } else if scope.starts_with("session:") {
        "session"
    } else {
        "workspace"
    }
}

/// One-line rendering of a memory entry for prompt injection / search output.
pub(crate) fn render_memory_line(m: &MemoryEntry) -> String {
    let tags = if m.tags.is_empty() {
        String::new()
    } else {
        format!(" #{}", m.tags.join(" #"))
    };
    format!(
        "[{}/{}] {}{}",
        scope_label(&m.scope),
        m.kind,
        m.content,
        tags
    )
}

pub(crate) fn memory_write_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "memory_write".to_string(),
        description: "写入一条长期记忆，跨会话保留。适用于用户明确的偏好、关于项目/领域的稳定事实、\
需长期遵守的约定或重要结论。不要记录一次性的、易过期的或可直接从文件读到的信息，也不要重复写入已有记忆。"
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "一句话、自包含的记忆内容"},
                "kind": {"type": "string", "enum": ["preference", "fact", "convention"], "description": "preference=用户偏好，fact=事实，convention=约定"},
                "scope": {"type": "string", "enum": ["global", "workspace", "session"], "description": "记忆作用域：global=全局，workspace=当前工作区（默认），session=仅本会话"},
                "tags": {"type": "array", "items": {"type": "string"}, "description": "便于检索的标签（可选）"}
            },
            "required": ["content"]
        }),
    }
}

pub(crate) fn memory_search_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "memory_search".to_string(),
        description: "按关键词检索长期记忆（覆盖 global / 当前工作区 / 本会话）。\
系统已在 system prompt 中预置了与本轮最相关的记忆；仅当需要查找更多或更具体的记忆时才调用本工具。"
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "检索关键词"},
                "limit": {"type": "integer", "description": "返回条数上限（1-20，默认 8）"}
            },
            "required": ["query"]
        }),
    }
}

pub(crate) fn memory_delete_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "memory_delete".to_string(),
        description: "删除一条不再正确或已过期的长期记忆（通过 memory_search 返回的 id）。"
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "要删除的记忆 id"}
            },
            "required": ["id"]
        }),
    }
}
