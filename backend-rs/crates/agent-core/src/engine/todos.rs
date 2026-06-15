//! Runtime-handled checklist tools: `todo_write` / `todo_update`.

use serde_json::json;

use agent_protocol::models::now_ts;
use agent_protocol::{ApiError, ApiResult};
use agent_providers::types::ToolSpec;

use super::Runtime;

impl Runtime {
    /// Shared todo batch creation for `todo_write` / `plan_write`.
    fn write_todo_batch(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> usize {
        let mut created_ids: Vec<String> = Vec::new();
        if let Some(items) = args.get("todos").and_then(|v| v.as_array()) {
            for item in items {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if title.is_empty() {
                    continue;
                }
                let desc = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("edit");
                let deps: Vec<String> = item
                    .get("dependsOn")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_u64().map(|n| n as usize))
                            .filter(|&n| n < created_ids.len())
                            .map(|n| created_ids[n].clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let todo = self.todos.add(session_id, title, desc, kind, deps.clone());
                self.emit(
                    session_id,
                    "todo.created",
                    "todo",
                    json!({
                        "id": todo.id,
                        "title": todo.title,
                        "kind": todo.kind,
                        "status": todo.status,
                        "description": todo.description,
                        "dependencies": deps,
                    }),
                    Some(run_id.to_string()),
                );
                created_ids.push(todo.id);
            }
        }
        created_ids.len()
    }

    /// `todo_write`: create a batch of todos. Items support `kind`
    /// (explore/edit) and `dependsOn` (indexes into this batch). Returns the
    /// session checklist (with ids) so the model can reference them later via
    /// `todo_update`.
    pub fn handle_write_todos(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> String {
        let count = self.write_todo_batch(session_id, run_id, args);
        format!(
            "recorded {} todos\n{}\n提醒：开始任何一项前先用 todo_update 标记 in_progress，完成后立即标记 completed 并附 summary。",
            count,
            self.render_todo_checklist(session_id)
        )
    }

    /// `plan_write`: Plan composer mode — create a reviewable plan draft (same
    /// payload as `todo_write`). The backend wraps these todos into a `ready`
    /// plan after the turn; nothing executes until the user confirms.
    pub fn handle_plan_write(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> String {
        let count = self.write_todo_batch(session_id, run_id, args);
        format!(
            "已创建计划草案（{count} 项）。本回合结束后系统会在 Plan 面板生成 status=ready 的计划，\
等待用户点击「执行计划」后才会开始实施。\n{}\n\
禁止在本回合调用 todo_update 或任何写文件/运行命令工具。",
            self.render_todo_checklist(session_id)
        )
    }

    /// `todo_update`: the model marks a todo in_progress / completed / failed
    /// / cancelled as it works through its checklist; mirrored to the UI via
    /// `todo.*` events. Returns the refreshed checklist.
    pub fn handle_update_todo(
        &self,
        session_id: &str,
        run_id: &str,
        args: &serde_json::Value,
    ) -> ApiResult<String> {
        let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        let status_in = args
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let engine_status =
            match status_in {
                "in_progress" | "running" | "started" => "running",
                "completed" | "done" => "completed",
                "failed" => "failed",
                "cancelled" | "canceled" => "cancelled",
                other => return Err(ApiError::new(
                    "TOOL_INVALID_ARGS",
                    format!(
                        "invalid status: {other} (expected in_progress|completed|failed|cancelled)"
                    ),
                )),
            };
        let mut todo = self.todos.get(id)?;
        todo.status = engine_status.to_string();
        if engine_status == "running" {
            todo.related_agent_run_id = Some(run_id.to_string());
        }
        if let Some(summary) = args.get("summary").and_then(|v| v.as_str()) {
            todo.summary = Some(summary.chars().take(500).collect());
        }
        todo.updated_at = now_ts();
        self.todos.save(&todo);

        let etype = match engine_status {
            "running" => "todo.started",
            "completed" => "todo.completed",
            "failed" => "todo.failed",
            _ => "todo.updated",
        };
        self.emit(
            session_id,
            etype,
            "todo",
            json!({
                "id": todo.id,
                "title": todo.title,
                "kind": todo.kind,
                "status": todo.status,
                "summary": todo.summary,
            }),
            Some(run_id.to_string()),
        );
        Ok(format!(
            "todo '{}' -> {}\n{}",
            todo.title,
            todo.status,
            self.render_todo_checklist(session_id)
        ))
    }

    /// Conclusions (title + summary) of completed todos, newest-capped, used
    /// to hand exploration results to subsequent edit tasks.
    pub(crate) fn completed_todo_context(&self, session_id: &str) -> String {
        const MAX_ITEMS: usize = 8;
        let todos = self.todos.list_by_session(session_id);
        let mut lines: Vec<String> = todos
            .iter()
            .filter(|t| t.status == "completed")
            .filter_map(|t| {
                t.summary.as_ref().map(|s| {
                    format!(
                        "- {}（{}）：{}",
                        t.title,
                        t.kind,
                        s.chars().take(500).collect::<String>()
                    )
                })
            })
            .collect();
        if lines.len() > MAX_ITEMS {
            lines = lines.split_off(lines.len() - MAX_ITEMS);
        }
        lines.join("\n")
    }

    /// Plain-text checklist of the session's todos, fed back to the model.
    pub(crate) fn render_todo_checklist(&self, session_id: &str) -> String {
        let todos = self.todos.list_by_session(session_id);
        if todos.is_empty() {
            return "(no todos)".to_string();
        }
        let mut out = String::from("当前任务清单：\n");
        for t in todos {
            let mark = match t.status.as_str() {
                "completed" => "[x]",
                "running" => "[~]",
                "failed" => "[!]",
                "cancelled" | "blocked" => "[-]",
                _ => "[ ]",
            };
            out.push_str(&format!("{mark} {} ({}) id={}\n", t.title, t.kind, t.id));
        }
        out
    }
}

/// Tool spec for Plan composer mode: structured plan draft creation.
pub(crate) fn plan_write_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "plan_write".to_string(),
        description: "创建可审阅的执行计划（Plan 模式专用）。调研完成后调用此工具产出结构化任务列表：\
每项动词开头标题，调研结论写入 description，kind 区分 explore/edit，dependsOn 用本批次下标表达依赖。\
返回带 id 的任务清单；回合结束后系统会生成 ready 计划，等用户确认后才执行。本回合禁止 todo_update 与写操作。"
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string", "description": "任务标题（动词开头）"},
                            "description": {"type": "string", "description": "完成标准与调研结论"},
                            "kind": {"type": "string", "enum": ["explore", "edit"]},
                            "dependsOn": {"type": "array", "items": {"type": "integer"}}
                        },
                        "required": ["title"]
                    }
                }
            },
            "required": ["todos"]
        }),
    }
}

