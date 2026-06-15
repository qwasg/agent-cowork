//! Plan engine: LLM-generated (with heuristic fallback) structured plan +
//! derived todos. Port of `plan_engine.py`.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::session::SessionService;
use crate::todo::TodoEngine;
use agent_protocol::events::EventDraft;
use agent_protocol::models::{new_id, now_ts, ChatMessage, Plan, PlanStage, PlanTask, TodoItem};
use agent_protocol::{ApiError, ApiResult};
use agent_providers::types::{noop_sink, ProviderRequest};
use agent_providers::ProviderExecutionService;
use agent_store::store::T_PLANS;
use agent_store::{EventBus, Store};

pub struct PlanEngine {
    pub providers: Arc<ProviderExecutionService>,
    pub store: Arc<Store>,
    pub bus: Arc<EventBus>,
    pub todos: Arc<TodoEngine>,
    pub sessions: Arc<SessionService>,
}

impl PlanEngine {
    pub fn new(
        providers: Arc<ProviderExecutionService>,
        store: Arc<Store>,
        bus: Arc<EventBus>,
        todos: Arc<TodoEngine>,
        sessions: Arc<SessionService>,
    ) -> Self {
        PlanEngine {
            providers,
            store,
            bus,
            todos,
            sessions,
        }
    }

    pub fn get(&self, plan_id: &str) -> ApiResult<Plan> {
        self.store
            .get::<Plan>(T_PLANS, plan_id)
            .ok()
            .flatten()
            .ok_or_else(|| ApiError::plan_not_found(plan_id))
    }

    pub fn save(&self, plan: &Plan) {
        let _ = self.store.put(T_PLANS, &plan.id, plan);
    }

    pub async fn generate(&self, session_id: &str, objective: &str) -> ApiResult<Value> {
        let model = self
            .sessions
            .get(session_id)
            .ok()
            .and_then(|s| s.selected_model_id)
            .unwrap_or_else(|| "default".to_string());

        let drafts = self.draft_tasks(&model, objective).await;

        let plan_id = new_id("plan");
        let stage_id = new_id("stage");

        // Create the todos first so the plan tasks can share their ids and
        // dependency edges — the executable DAG and the displayed plan stay in
        // sync (previously `depends_on` was always empty).
        let explore_indexes: Vec<usize> = drafts
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == "explore")
            .map(|(i, _)| i)
            .collect();
        let mut todo_ids: Vec<String> = Vec::with_capacity(drafts.len());
        let mut tasks: Vec<PlanTask> = Vec::with_capacity(drafts.len());
        for (i, draft) in drafts.iter().enumerate() {
            let mut dep_idx: Vec<usize> = draft
                .depends_on
                .iter()
                .copied()
                .filter(|&d| d < i)
                .collect();
            // Default rule: an edit task with no explicit dependencies depends
            // on every explore task — exploration always lands first.
            if dep_idx.is_empty() && draft.kind != "explore" {
                dep_idx = explore_indexes.iter().copied().filter(|&d| d < i).collect();
            }
            let deps: Vec<String> = dep_idx.iter().map(|&d| todo_ids[d].clone()).collect();
            let todo = self.todos.add(
                session_id,
                &draft.title,
                &draft.description,
                &draft.kind,
                deps.clone(),
            );
            self.bus.emit(
                EventDraft::new(session_id, "todo.created", "todo").payload(
                    json!({ "id": todo.id, "title": todo.title, "kind": todo.kind, "status": todo.status, "dependencies": deps }),
                ),
            );
            tasks.push(PlanTask {
                id: todo.id.clone(),
                stage_id: stage_id.clone(),
                title: draft.title.clone(),
                description: draft.description.clone(),
                priority: "medium".to_string(),
                parallelism: if deps.is_empty() {
                    "parallel".to_string()
                } else {
                    "serial".to_string()
                },
                depends_on: deps,
                status: "pending".to_string(),
                owner_type: "main-agent".to_string(),
            });
            todo_ids.push(todo.id);
        }

        let stage = PlanStage {
            id: stage_id,
            plan_id: plan_id.clone(),
            title: "执行阶段".to_string(),
            order: 0,
            status: "pending".to_string(),
            tasks,
        };
        let plan = Plan {
            id: plan_id.clone(),
            session_id: session_id.to_string(),
            objective: objective.to_string(),
            status: "ready".to_string(),
            current_version_id: new_id("ver"),
            latest_execution_id: None,
            stages: vec![stage],
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        self.save(&plan);

        if let Ok(mut session) = self.sessions.get(session_id) {
            session.active_plan_id = Some(plan_id.clone());
            session.touch();
            self.sessions.save(&session);
        }

        self.bus.emit(
            EventDraft::new(session_id, "plan.generated", "plan").payload(
                json!({ "planId": plan_id, "objective": objective, "taskCount": todo_ids.len() }),
            ),
        );

        Ok(json!({ "plan": plan }))
    }

