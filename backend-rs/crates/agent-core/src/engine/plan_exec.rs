//! Plan DAG executor: drives ready todo batches (explore wide / edit
//! bounded) to completion with true parallelism.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::json;

use agent_protocol::models::{now_ts, ChatMessage};
use agent_protocol::ApiResult;

use super::Runtime;

/// Result of executing a plan DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOutcome {
    Completed,
    Cancelled,
    PartialFailure,
}

impl Runtime {
    pub async fn run_plan(
        self: &Arc<Self>,
        run_id: &str,
        session_id: &str,
        model: &str,
    ) -> ApiResult<PlanOutcome> {
        let mut run = self.get_run(run_id)?;
        let control = self.register_run(run_id);
        let mut any_failed = false;

        let outcome = loop {
            if control.cancel.is_cancelled() {
                break PlanOutcome::Cancelled;
            }
            // Mid-run plan revision works by construction: every iteration
            // re-queries `ready_todos`, so todos added via REST while a batch
            // is running join the DAG on the next pass without interrupting
            // in-flight branches.
            let ready = self.todos.ready_todos(session_id);
            if ready.is_empty() {
                break if any_failed {
                    PlanOutcome::PartialFailure
                } else {
                    PlanOutcome::Completed
                };
            }
            // `ready_todos` sorts explore first and holds edits back while
            // exploration is unfinished, so a batch is homogeneous in kind.
            // Explore todos fan out wide (read-only, safe to parallelize);
            // edit todos run with bounded parallelism (default 1: serial) so
            // concurrent agents don't fight over the same files.
            let batch_kind = ready[0].kind.clone();
            let batch_limit = if batch_kind == "explore" {
                self.parallel_limit
            } else {
                self.edit_parallel
            };
            let batch: Vec<_> = ready
                .into_iter()
                .filter(|t| t.kind == batch_kind)
                .take(batch_limit)
                .collect();
            let mut batch_ids: HashSet<String> = batch.iter().map(|t| t.id.clone()).collect();

            run.active_todo_ids = batch.iter().map(|t| t.id.clone()).collect();
            run.updated_at = now_ts();
            self.save_run(&run).await;

            // Conclusions from already-finished todos (exploration results,
            // earlier edits) are handed to the next tasks as context.
            let prior_context = self.completed_todo_context(session_id);

            // True parallel execution of the ready batch via JoinSet.
            let mut set = tokio::task::JoinSet::new();
            for todo in batch {
                let this = self.clone();
                let sid = session_id.to_string();
                let mdl = model.to_string();
                let rid = run_id.to_string();
                let ctrl = control.clone();
                let prior = prior_context.clone();
                if let Ok(mut t) = self.todos.get(&todo.id) {
                    t.status = "running".to_string();
                    t.related_agent_run_id = Some(run_id.to_string());
                    t.updated_at = now_ts();
                    self.todos.save(&t);
                }
                this.emit(
                    &sid,
                    "todo.started",
                    "todo",
                    json!({ "id": todo.id, "title": todo.title, "kind": todo.kind, "status": "running" }),
                    Some(rid.clone()),
                );
                set.spawn(async move {
                    let is_explore = todo.kind == "explore";
                    // Explore todos only see read-only tools.
                    let mut allowed = this.allowed_tools(&sid, "build");
                    if is_explore {
                        allowed.retain(|n| !crate::permission::WRITE_TOOLS.contains(&n.as_str()));
                    }
                    let mut prompt = format!("任务：{}\n说明：{}", todo.title, todo.description);
                    if !prior.is_empty() {
                        prompt.push_str("\n\n已完成的前序任务结论：\n");
                        prompt.push_str(&prior);
                    }
                    if is_explore {
                        prompt.push_str(crate::prompts::EXPLORE_TASK_SUFFIX);
                    }
                    let messages = vec![
                        ChatMessage::system(this.build_main_system_prompt(
                            &sid,
                            "build",
                            &allowed,
                            None,
                            Some(&prompt),
                        )),
                        ChatMessage::user(prompt),
                    ];
                    // Per-todo timeout + bounded retries (fresh context each
                    // attempt; cancellation is never retried).
                    let mut attempt = 0usize;
                    let res = loop {
                        attempt += 1;
                        let run = this.run_react(
                            &sid,
                            &rid,
                            &mdl,
                            messages.clone(),
                            &allowed,
                            ctrl.clone(),
                            0,
                            None,
                        );
                        let attempt_res = if this.todo_timeout_secs > 0 {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(this.todo_timeout_secs),
                                run,
                            )
                            .await
                            {
                                Ok(inner) => inner.map(|o| o.text),
                                Err(_) => Err(agent_protocol::ApiError::new(
                                    "TODO_TIMEOUT",
                                    format!("todo 执行超时（{}s）", this.todo_timeout_secs),
                                )),
                            }
                        } else {
                            run.await.map(|o| o.text)
                        };
                        match attempt_res {
                            Ok(text) => break Ok(text),
                            Err(e) if e.code == "RUN_CANCELLED" => break Err(e),
                            Err(e) if attempt <= this.todo_retry_limit => {
                                this.emit(
                                    &sid,
                                    "todo.retrying",
                                    "todo",
                                    json!({
                                        "id": todo.id,
                                        "title": todo.title,
                                        "attempt": attempt,
                                        "error": e.message,
                                    }),
                                    Some(rid.clone()),
                                );
                            }
                            Err(e) => break Err(e),
                        }
                    };
                    (todo.id, todo.title, res)
                });
            }

            while let Some(joined) = set.join_next().await {
                match joined {
                    Ok((todo_id, title, res)) => {
                        batch_ids.remove(&todo_id);
                        match res {
                            Ok(summary) => {
                                if let Ok(mut t) = self.todos.get(&todo_id) {
                                    t.status = "completed".to_string();
                                    t.summary = Some(summary.chars().take(500).collect());
                                    t.updated_at = now_ts();
                                    self.todos.save(&t);
                                }
                                run.completed_todo_ids.push(todo_id.clone());
                                self.emit(
                                    session_id,
                                    "todo.completed",
                                    "todo",
                                    json!({ "id": todo_id, "title": title, "status": "completed" }),
                                    Some(run_id.to_string()),
                                );
                            }
                            Err(e) => {
                                any_failed = true;
                                if let Ok(mut t) = self.todos.get(&todo_id) {
                                    t.status = "failed".to_string();
                                    t.last_error = Some(e.message.clone());
                                    t.updated_at = now_ts();
                                    self.todos.save(&t);
                                }
                                run.failed_todo_ids.push(todo_id.clone());
                                self.emit(
                                    session_id,
                                    "todo.failed",
                                    "todo",
                                    json!({ "id": todo_id, "title": title, "status": "failed", "error": e.message }),
                                    Some(run_id.to_string()),
                                );
                            }
                        }
                    }
                    Err(join_err) => {
                        // A task panicked or was aborted; the affected todo is
                        // resolved below from the unaccounted batch ids.
                        tracing::warn!("plan task join error: {join_err}");
                    }
                }
            }

            // Any todo not joined back (task panic) must not stay `running`.
            for todo_id in batch_ids {
                any_failed = true;
                if let Ok(mut t) = self.todos.get(&todo_id) {
                    t.status = "failed".to_string();
                    t.last_error = Some("task panicked".to_string());
                    t.updated_at = now_ts();
                    self.todos.save(&t);
                }
                run.failed_todo_ids.push(todo_id.clone());
                self.emit(
                    session_id,
                    "todo.failed",
                    "todo",
                    json!({ "id": todo_id, "status": "failed", "error": "task panicked" }),
                    Some(run_id.to_string()),
                );
            }

            run.active_todo_ids.clear();
            run.updated_at = now_ts();
            self.save_run(&run).await;
        };

        // Todos whose dependencies ended in failure can never become ready;
        // mark them blocked instead of leaving them queued forever.
        for todo in self.todos.blocked_todos(session_id) {
            if let Ok(mut t) = self.todos.get(&todo.id) {
                t.status = "blocked".to_string();
                t.last_error = Some("dependency failed".to_string());
                t.updated_at = now_ts();
                self.todos.save(&t);
            }
            self.emit(
                session_id,
                "todo.failed",
                "todo",
                json!({ "id": todo.id, "title": todo.title, "error": "dependency failed", "status": "blocked" }),
                Some(run_id.to_string()),
            );
        }

        run.active_todo_ids.clear();
        run.updated_at = now_ts();
        self.save_run(&run).await;
        self.unregister_run(run_id);
        Ok(outcome)
    }
}
