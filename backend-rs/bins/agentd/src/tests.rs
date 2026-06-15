//! Parity / regression tests porting the key pytest scenarios (health, session
//! CRUD, ask flow, plan flow, error envelope) to the Rust stack. They exercise
//! the axum router directly via `tower::ServiceExt::oneshot`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::api::{routes, AppServices};
use agent_config::Config;
use agent_protocol::models::new_id;

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

async fn put(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(
            Request::put(uri)
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

async fn delete(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(Request::delete(uri).body(Body::empty()).unwrap())
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
    let session = app.sessions.create("todo lifecycle", "coding", None, false);
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
        .handle_update_todo(
            &sid,
            "run_test",
            &json!({ "id": edit.id, "status": "nope" }),
        )
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
async fn search_config_roundtrip_normalization_and_secrecy() {
    let app = test_services();

    // Default view: disabled tavily config in the Python `{"config": ...}` shape.
    let v = app.get_search_config();
    assert_eq!(v["config"]["enabled"], json!(false));
    assert_eq!(v["config"]["provider"], json!("tavily"));

    // Save with mixed valid / invalid values: invalid ones are normalized.
    let v = app.set_search_config(&json!({
        "enabled": true,
        "apiKey": "tvly-secret",
        "topic": "weird-topic",
        "searchDepth": "advanced",
        "timeRange": "decade",
        "extractDepth": "advanced",
    }));
    assert_eq!(v["config"]["enabled"], json!(true));
    assert_eq!(v["config"]["apiKeySet"], json!(true));
    assert_eq!(v["config"]["topic"], json!("general"));
    assert_eq!(v["config"]["searchDepth"], json!("advanced"));
    assert_eq!(v["config"]["timeRange"], json!(""));
    assert_eq!(v["config"]["extractDepth"], json!("advanced"));
    assert!(!v["config"]["updatedAt"].as_str().unwrap().is_empty());

    // Secret is encrypted at rest and never appears in the public view.
    let stored = app.search_config.get_stored();
    assert!(
        stored.api_key.starts_with("enc:"),
        "api_key must be encrypted"
    );
    assert!(v["config"].get("apiKey").is_none());

    // Empty / missing apiKey keeps the previously stored secret.
    let _ = app.set_search_config(&json!({ "apiKey": "", "timeRange": "week" }));
    assert_eq!(app.search_config.get_stored().api_key, stored.api_key);
    assert_eq!(app.search_config.get_stored().time_range, "week");

    // Resolution decrypts the stored key.
    assert_eq!(
        app.search_config.resolve_api_key().as_deref(),
        Some("tvly-secret")
    );
    assert!(app.search_config.effectively_enabled());

    // Disabling gates the web_search tool with a clear message (and never
    // reaches the network).
    let _ = app.set_search_config(&json!({ "enabled": false }));
    assert!(!app.search_config.effectively_enabled());
    let ctx = agent_tools::ToolContext {
        session_id: "s".into(),
        run_id: "r".into(),
        workspace_root: app.cfg.workspace_root.clone(),
        web: agent_tools::WebConfig {
            fetch_max_chars: 1000,
            allow_private: false,
        },
        search: app.search_config.clone(),
        skill_dirs: vec![],
        tool_output_dir: app.cfg.data_dir.join("tool-outputs"),
        shell: agent_tools::shell::ShellManager::new(app.cfg.data_dir.join("shell-outputs")),
    };
    let out = app
        .tools
        .run("web_search", json!({ "query": "hello" }), &ctx)
        .await
        .unwrap();
    assert!(
        out.content.contains("web_search unavailable"),
        "got: {}",
        out.content
    );
}

#[tokio::test]
async fn search_config_rest_roundtrip_and_legacy_migration() {
    let app = test_services();
    let router = routes::router(app.clone());

    let (status, body) = put(
        &router,
        "/api/agent-debug/search-config",
        json!({ "enabled": true, "apiKey": "k1", "topic": "news" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["config"]["topic"], json!("news"));
    let (status, body) = get(&router, "/api/agent-debug/search-config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["config"]["enabled"], json!(true));
    assert_eq!(body["config"]["apiKeySet"], json!(true));

    // Legacy stub entry (`search_api_key`) is folded into the full config by
    // a fresh service instance and then removed.
    let enc = app.crypto.encrypt("legacy-key");
    let _ = app.store.kv_delete("search_config");
    let _ = app.store.kv_put("search_api_key", &enc);
    let svc = agent_core::SearchConfigService::new(app.store.clone(), app.crypto.clone(), &app.cfg);
    assert!(
        svc.get_stored().enabled,
        "migrated config should be enabled"
    );
    assert_eq!(svc.resolve_api_key().as_deref(), Some("legacy-key"));
    assert!(
        app.store.kv_get("search_api_key").is_none(),
        "legacy key removed"
    );
}

#[tokio::test]
async fn auth_register_login_and_password_change() {
    let app = test_services();

    // Input validation.
    assert_eq!(
        app.auth
            .register("not-an-email", "123456", "", "")
            .unwrap_err()
            .code,
        "AUTH_INVALID_INPUT"
    );
    assert_eq!(
        app.auth.register("a@b.co", "123", "", "").unwrap_err().code,
        "AUTH_INVALID_INPUT"
    );

    // Happy path.
    let out = app.auth.register("a@b.co", "123456", "甲", "ws").unwrap();
    let token = out["token"].as_str().unwrap().to_string();
    let uid = out["user"]["id"].as_str().unwrap().to_string();
    assert!(!out["user"]["updatedAt"].as_str().unwrap().is_empty());
    assert_eq!(
        app.auth
            .register("a@b.co", "123456", "", "")
            .unwrap_err()
            .code,
        "AUTH_EMAIL_TAKEN"
    );

    // Token verification.
    assert!(app.auth.user_from_token(&token).is_some());
    assert!(app.auth.user_from_token("not.a.token").is_none());

    // Bad credentials (wrong password and unknown email are indistinguishable).
    assert_eq!(
        app.auth.login("a@b.co", "wrong-pass").unwrap_err().code,
        "AUTH_BAD_CREDENTIALS"
    );
    assert_eq!(
        app.auth.login("ghost@b.co", "123456").unwrap_err().code,
        "AUTH_BAD_CREDENTIALS"
    );

    // Password change via profile patch.
    app.auth
        .update_profile(&uid, &json!({ "password": "newpass1" }))
        .unwrap();
    assert!(app.auth.login("a@b.co", "123456").is_err());
    assert!(app.auth.login("a@b.co", "newpass1").is_ok());
    assert_eq!(
        app.auth
            .update_profile(&uid, &json!({ "password": "123" }))
            .unwrap_err()
            .code,
        "AUTH_INVALID_INPUT"
    );
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

#[tokio::test]
async fn design_snapshot_returns_full_aggregate() {
    let router = test_app();
    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "snap" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    let (status, body) = get(
        &router,
        &format!("/api/agent-debug/design-snapshot?sessionId={sid}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // DesignSnapshot contract: all aggregate fields present.
    for key in [
        "sessions",
        "activeSession",
        "planBundle",
        "todos",
        "events",
        "run",
        "swarm",
        "diffs",
        "proposals",
        "metrics",
        "contextWindow",
        "models",
        "latestSeq",
    ] {
        assert!(body.get(key).is_some(), "missing snapshot key: {key}");
    }
    assert_eq!(body["activeSession"]["id"], json!(sid));
    assert!(body["swarm"]["nodes"].is_array());
    assert!(body["metrics"]["todos"]["total"].is_number());
    assert!(body["models"]["models"].is_array());
}

#[tokio::test]
async fn proposal_apply_and_discard_lifecycle() {
    let services = test_services();
    let workspace = services.cfg.workspace_root.clone();
    let router = routes::router(services.clone());

    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "prop" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    let record = services.proposals.record(&json!({
        "sessionId": sid,
        "summary": "fix demo file",
        "changes": [{
            "changeId": "chg-1",
            "path": "src/demo.ts",
            "originalContent": "let a: number = \"1\";",
            "proposedContent": "let a: number = 1;",
        }],
    }));

    // List shape: agent-ide reads `proposals`, Python parity keeps `items`.
    let (status, body) = get(
        &router,
        &format!("/api/agent-debug/proposals?sessionId={sid}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["proposals"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], json!(record.id));

    // Apply writes the proposed content into the workspace.
    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/proposals/{}:apply", record.id),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["proposal"]["status"], json!("applied"));
    let written = std::fs::read_to_string(workspace.join("src/demo.ts")).unwrap();
    assert_eq!(written, "let a: number = 1;");

    // Applied proposals can't be re-applied.
    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/proposals/{}:apply", record.id),
        json!({}),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!("PROPOSAL_INVALID_STATE"));

    // The apply event landed in the session stream.
    let events = services.bus.snapshot(&sid);
    assert!(events
        .iter()
        .any(|e| e.event_type == "agent.code_edit.applied"));

    // Discard a second pending proposal.
    let p2 = services.proposals.record(&json!({
        "sessionId": sid,
        "summary": "another",
        "changes": [],
    }));
    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/proposals/{}:discard", p2.id),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["proposal"]["status"], json!("discarded"));
}

#[tokio::test]
async fn session_fork_copies_todos_history_and_events() {
    let services = test_services();
    let router = routes::router(services.clone());

    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "origin" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    post(
        &router,
        "/api/agent-debug/todos",
        json!({ "sessionId": sid, "title": "todo-a" }),
    )
    .await;
    services.bus.emit(
        agent_protocol::events::EventDraft::new(&sid, "agent.message", "agent")
            .payload(json!({ "text": "hello" })),
    );

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}:fork"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let fork_id = body["session"]["id"].as_str().unwrap().to_string();
    assert_ne!(fork_id, sid);

    // Todos cloned with fresh ids.
    let forked_todos = services.todos.list_by_session(&fork_id);
    assert_eq!(forked_todos.len(), 1);
    assert_eq!(forked_todos[0].title, "todo-a");
    let original_todos = services.todos.list_by_session(&sid);
    assert_ne!(forked_todos[0].id, original_todos[0].id);

    // Events copied into the fork's stream.
    let forked_events = services.bus.snapshot(&fork_id);
    assert!(forked_events
        .iter()
        .any(|e| e.event_type == "agent.message"));
}

#[tokio::test]
async fn session_revert_truncates_event_stream() {
    let services = test_services();
    let router = routes::router(services.clone());

    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "revert" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    let e1 = services.bus.emit(
        agent_protocol::events::EventDraft::new(&sid, "agent.message", "agent")
            .payload(json!({ "n": 1 })),
    );
    services.bus.emit(
        agent_protocol::events::EventDraft::new(&sid, "agent.message", "agent")
            .payload(json!({ "n": 2 })),
    );
    services.bus.emit(
        agent_protocol::events::EventDraft::new(&sid, "agent.message", "agent")
            .payload(json!({ "n": 3 })),
    );
    assert_eq!(services.bus.snapshot(&sid).len(), 3);

    // Inclusive revert at the first event keeps only that event.
    let (status, _) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}:revert"),
        json!({ "messageId": e1.id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let remaining = services.bus.snapshot(&sid);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].payload["n"], json!(1));
}

#[tokio::test]
async fn checkpoint_rewind_restores_files_and_truncates_events() {
    let services = test_services();
    let workspace = services.cfg.workspace_root.clone();
    let router = routes::router(services.clone());

    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "ckpt" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    std::fs::write(workspace.join("notes.txt"), "v1").unwrap();
    services.bus.emit(
        agent_protocol::events::EventDraft::new(&sid, "agent.message", "agent")
            .payload(json!({ "n": 1 })),
    );

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/checkpoints"),
        json!({ "label": "before", "paths": ["notes.txt"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ckpt_id = body["checkpoint"]["id"].as_str().unwrap().to_string();
    let seq_at_ckpt = body["checkpoint"]["eventSeq"].as_i64().unwrap();

    // Mutate the file and the event stream past the checkpoint.
    std::fs::write(workspace.join("notes.txt"), "v2-dirty").unwrap();
    services.bus.emit(
        agent_protocol::events::EventDraft::new(&sid, "agent.message", "agent")
            .payload(json!({ "n": 2 })),
    );
    assert!(services.bus.latest_seq(&sid) > seq_at_ckpt);

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/checkpoints/{ckpt_id}:rewind"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.txt")).unwrap(),
        "v1"
    );
    // Stream truncated to the checkpoint's seq plus the `checkpoint.rewound`
    // marker event.
    let events = services.bus.snapshot(&sid);
    assert!(events
        .iter()
        .all(|e| e.seq <= seq_at_ckpt || e.event_type == "checkpoint.rewound"));
    assert!(events.iter().any(|e| e.event_type == "checkpoint.rewound"));
}

#[tokio::test]
async fn mcp_manager_connects_demo_server_and_calls_tools() {
    if agent_mcp::demo_command().is_none() {
        eprintln!("skip: mcp-demo-server binary not found");
        return;
    }
    let dir = std::env::temp_dir().join(format!("agentd-mcp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = agent_mcp::McpManager::new(dir.join("mcp.json"));

    // Bootstrap writes a default config exposing the demo server.
    mgr.ensure_default_config();
    let statuses = mgr.reload().await;
    assert_eq!(statuses.len(), 1);
    assert!(
        statuses[0].ok,
        "demo connect failed: {:?}",
        statuses[0].error
    );
    assert_eq!(statuses[0].tool_count, 3);

    // Tools surface with the `mcp__{server}__{tool}` naming convention.
    let specs = mgr.tool_specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"mcp__demo__echo"), "got: {names:?}");
    assert!(names.contains(&"mcp__demo__add"));

    // Calls reuse the long-lived connection.
    let out = mgr
        .call_tool("demo", "echo", json!({"text": "hello-mcp"}))
        .await
        .unwrap();
    assert!(out.contains("hello-mcp"), "got: {out}");
    let out = mgr
        .call_tool("demo", "add", json!({"a": 2, "b": 40}))
        .await
        .unwrap();
    assert!(out.contains("42"), "got: {out}");

    // Tool-level errors propagate as Err.
    assert!(mgr.call_tool("demo", "add", json!({"a": 1})).await.is_err());
    // Unknown server is a clean error.
    assert!(mgr.call_tool("nope", "echo", json!({})).await.is_err());
}

#[tokio::test]
async fn permission_rules_evaluate_in_order() {
    use agent_core::permission::{PermissionRule, RuleDecision};
    let services = test_services();
    let rule = |action: &str, pattern: &str| PermissionRule {
        action: action.to_string(),
        pattern: pattern.to_string(),
    };
    services
        .permissions
        .set_rules(vec![
            rule("deny", "run_command(rm *)"),
            rule("allow", "run_command(git *)"),
            rule("ask", "delete_file(*)"),
            rule("allow", "mcp__*"),
        ])
        .unwrap();

    let eval = |tool: &str, args: serde_json::Value| services.permissions.evaluate(tool, &args);
    assert_eq!(
        eval("run_command", json!({"command": "rm -rf /tmp/x"})),
        RuleDecision::Deny
    );
    assert_eq!(
        eval("run_command", json!({"command": "git status"})),
        RuleDecision::Allow
    );
    assert_eq!(
        eval("run_command", json!({"command": "cargo build"})),
        RuleDecision::Default
    );
    assert_eq!(
        eval("delete_file", json!({"path": "src/main.rs"})),
        RuleDecision::Ask
    );
    assert_eq!(eval("mcp__demo__echo", json!({})), RuleDecision::Allow);

    // Invalid rules are rejected.
    assert!(services
        .permissions
        .set_rules(vec![rule("nope", "run_command")])
        .is_err());
    assert!(services
        .permissions
        .set_rules(vec![rule("allow", "broken(unclosed")])
        .is_err());
}

#[tokio::test]
async fn hooks_block_and_rewrite_tool_calls() {
    use agent_core::hooks::{HookRunner, PreToolVerdict};
    let dir = std::env::temp_dir().join(format!("agentd-hooks-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("hooks.json");

    // 1. A PreToolUse hook that blocks run_command (exit 2 = block).
    let block_cmd = if cfg!(windows) {
        "[Console]::Error.Write('blocked-by-test'); exit 2"
    } else {
        "echo blocked-by-test >&2; exit 2"
    };
    std::fs::write(
        &config,
        json!({ "hooks": [{ "event": "PreToolUse", "matcher": "run_command", "command": block_cmd }] })
            .to_string(),
    )
    .unwrap();
    let runner = HookRunner::new(config.clone());
    match runner
        .pre_tool_use("s1", "run_command", &json!({"command": "rm -rf /"}))
        .await
    {
        PreToolVerdict::Block(reason) => assert!(reason.contains("blocked-by-test")),
        _ => panic!("expected block"),
    }
    // Matcher means other tools pass through untouched.
    match runner
        .pre_tool_use("s1", "read_file", &json!({"path": "a.txt"}))
        .await
    {
        PreToolVerdict::Continue(None) => {}
        _ => panic!("expected pass-through"),
    }

    // 2. A rewriting hook: stdout JSON `{"args": ...}` replaces the args.
    // (Small sleep so the config file's mtime visibly changes for the cache.)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let rewrite_cmd = if cfg!(windows) {
        "Write-Output '{\"args\":{\"command\":\"echo safe\"}}'"
    } else {
        "echo '{\"args\":{\"command\":\"echo safe\"}}'"
    };
    std::fs::write(
        &config,
        json!({ "hooks": [{ "event": "PreToolUse", "matcher": "run_*", "command": rewrite_cmd }] })
            .to_string(),
    )
    .unwrap();
    match runner
        .pre_tool_use("s1", "run_command", &json!({"command": "rm -rf /"}))
        .await
    {
        PreToolVerdict::Continue(Some(args)) => {
            assert_eq!(args["command"], "echo safe");
        }
        _ => panic!("expected rewritten args"),
    }
}

#[tokio::test]
async fn edit_tools_apply_and_record_proposals() {
    let services = test_services();
    let workspace = services.cfg.workspace_root.clone();
    let session = services.sessions.create("edit", "coding", None, false);

    // str_replace_edit: exact replacement with uniqueness validation.
    std::fs::write(workspace.join("hello.rs"), "fn main() {\n    old();\n}\n").unwrap();
    let out = services
        .runtime
        .handle_edit_tool(
            &session.id,
            "run_e1",
            "str_replace_edit",
            &json!({ "path": "hello.rs", "old_string": "    old();", "new_string": "    new();" }),
        )
        .await
        .unwrap();
    assert!(out.contains("hello.rs"), "got: {out}");
    assert_eq!(
        std::fs::read_to_string(workspace.join("hello.rs")).unwrap(),
        "fn main() {\n    new();\n}\n"
    );

    // Ambiguous old_string is rejected.
    std::fs::write(workspace.join("dup.txt"), "x\nx\n").unwrap();
    let err = services
        .runtime
        .handle_edit_tool(
            &session.id,
            "run_e1",
            "str_replace_edit",
            &json!({ "path": "dup.txt", "old_string": "x", "new_string": "y" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "EDIT_CONFLICT");

    // apply_patch: add + update + delete in one call.
    std::fs::write(workspace.join("to_delete.txt"), "bye\n").unwrap();
    let patch = "*** Begin Patch\n\
*** Add File: added.txt\n\
+hello added\n\
*** Update File: hello.rs\n\
@@\n fn main() {\n-    new();\n+    newest();\n }\n\
*** Delete File: to_delete.txt\n\
*** End Patch";
    services
        .runtime
        .handle_edit_tool(
            &session.id,
            "run_e1",
            "apply_patch",
            &json!({ "patch": patch }),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("added.txt")).unwrap(),
        "hello added\n"
    );
    assert!(std::fs::read_to_string(workspace.join("hello.rs"))
        .unwrap()
        .contains("newest();"));
    assert!(!workspace.join("to_delete.txt").exists());

    // Both edits were recorded as applied proposals + emitted events.
    let proposals = services.proposals.list_for_session(&session.id);
    assert_eq!(proposals.len(), 2);
    assert!(proposals.iter().all(|p| p.status == "applied"));
    assert!(services
        .bus
        .snapshot(&session.id)
        .iter()
        .any(|e| e.event_type == "agent.code_edit.proposed"));
}

#[tokio::test]
async fn steer_queues_message_and_emits_event() {
    use agent_protocol::models::{now_ts, AgentRun};

    let services = test_services();
    let router = routes::router(services.clone());

    // Steering an inactive run is rejected.
    let (status, body) = post(
        &router,
        "/api/agent-debug/runs/run_missing:steer",
        json!({ "text": "focus on tests" }),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], json!("RUN_NOT_ACTIVE"));

    // Simulate an active run: persisted + registered control.
    let session = services.sessions.create("steer", "coding", None, false);
    let run = AgentRun {
        id: new_id("run"),
        session_id: session.id.clone(),
        plan_id: None,
        trigger: "composer_chat".to_string(),
        status: "running".to_string(),
        active_todo_ids: vec![],
        completed_todo_ids: vec![],
        failed_todo_ids: vec![],
        parallel_limit: 4,
        created_at: now_ts(),
        updated_at: now_ts(),
    };
    services.runtime.save_run(&run).await;
    let control = services.runtime.register_run(&run.id);

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/runs/{}:steer", run.id),
        json!({ "text": "change of plan" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));

    // Message queued for the loop + `agent.steered` event emitted.
    assert_eq!(control.drain_steering(), vec!["change of plan".to_string()]);
    assert!(services
        .bus
        .snapshot(&session.id)
        .iter()
        .any(|e| e.event_type == "agent.steered"));

    // Empty text is invalid.
    let (status, _) = post(
        &router,
        &format!("/api/agent-debug/runs/{}:steer", run.id),
        json!({ "text": "  " }),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    services.runtime.unregister_run(&run.id);
}

#[tokio::test]
async fn legacy_history_migrates_to_rollout_on_first_read() {
    use agent_protocol::models::ChatMessage;

    let services = test_services();
    let session = services.sessions.create("legacy", "coding", None, false);

    // Simulate a pre-rollout install: history lives as a `hist:{id}` blob.
    let legacy = serde_json::to_string(&vec![
        ChatMessage::user("old q"),
        ChatMessage::assistant("old a"),
    ])
    .unwrap();
    services
        .store
        .kv_put(&format!("hist:{}", session.id), &legacy)
        .unwrap();
    assert!(!services.sessions.rollout().exists(&session.id));

    // First read migrates: same messages, rollout file created, blob dropped.
    let hist = services.sessions.history(&session.id);
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].content, "old q");
    assert!(services.sessions.rollout().exists(&session.id));
    assert!(services
        .store
        .kv_get(&format!("hist:{}", session.id))
        .is_none());

    // New turns append to the rollout and read back in order.
    services.sessions.remember_turn(
        &session.id,
        ChatMessage::user("new q"),
        ChatMessage::assistant("new a"),
    );
    let hist = services.sessions.history(&session.id);
    assert_eq!(hist.len(), 4);
    assert_eq!(hist[3].content, "new a");

    // Fork copies the rollout file.
    services.sessions.copy_history(&session.id, "forked-id");
    assert_eq!(services.sessions.history("forked-id").len(), 4);

    // Clear removes it.
    services.sessions.clear_history(&session.id);
    assert!(services.sessions.history(&session.id).is_empty());
}

#[tokio::test]
async fn skills_subagents_tools_swarm_shapes() {
    let router = test_app();

    // Skills: Python parity shape `{items: [...]}`.
    let (status, body) = get(&router, "/api/agent-debug/skills").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].is_array());

    // Builtin subagents ported from `builtin_subagents.py`.
    let (status, body) = get(&router, "/api/agent-debug/subagents").await;
    assert_eq!(status, StatusCode::OK);
    let subagents = body["subagents"].as_array().unwrap();
    assert!(subagents
        .iter()
        .any(|s| s["name"] == json!("general") || s["id"] == json!("general")));

    // Tools list exposes `items` (+ legacy `tools`).
    let (status, body) = get(&router, "/api/agent-debug/tools").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["items"].as_array().unwrap().is_empty());

    // Swarm: seed-demo populates real coordinator state.
    let (status, body) = post(&router, "/api/agent-debug/swarm/seed-demo", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["nodes"].as_array().unwrap().is_empty());
    let (status, body) = get(&router, "/api/agent-debug/swarm/state").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["nodes"].as_array().unwrap().is_empty());
    assert!(body["shards"].is_array());
}

#[tokio::test]
async fn permission_rules_shells_hooks_endpoints() {
    let services = test_services();
    let router = routes::router(services.clone());

    // Rules: empty by default, round-trip through PUT/GET.
    let (status, body) = get(&router, "/api/agent-debug/permissions/rules").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rules"], json!([]));

    let (status, body) = put(
        &router,
        "/api/agent-debug/permissions/rules",
        json!({ "rules": [
            { "action": "deny", "pattern": "delete_file(*)" },
            { "action": "allow", "pattern": "run_command(git *)" },
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rules"].as_array().unwrap().len(), 2);

    let (_, body) = get(&router, "/api/agent-debug/permissions/rules").await;
    assert_eq!(body["rules"][0]["action"], json!("deny"));

    // Invalid action is rejected with the error envelope.
    let (status, body) = put(
        &router,
        "/api/agent-debug/permissions/rules",
        json!({ "rules": [{ "action": "nope", "pattern": "x(*)" }] }),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(body["error"]["code"].is_string());

    // Hooks config: empty list when no hooks.json exists.
    let (status, body) = get(&router, "/api/agent-debug/hooks").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["hooks"].is_array());

    // Shells: empty list, then a real background job shows up and is killable.
    let (status, body) = get(&router, "/api/agent-debug/shells").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["shells"].as_array().unwrap().is_empty());

    let (status, body) = get(&router, "/api/agent-debug/shells/nope/output").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], json!("SHELL_NOT_FOUND"));

    let outcome = services
        .runtime
        .shell
        .run(
            "echo hello-shell-endpoint",
            &services.runtime.workspace_root(),
            10_000,
            None,
        )
        .await
        .expect("spawn shell job");
    let (status, body) = get(&router, "/api/agent-debug/shells").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["shells"].as_array().unwrap().len(), 1);

    let (status, body) = get(
        &router,
        &format!("/api/agent-debug/shells/{}/output?offset=0", outcome.job.id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["output"]
        .as_str()
        .unwrap()
        .contains("hello-shell-endpoint"));
    assert_eq!(body["running"], json!(false));

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/shells/{}:kill", outcome.job.id),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["ok"].is_boolean());
}

#[tokio::test]
async fn openapi_document_covers_router() {
    let router = test_app();
    let (status, body) = get(&router, "/api/agent-debug/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["info"]["title"], json!("agentd"));

    let paths = body["paths"].as_object().unwrap();
    // Every ROUTES row must appear with its method in the generated doc.
    for (method, path, _, _) in crate::api::openapi::ROUTES {
        let item = paths
            .get(*path)
            .unwrap_or_else(|| panic!("missing path in openapi doc: {path}"));
        assert!(
            item.get(*method).is_some(),
            "missing operation {method} {path}"
        );
    }
    // And the document must know about the endpoints added this phase.
    for p in [
        "/api/agent-debug/permissions/rules",
        "/api/agent-debug/shells",
        "/api/agent-debug/hooks",
        "/api/agent-debug/mcp/servers",
    ] {
        assert!(paths.contains_key(p), "openapi doc missing {p}");
    }
}

#[tokio::test]
async fn run_metrics_include_tool_durations_and_steps() {
    use agent_protocol::events::EventDraft;
    use agent_protocol::models::{now_ts, AgentRun};

    let services = test_services();
    let router = routes::router(services.clone());

    let session = services.sessions.create("metrics", "coding", None, false);
    let run = AgentRun {
        id: new_id("run"),
        session_id: session.id.clone(),
        plan_id: None,
        trigger: "composer_chat".to_string(),
        status: "completed".to_string(),
        active_todo_ids: vec![],
        completed_todo_ids: vec![],
        failed_todo_ids: vec![],
        parallel_limit: 4,
        created_at: now_ts(),
        updated_at: now_ts(),
    };
    services.runtime.save_run(&run).await;

    let emit = |event_type: &str, payload: Value| {
        services.bus.emit(
            EventDraft::new(&session.id, event_type, "agent")
                .payload(payload)
                .correlation(Some(run.id.clone())),
        );
    };
    emit("agent.step", json!({ "step": 1 }));
    emit("agent.step", json!({ "step": 2 }));
    emit(
        "agent.tool.completed",
        json!({ "name": "read_file", "durationMs": 12 }),
    );
    emit(
        "agent.tool.failed",
        json!({ "name": "run_command", "durationMs": 30 }),
    );
    emit(
        "agent.usage",
        json!({ "promptTokens": 100, "completionTokens": 20, "totalTokens": 120, "cacheReadTokens": 50 }),
    );

    let (status, body) = get(
        &router,
        &format!("/api/agent-debug/runs/{}/metrics", run.id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stepsTotal"], json!(2));
    assert_eq!(body["toolDurationMsTotal"], json!(42));
    assert_eq!(body["toolsByName"]["read_file"]["durationMs"], json!(12));
    assert_eq!(body["toolsByName"]["run_command"]["calls"], json!(1));
    assert_eq!(body["usage"]["cacheReadTokens"], json!(50));
    assert!(body["storeWriteFailures"].is_number());
}

// ---- engine loop tests driven by a scripted provider ----

/// Deterministic provider that pops pre-scripted responses, records every
/// request it receives, and runs an optional hook on each call (used to steer
/// or cancel the run from "inside" the model).
struct ScriptedProvider {
    responses:
        std::sync::Mutex<std::collections::VecDeque<agent_providers::types::ProviderResponse>>,
    requests: std::sync::Mutex<Vec<agent_providers::types::ProviderRequest>>,
    on_call: Option<Box<dyn Fn(usize) + Send + Sync>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<agent_providers::types::ProviderResponse>) -> Self {
        ScriptedProvider {
            responses: std::sync::Mutex::new(responses.into()),
            requests: std::sync::Mutex::new(Vec::new()),
            on_call: None,
        }
    }

    fn with_hook(mut self, hook: impl Fn(usize) + Send + Sync + 'static) -> Self {
        self.on_call = Some(Box::new(hook));
        self
    }

    fn text_response(text: &str) -> agent_providers::types::ProviderResponse {
        let mut resp =
            agent_providers::types::ProviderResponse::empty("scripted", "scripted-model");
        resp.text = text.to_string();
        resp
    }

    fn tool_call_response(name: &str, args: Value) -> agent_providers::types::ProviderResponse {
        let mut resp =
            agent_providers::types::ProviderResponse::empty("scripted", "scripted-model");
        resp.finish_reason = "tool_calls".to_string();
        resp.tool_calls = vec![agent_protocol::models::ToolCall {
            id: new_id("call"),
            kind: "function".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }];
        resp
    }

    fn recorded_requests(&self) -> Vec<agent_providers::types::ProviderRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl agent_providers::LLMProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn chat(
        &self,
        req: &agent_providers::types::ProviderRequest,
        _sink: agent_providers::types::DeltaSink<'_>,
    ) -> agent_protocol::ApiResult<agent_providers::types::ProviderResponse> {
        let call_no = {
            let mut reqs = self.requests.lock().unwrap();
            reqs.push(req.clone());
            reqs.len()
        };
        if let Some(hook) = &self.on_call {
            hook(call_no);
        }
        self.responses.lock().unwrap().pop_front().ok_or_else(|| {
            agent_protocol::ApiError::new("PROVIDER_UNAVAILABLE", "script exhausted")
        })
    }
}

#[tokio::test]
async fn react_loop_runs_tools_and_injects_steering() {
    let services = test_services();
    let session = services.sessions.create("loop", "coding", None, false);
    let sid = session.id.clone();

    std::fs::write(
        services.runtime.workspace_root().join("a.txt"),
        "alpha-content",
    )
    .unwrap();

    // Step 1 reads a file; while the tools run a steering message arrives;
    // step 2 must see both the tool result and the steered user message.
    let steer_services = services.clone();
    let steer_sid = sid.clone();
    let provider = std::sync::Arc::new(
        ScriptedProvider::new(vec![
            ScriptedProvider::tool_call_response("read_file", json!({ "path": "a.txt" })),
            ScriptedProvider::text_response("FINAL-ANSWER"),
        ])
        .with_hook(move |call_no| {
            if call_no == 1 {
                let run_id = steer_services
                    .sessions
                    .get(&steer_sid)
                    .ok()
                    .and_then(|s| s.active_run_id)
                    .expect("active run");
                assert!(steer_services.runtime.steer_run(&run_id, "STEER-MSG"));
            }
        }),
    );
    services.providers.override_chain(vec![provider.clone()]);

    let out = services
        .runtime
        .run_composer_chat(&sid, "读取 a.txt", "build", None)
        .await
        .expect("chat turn");
    assert_eq!(out["message"]["text"], json!("FINAL-ANSWER"));
    assert_eq!(out["run"]["status"], json!("completed"));

    // Second provider request: tool result + steered message both present.
    let reqs = provider.recorded_requests();
    assert_eq!(reqs.len(), 2);
    let second = &reqs[1];
    assert!(second
        .messages
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("alpha-content")));
    assert!(second
        .messages
        .iter()
        .any(|m| m.role == "user" && m.content == "STEER-MSG"));

    // Unified event emission: invoked + completed (with duration) + steered.
    let events = services.bus.snapshot(&sid);
    assert!(events.iter().any(|e| e.event_type == "agent.tool.invoked"));
    let completed = events
        .iter()
        .find(|e| e.event_type == "agent.tool.completed")
        .expect("tool completed event");
    assert!(completed.payload["durationMs"].is_number());
    assert!(events.iter().any(|e| e.event_type == "agent.steered"));

    // Rollout persistence: the steered message survives as history.
    let hist = services.sessions.history(&sid);
    assert!(hist
        .iter()
        .any(|m| m.role == "user" && m.content == "STEER-MSG"));
    assert!(hist.iter().any(|m| m.content == "FINAL-ANSWER"));
}

#[tokio::test]
async fn react_loop_guard_stops_repeated_identical_calls() {
    let services = test_services();
    let session = services
        .sessions
        .create("loop-guard", "coding", None, false);
    std::fs::write(services.runtime.workspace_root().join("b.txt"), "b").unwrap();

    // The model "gets stuck" issuing the identical call forever; the guard
    // must stop the loop after `repeated_tool_limit` (default 8) occurrences.
    let stuck: Vec<_> = (0..12)
        .map(|_| ScriptedProvider::tool_call_response("read_file", json!({ "path": "b.txt" })))
        .collect();
    let provider = std::sync::Arc::new(ScriptedProvider::new(stuck));
    services.providers.override_chain(vec![provider.clone()]);

    let out = services
        .runtime
        .run_composer_chat(&session.id, "陷入循环", "build", None)
        .await
        .expect("guarded turn still completes");
    let text = out["message"]["text"].as_str().unwrap();
    assert!(text.contains("重复工具调用"), "guard text, got: {text}");
    // The guard fired before the script was exhausted (12 responses queued).
    assert!(provider.recorded_requests().len() <= 9);
}

#[tokio::test]
async fn react_loop_cancel_mid_run_marks_run_cancelled() {
    let services = test_services();
    let session = services
        .sessions
        .create("loop-cancel", "coding", None, false);
    let sid = session.id.clone();
    std::fs::write(services.runtime.workspace_root().join("c.txt"), "c").unwrap();

    let cancelled_run: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let hook_services = services.clone();
    let hook_sid = sid.clone();
    let hook_run = cancelled_run.clone();
    let provider = std::sync::Arc::new(
        ScriptedProvider::new(vec![
            ScriptedProvider::tool_call_response("read_file", json!({ "path": "c.txt" })),
            ScriptedProvider::text_response("should never be reached"),
        ])
        .with_hook(move |call_no| {
            if call_no == 1 {
                let run_id = hook_services
                    .sessions
                    .get(&hook_sid)
                    .ok()
                    .and_then(|s| s.active_run_id)
                    .expect("active run");
                assert!(hook_services.runtime.cancel_run(&run_id));
                *hook_run.lock().unwrap() = Some(run_id);
            }
        }),
    );
    services.providers.override_chain(vec![provider]);

    let err = services
        .runtime
        .run_composer_chat(&sid, "取消我", "build", None)
        .await
        .expect_err("cancelled run must error");
    assert_eq!(err.code, "RUN_CANCELLED");

    let run_id = cancelled_run.lock().unwrap().clone().unwrap();
    let run = services.runtime.get_run(&run_id).unwrap();
    assert_eq!(run.status, "cancelled");
    assert!(services
        .bus
        .snapshot(&sid)
        .iter()
        .any(|e| e.event_type == "agent.failed"));
}

#[tokio::test]
async fn plan_mode_builds_ready_plan_and_waits_for_confirmation() {
    let services = test_services();
    let router = routes::router(services.clone());
    let (_, s) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "plan-mode", "agentKind": "coding" }),
    )
    .await;
    let sid = s["session"]["id"].as_str().unwrap().to_string();

    // Plan turn: the model "researches" then writes the todo list itself;
    // two extra text responses cover the later per-todo execution runs.
    let provider = std::sync::Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call_response(
            "plan_write",
            json!({ "todos": [
                { "title": "调研模块", "kind": "explore" },
                { "title": "实施修改", "kind": "edit", "dependsOn": [0] },
            ]}),
        ),
        ScriptedProvider::text_response("计划已生成：先调研、后实施，确认后开始执行。"),
        ScriptedProvider::text_response("调研结论：无阻塞。"),
        ScriptedProvider::text_response("修改完成。"),
    ]));
    services.providers.override_chain(vec![provider.clone()]);

    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/ask:execute"),
        json!({ "userInput": "做个功能", "composerMode": "plan" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["composerMode"], json!("plan"));

    // The plan is assembled from the agent-authored todos and stays `ready`.
    assert_eq!(body["plan"]["status"], json!("ready"));
    let plan_id = body["plan"]["id"].as_str().unwrap().to_string();
    let tasks = body["plan"]["stages"][0]["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[1]["dependsOn"].as_array().unwrap().len(), 1);

    // Read-only gating: no write tools were offered to the planning turn.
    let write_tools = [
        "write_file",
        "create_document",
        "delete_file",
        "str_replace_edit",
        "apply_patch",
        "run_command",
        "stop_command",
    ];
    let plan_req = &provider.recorded_requests()[0];
    assert!(
        plan_req
            .tools
            .iter()
            .all(|t| !write_tools.contains(&t.name.as_str())),
        "plan mode must not expose write tools"
    );
    assert!(
        plan_req.tools.iter().any(|t| t.name == "plan_write"),
        "plan mode must expose plan_write"
    );
    assert!(
        !plan_req.tools.iter().any(|t| t.name == "todo_write"),
        "plan mode must not expose todo_write"
    );
    assert!(
        !plan_req.tools.iter().any(|t| t.name == "todo_update"),
        "plan mode must not expose todo_update"
    );

    // No auto-execution: the todos are still queued, the plan is active.
    let todos = services.todos.list_by_session(&sid);
    assert_eq!(todos.len(), 2);
    assert!(todos.iter().all(|t| t.status == "queued"));
    assert_eq!(
        services
            .sessions
            .get(&sid)
            .unwrap()
            .active_plan_id
            .as_deref(),
        Some(plan_id.as_str())
    );

    // User confirmation: explicit execute drives the DAG to completion.
    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/plans/{plan_id}:execute"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["run"]["status"], json!("completed"));
    let todos = services.todos.list_by_session(&sid);
    assert!(todos.iter().all(|t| t.status == "completed"));

    // Lifecycle events carried explicit statuses for the frontend merge.
    let events = services.bus.snapshot(&sid);
    assert!(events
        .iter()
        .filter(|e| e.event_type == "todo.started")
        .all(|e| e.payload["status"] == json!("running")));
    assert!(events
        .iter()
        .filter(|e| e.event_type == "todo.completed")
        .all(|e| e.payload["status"] == json!("completed")));
}

#[tokio::test]
async fn agent_kind_create_and_mode_downgrade() {
    let services = test_services();
    let router = routes::router(services.clone());

    // Create a document-kind session.
    let (status, body) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "doc", "agentKind": "document" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sid = body["session"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["session"]["agentKind"], json!("document"));

    // A single scripted text reply so ask:execute completes without a plan.
    let provider = std::sync::Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::text_response("文档草稿已完成。"),
    ]));
    services.providers.override_chain(vec![provider]);

    // composerMode=plan must NOT trigger the plan engine for a document agent;
    // it downgrades to build and returns a chat result.
    let (status, body) = post(
        &router,
        &format!("/api/agent-debug/sessions/{sid}/ask:execute"),
        json!({ "userInput": "写个说明", "composerMode": "plan" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["composerMode"], json!("build"));
    assert!(
        body.get("plan").is_none(),
        "document agent must not run the plan engine"
    );
    assert!(body["message"]["text"]
        .as_str()
        .unwrap()
        .contains("文档草稿"));

    // Default create (no agentKind) is the coding profile.
    let (_, body) = post(
        &router,
        "/api/agent-debug/sessions",
        json!({ "title": "legacy" }),
    )
    .await;
    assert_eq!(body["session"]["agentKind"], json!("coding"));
}

#[tokio::test]
async fn memory_tool_persists_and_rest_crud() {
    let services = test_services();
    let router = routes::router(services.clone());
    let session = services.sessions.create("mem", "coding", None, false);

    // The agent writes a memory via the runtime tool, then replies.
    let provider = std::sync::Arc::new(ScriptedProvider::new(vec![
        ScriptedProvider::tool_call_response(
            "memory_write",
            json!({ "content": "用户偏好简体中文回复", "kind": "preference", "scope": "global" }),
        ),
        ScriptedProvider::text_response("好的，已记住。"),
    ]));
    services.providers.override_chain(vec![provider]);
    services
        .runtime
        .run_composer_chat(&session.id, "请记住我偏好中文", "build", None)
        .await
        .unwrap();

    // The memory is queryable via REST and an event was emitted.
    let (status, body) = get(&router, "/api/agent-debug/memories").await;
    assert_eq!(status, StatusCode::OK);
    let mems = body["memories"].as_array().unwrap();
    let agent_mem = mems
        .iter()
        .find(|m| m["content"].as_str().unwrap_or("").contains("简体中文"))
        .expect("agent-written memory present");
    let agent_mem_id = agent_mem["id"].as_str().unwrap().to_string();
    assert_eq!(agent_mem["kind"], json!("preference"));
    assert!(services
        .bus
        .snapshot(&session.id)
        .iter()
        .any(|e| e.event_type == "agent.memory.created"));

    // REST create + delete roundtrip.
    let (status, body) = post(
        &router,
        "/api/agent-debug/memories",
        json!({ "content": "项目使用 redb 存储", "kind": "fact", "scope": "global" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let new_mem_id = body["memory"]["id"].as_str().unwrap().to_string();

    let (status, _) = delete(&router, &format!("/api/agent-debug/memories/{new_mem_id}")).await;
    assert_eq!(status, StatusCode::OK);
    // Deleting a missing memory 404s.
    let (status, _) = delete(&router, &format!("/api/agent-debug/memories/{new_mem_id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The agent-written memory is also deletable.
    let (status, _) = delete(
        &router,
        &format!("/api/agent-debug/memories/{agent_mem_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