/// Tool spec for the runtime-handled `todo_write` (checklist creation) tool.
pub(crate) fn todo_write_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "todo_write".to_string(),
        description: "为当前任务创建结构化任务清单（todo）。适用于约 3 步以上的非平凡任务；\
一步能完成的简单请求不要建清单。kind=explore 表示只读调研项，kind=edit 表示实施修改项；\
dependsOn 用本批次内的数组下标表达依赖（互相独立留空）。返回带 id 的完整清单，\
后续用 todo_update 按 id 更新状态。"
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                    "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string", "description": "任务的简短标题（动词开头）"},
                            "description": {"type": "string", "description": "完成标准与已知上下文（探索结论写在这里）"},
                            "kind": {"type": "string", "enum": ["explore", "edit"], "description": "explore=只读调研，edit=实施修改"},
                            "dependsOn": {"type": "array", "items": {"type": "integer"}, "description": "所依赖任务在本批次中的下标"}
                        },
                        "required": ["title"]
                    }
                }
            },
            "required": ["todos"]
        }),
    }
}

/// Tool spec for the runtime-handled `todo_update` (checklist progress) tool.
pub(crate) fn todo_update_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "todo_update".to_string(),
        description:
            "更新任务清单中某个 todo 的状态。开始处理前标记 in_progress（同一时刻只保留一项 \
in_progress），完成后立即标记 completed 并附 summary（关键结论/文件路径，供后续任务引用）；\
无法完成时标记 failed 或 cancelled。返回刷新后的清单。"
                .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "todo 的 id（todo_write 返回的清单中给出）"},
                "status": {"type": "string", "enum": ["in_progress", "completed", "failed", "cancelled"]},
                "summary": {"type": "string", "description": "完成结论要点（completed 时建议提供，500 字以内）"}
            },
            "required": ["id", "status"]
        }),
    }
}
