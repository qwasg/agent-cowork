//! Parity / regression tests porting the key pytest scenarios (health, session
//! CRUD, ask flow, plan flow, error envelope) to the Rust stack. They exercise
//! the axum router directly via `tower::ServiceExt::oneshot`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::api::{routes, AppServices};
use crate::config::Config;
use crate::contracts::models::new_id;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn test_services() -> std::sync::Arc<AppServices> {
    // Serialize the process-global env mutation + Config::load section so
    // parallel tests don't clobber each other's data-dir env vars.
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = std::env::temp_dir().join(new_id("agentd_test"));
    std::env::set_var("AGENT_DEBUG_DATA_DIR", &tmp);
    std::env::set_var("AGENT_DEBUG_SESSION_DIR", tmp.join("sessions"));
    std::env::set_var("AGENT_DEBUG_WORKSPACE_ROOT", &tmp);
    std::env::set_var("AGENT_DEBUG_PERSIST_EVENTS", "1");
    let _ = std::fs::create_dir_all(&tmp);
    let cfg = Config::load();
    AppServices::build(cfg).expect("build services")
}

fn test_app() -> axum::Router {
    routes::router(test_services())
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, value)
}

async fn post(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
    (status, value)
}

#[tokio::test]
async fn health_ok() {
    let router = test_app();
    let (status, body) = get(&router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
}

#[tokio::test]
async fn session_crud_and_error_envelope() {
    let router = test_app();

    let (status, body) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "t1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sid = body["session"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["session"]["title"], json!("t1"));

    let (status, body) = get(&router, &format!("/api/agent-debug/sessions/{sid}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session"]["id"], json!(sid));

    // Unknown session -> 404 with error envelope (parity with _maybe_raise).
    let (status, body) = get(&router, "/api/agent-debug/sessions/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], json!("SESSION_NOT_FOUND"));
}

#[tokio::test]
async fn ask_execute_returns_message() {
    let router = test_app();
    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "ask" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/ask:execute"),
        json!({ "userInput": "你好", "composerMode": "ask" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = body["message"]["text"].as_str().unwrap_or("");
    assert!(!text.is_empty(), "expected non-empty assistant text");
    assert_eq!(body["run"]["status"], json!("completed"));
}

#[tokio::test]
async fn plan_generate_and_execute() {
    let router = test_app();
    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "plan" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/plan:generate"),
        json!({ "userInput": "做一个功能" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let plan_id = body["plan"]["id"].as_str().unwrap().to_string();
    assert!(plan_id.starts_with("plan_"));

    // todos derived from the plan.
    let (_, todos) = get(&router, &format!("/api/agent-debug/sessions/{sid}/todos")).await;
    assert!(todos["todos"].as_array().unwrap().len() >= 1);

    // Execute with an empty body (parity: tolerant body handling).
    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/plans/{plan_id}:execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["run"]["status"], json!("completed"));
}

#[tokio::test]
async fn todo_lifecycle_via_runtime_tools() {
    let app = test_services();
    let session = app.sessions.create("todo lifecycle", None, false);
    let sid = session.id.clone();

    // todo_write: explore + edit with an in-batch dependency.
    let out = app.runtime.handle_write_todos(
        &sid,
        "run_test",
        &json!({ "todos": [
            { "title": "调研现状", "kind": "explore" },
            { "title": "实施修改", "kind": "edit", "dependsOn": [0] },
        ]}),
    );
    assert!(out.contains("recorded 2 todos"));
    assert!(out.contains("id=todo_"), "checklist must expose todo ids");

    let todos = app.todos.list_by_session(&sid);
    assert_eq!(todos.len(), 2);
    let explore = todos.iter().find(|t| t.kind == "explore").unwrap().clone();
    let edit = todos.iter().find(|t| t.kind == "edit").unwrap().clone();
    assert_eq!(edit.dependencies, vec![explore.id.clone()]);

    // The edit todo is gated until exploration finishes.
    let ready = app.todos.ready_todos(&sid);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, explore.id);

    // todo_update drives the in_progress -> completed lifecycle.
    let out = app
        .runtime
        .handle_update_todo(
            &sid,
            "run_test",
            &json!({ "id": explore.id, "status": "in_progress" }),
        )
        .unwrap();
    assert!(out.contains("[~]"), "in-progress mark must appear: {out}");
    assert_eq!(app.todos.get(&explore.id).unwrap().status, "running");

    app.runtime
        .handle_update_todo(
            &sid,
            "run_test",
            &json!({ "id": explore.id, "status": "completed", "summary": "结论：无阻塞" }),
        )
        .unwrap();
    let done = app.todos.get(&explore.id).unwrap();
    assert_eq!(done.status, "completed");
    assert_eq!(done.summary.as_deref(), Some("结论：无阻塞"));

    // Edit todo is released now.
    let ready = app.todos.ready_todos(&sid);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, edit.id);

    // Invalid status is rejected.
    let err = app
        .runtime
        .handle_update_todo(&sid, "run_test", &json!({ "id": edit.id, "status": "nope" }))
        .unwrap_err();
    assert_eq!(err.code, "TOOL_INVALID_ARGS");
}

#[tokio::test]
async fn plan_todos_carry_kind_and_settle_terminal() {
    let router = test_app();
    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "phase" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/plan:generate"),
        json!({ "userInput": "重构模块" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let plan_id = body["plan"]["id"].as_str().unwrap().to_string();

    // Todos expose the kind field on the wire.
    let (_, todos) = get(&router, &format!("/api/agent-debug/sessions/{sid}/todos")).await;
    let arr = todos["todos"].as_array().unwrap();
    assert!(!arr.is_empty());
    assert!(arr.iter().all(|t| t["kind"].is_string()));

    // After execution every todo has settled into a terminal state.
    let (status, _) = post(
        &router,
        &format!("/api/agent-debug/plans/{plan_id}:execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, todos) = get(&router, &format!("/api/agent-debug/sessions/{sid}/todos")).await;
    for t in todos["todos"].as_array().unwrap() {
        let st = t["status"].as_str().unwrap();
        assert!(
            matches!(st, "completed" | "failed" | "cancelled" | "blocked"),
            "todo left in non-terminal state: {st}"
        );
    }
}

#[tokio::test]
async fn permission_resolution_endpoint() {
    let router = test_app();
    // Unknown / already-resolved request id -> 404 with error envelope.
    let (status, body) = post(
        &router,
        "/api/agent-debug/permissions/perm_nonexistent:approve",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], json!("PERMISSION_REQUEST_NOT_FOUND"));
    // Unknown action -> 404.
    let (status, _) = post(
        &router,
        "/api/agent-debug/permissions/perm_x:frobnicate",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn local_file_is_confined_to_workspace_by_default() {
    let router = test_app();
    // Path traversal outside the workspace root must be rejected.
    let (status, body) = get(
        &router,
        "/api/agent-debug/local-file?path=..%2F..%2F..%2Fetc%2Fpasswd",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], json!("PATH_OUTSIDE_ROOT"));
}

#[tokio::test]
async fn ws_replay_contract_via_replay_since() {
    let router = test_app();
    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "replay" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();
    post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/ask:execute"),
        json!({ "userInput": "hi", "composerMode": "ask" }),
    )
    .await;

    let (status, body) = get(
        &router,
        &format!("/api/agent-debug/replay/{sid}/since?fromSeq=0"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["gap"], json!(false));
    let events = body["events"].as_array().unwrap();
    assert!(!events.is_empty());
    // Wire shape carries the derived `channel` and camelCase keys.
    assert!(events[0]["channel"].is_string());
    assert!(events[0]["sessionId"].is_string());
}