    /// Assemble a reviewable Plan record from todos the agent already created
    /// itself (agentic plan mode: the model researches read-only, then writes
    /// the checklist via `todo_write`). The plan stays `ready` until the user
    /// confirms execution — nothing runs here.
    pub fn from_todos(&self, session_id: &str, objective: &str, todos: &[TodoItem]) -> Plan {
        let plan_id = new_id("plan");
        let stage_id = new_id("stage");
        let tasks: Vec<PlanTask> = todos
            .iter()
            .map(|t| PlanTask {
                id: t.id.clone(),
                stage_id: stage_id.clone(),
                title: t.title.clone(),
                description: t.description.clone(),
                priority: "medium".to_string(),
                parallelism: if t.dependencies.is_empty() {
                    "parallel".to_string()
                } else {
                    "serial".to_string()
                },
                depends_on: t.dependencies.clone(),
                status: "pending".to_string(),
                owner_type: "main-agent".to_string(),
            })
            .collect();
        let stage = PlanStage {
            id: stage_id,
            plan_id: plan_id.clone(),
            title: "执行阶段".to_string(),
            order: 0,
            status: "pending".to_string(),
            tasks,
        };
        let plan = Plan {
            id: plan_id.clone(),
            session_id: session_id.to_string(),
            objective: objective.to_string(),
            status: "ready".to_string(),
            current_version_id: new_id("ver"),
            latest_execution_id: None,
            stages: vec![stage],
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        self.save(&plan);

        if let Ok(mut session) = self.sessions.get(session_id) {
            session.active_plan_id = Some(plan_id.clone());
            session.touch();
            self.sessions.save(&session);
        }

        self.bus.emit(
            EventDraft::new(session_id, "plan.generated", "plan").payload(
                json!({ "planId": plan_id, "objective": objective, "taskCount": todos.len() }),
            ),
        );
        plan
    }

    async fn draft_tasks(&self, model: &str, objective: &str) -> Vec<DraftTask> {
        let system = ChatMessage::system(crate::prompts::PLAN_DRAFT_SYSTEM_PROMPT);
        let user = ChatMessage::user(format!("目标：{objective}"));
        let req = ProviderRequest {
            model: model.to_string(),
            messages: vec![system, user],
            tools: vec![],
            temperature: Some(0.3),
            stream: false,
            // Descriptions are part of the contract now; 512 was too tight.
            max_tokens: Some(1024),
        };
        let sink = noop_sink();
        if let Ok(resp) = self.providers.execute(&req, &sink, None).await {
            if let Some(tasks) = parse_task_array(&resp.text) {
                if !tasks.is_empty() {
                    return tasks;
                }
            }
        }
        // Heuristic fallback.
        vec![DraftTask {
            title: format!("完成目标：{objective}"),
            description: String::new(),
            kind: "edit".to_string(),
            depends_on: vec![],
        }]
    }
}

#[derive(Debug, Clone)]
pub struct DraftTask {
    pub title: String,
    pub description: String,
    /// `"explore"` or `"edit"` (default).
    pub kind: String,
    pub depends_on: Vec<usize>,
}

/// Accepts both shapes: `["t1", "t2"]` (independent edit tasks) and
/// `[{"title": "t1", "kind": "explore", "dependsOn": [0]}, …]`.
fn parse_task_array(text: &str) -> Option<Vec<DraftTask>> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    let slice = &text[start..=end];
    let value: Value = serde_json::from_str(slice).ok()?;
    let arr = value.as_array()?;
    let mut out: Vec<DraftTask> = Vec::new();
    for (i, v) in arr.iter().enumerate() {
        let task = if let Some(s) = v.as_str() {
            DraftTask {
                title: s.trim().to_string(),
                description: String::new(),
                kind: "edit".to_string(),
                depends_on: vec![],
            }
        } else if let Some(obj) = v.as_object() {
            let title = obj
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let kind = match obj.get("kind").and_then(|k| k.as_str()).unwrap_or("edit") {
                "explore" | "research" | "investigate" => "explore",
                _ => "edit",
            }
            .to_string();
            let depends_on = obj
                .get("dependsOn")
                .and_then(|d| d.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64().map(|n| n as usize))
                        .filter(|&n| n < i)
                        .collect()
                })
                .unwrap_or_default();
            DraftTask {
                title,
                description: obj
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                kind,
                depends_on,
            }
        } else {
            continue;
        };
        if !task.title.is_empty() {
            out.push(task);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_string_tasks() {
        let tasks = parse_task_array(r#"["任务一", "任务二"]"#).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.depends_on.is_empty()));
        assert!(tasks.iter().all(|t| t.kind == "edit"));
    }

    #[test]
    fn parses_tasks_with_dependencies() {
        let tasks = parse_task_array(
            r#"前导文字 [{"title":"a","dependsOn":[]},{"title":"b","dependsOn":[0]},{"title":"c","dependsOn":[5]}] 收尾"#,
        )
        .unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[1].depends_on, vec![0]);
        // forward / out-of-range references are dropped
        assert!(tasks[2].depends_on.is_empty());
    }

    #[test]
    fn parses_task_kinds() {
        let tasks = parse_task_array(
            r#"[{"title":"调研代码","kind":"explore"},{"title":"实现","kind":"edit"},{"title":"未知","kind":"weird"}]"#,
        )
        .unwrap();
        assert_eq!(tasks[0].kind, "explore");
        assert_eq!(tasks[1].kind, "edit");
        assert_eq!(tasks[2].kind, "edit", "unknown kinds default to edit");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_task_array("no json here").is_none());
        assert!(parse_task_array("[]").is_none());
    }
}
