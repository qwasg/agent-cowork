//! axum router mirroring the full `/api/agent-debug/*` surface from `server.py`.
//! Colon-action paths (`{id}:fork`) are matched by capturing the whole segment
//! and splitting on the last `:` (axum/matchit can't match in-segment suffixes).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};

/// Body extractor that tolerates an empty body or missing JSON content-type
/// (returns `{}`), matching the permissive Python `payload: Dict` contract.
pub struct OptJson(pub Value);

impl<S> FromRequest<S> for OptJson
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state).await.unwrap_or_default();
        if bytes.is_empty() {
            return Ok(OptJson(json!({})));
        }
        Ok(OptJson(serde_json::from_slice(&bytes).unwrap_or(json!({}))))
    }
}

use crate::api::gateway::AppServices;
use crate::api::sse;
use crate::contracts::ApiResult;

type App = State<Arc<AppServices>>;

fn resp(r: ApiResult<Value>) -> (StatusCode, Json<Value>) {
    match r {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (
            StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::BAD_REQUEST),
            Json(serde_json::to_value(e.envelope()).unwrap_or(json!({}))),
        ),
    }
}

fn body_or_empty(b: OptJson) -> Value {
    b.0
}

fn split_action(seg: &str) -> (String, String) {
    match seg.rsplit_once(':') {
        Some((id, action)) => (id.to_string(), action.to_string()),
        None => (seg.to_string(), String::new()),
    }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
}

pub fn router(app: Arc<AppServices>) -> Router {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        // auth
        .route("/api/agent-debug/auth/register", post(auth_register))
        .route("/api/agent-debug/auth/login", post(auth_login))
        .route("/api/agent-debug/auth/me", get(auth_me))
        .route("/api/agent-debug/auth/profile", patch(auth_profile))
        .route(
            "/api/agent-debug/auth/logout",
            post(|| async { Json(json!({ "ok": true })) }),
        )
        // sessions
        .route(
            "/api/agent-debug/sessions",
            get(list_sessions).post(create_session),
        )
        .route("/api/agent-debug/design-snapshot", get(design_snapshot))
        .route(
            "/api/agent-debug/sessions/{id}",
            get(get_session)
                .patch(patch_session)
                .delete(delete_session)
                .post(session_action),
        )
        .route(
            "/api/agent-debug/sessions/{id}/model",
            put(set_session_model),
        )
        .route(
            "/api/agent-debug/models",
            get(|State(a): App| async move { Json(a.list_models()) }),
        )
        .route(
            "/api/agent-debug/provider-status",
            get(|State(a): App| async move { Json(a.provider_status()) }),
        )
        .route(
            "/api/agent-debug/search-config",
            get(get_search_config).put(set_search_config),
        )
        .route(
            "/api/agent-debug/model-preferences",
            get(get_model_prefs).put(set_model_prefs),
        )
        // channels
        .route(
            "/api/agent-debug/provider-types",
            get(|State(a): App| async move { Json(a.list_provider_types()) }),
        )
        .route(
            "/api/agent-debug/channels",
            get(list_channels).post(create_channel),
        )
        .route("/api/agent-debug/channels:fetch-models", post(fetch_models))
        .route(
            "/api/agent-debug/channels/{id}",
            put(update_channel).delete(delete_channel),
        )
        // permissions / subagents / skills
        .route(
            "/api/agent-debug/sessions/{id}/permission-mode",
            get(get_perm).put(set_perm),
        )
        .route(
            "/api/agent-debug/permissions/{seg}",
            post(permission_action),
        )
        .route(
            "/api/agent-debug/subagents",
            get(|State(a): App| async move { Json(a.list_builtin_subagents()) }),
        )
        .route(
            "/api/agent-debug/skills",
            get(|State(a): App| async move { Json(a.list_skills()) }),
        )
        .route("/api/agent-debug/skills/{name}", get(read_skill))
        // checkpoints
        .route(
            "/api/agent-debug/sessions/{id}/checkpoints",
            get(list_checkpoints).post(create_checkpoint),
        )
        .route("/api/agent-debug/checkpoints/{id}", post(checkpoint_action))
        // SSE
        .route(
            "/api/agent-debug/sessions/{id}/events/stream",
            get(events_stream),
        )
        // mcp demo
        .route("/api/agent-debug/mcp/demo/status", get(mcp_status))
        .route("/api/agent-debug/mcp/demo/call", post(mcp_call))
        // chat / plan
        .route(
            "/api/agent-debug/sessions/{id}/plan:generate",
            post(generate_plan),
        )
        .route(
            "/api/agent-debug/sessions/{id}/ask:execute",
            post(ask_execute),
        )
        .route(
            "/api/agent-debug/plans/{id}",
            get(get_plan).patch(patch_plan).post(plan_action),
        )
        // todos
        .route("/api/agent-debug/sessions/{id}/todos", get(get_todos))
        .route("/api/agent-debug/todos", post(add_todo))
        .route("/api/agent-debug/todos:batch-rerun", post(batch_rerun))
        .route("/api/agent-debug/todos/{id}", patch(patch_todo))
        // runs
        .route("/api/agent-debug/runs/{id}", get(get_run).post(run_action))
        .route("/api/agent-debug/runs/{id}/logs", get(get_run_logs))
        .route("/api/agent-debug/runs/{id}/metrics", get(get_run_metrics))
        .route(
            "/api/agent-debug/runs/{id}/todos/{seg}",
            post(run_todo_rerun),
        )
        .route(
            "/api/agent-debug/runs/{id}/nodes/{seg}",
            post(run_node_rerun),
        )
        // proposals
        .route("/api/agent-debug/proposals", get(list_proposals))
        .route("/api/agent-debug/proposals/{id}", post(proposal_action))
        // replay / swarm / workspace / tools
        .route("/api/agent-debug/replay/{id}", get(get_replay))
        .route("/api/agent-debug/replay/{id}/since", get(replay_since))
        .route(
            "/api/agent-debug/swarm/state",
            get(|State(a): App| async move { Json(a.list_swarm_state()) }),
        )
        .route(
            "/api/agent-debug/swarm/seed-demo",
            post(|State(a): App| async move { Json(a.seed_swarm_demo()) }),
        )
        .route(
            "/api/agent-debug/workspace/info",
            get(|State(a): App| async move { Json(a.get_workspace_info()) }),
        )
        .route("/api/agent-debug/workspace/tree", get(workspace_tree))
        .route("/api/agent-debug/workspace/browse", get(workspace_browse))
        .route("/api/agent-debug/workspace/root", post(workspace_root))
        .route(
            "/api/agent-debug/workspace/file",
            get(read_ws_file).post(write_ws_file),
        )
        .route("/api/agent-debug/workspace/revert", post(revert_ws_file))
        .route(
            "/api/agent-debug/local-file",
            get(read_local).post(write_local),
        )
        .route(
            "/api/agent-debug/tools",
            get(|State(a): App| async move { Json(a.list_tools()) }),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(app)
}

// ----- auth -----

async fn auth_register(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.auth.register(
        p.get("email").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("password").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("displayName").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("workspace").and_then(|v| v.as_str()).unwrap_or(""),
    ))
}

async fn auth_login(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.auth.login(
        p.get("email").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("password").and_then(|v| v.as_str()).unwrap_or(""),
    ))
}

async fn auth_me(State(a): App, headers: HeaderMap) -> impl IntoResponse {
    match bearer(&headers).and_then(|t| a.auth.user_from_token(&t)) {
        Some(user) => (StatusCode::OK, Json(json!({ "user": user }))),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({ "error": { "code": "AUTH_INVALID", "message": "Invalid or expired token" } }),
            ),
        ),
    }
}

async fn auth_profile(State(a): App, headers: HeaderMap, body: OptJson) -> impl IntoResponse {
    let Some(user) = bearer(&headers).and_then(|t| a.auth.user_from_token(&t)) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({ "error": { "code": "AUTH_INVALID", "message": "Invalid or expired token" } }),
            ),
        );
    };
    let uid = user
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    resp(a.auth.update_profile(&uid, &body_or_empty(body)))
}

// ----- sessions -----

async fn list_sessions(State(a): App) -> Json<Value> {
    Json(a.list_sessions())
}

async fn create_session(State(a): App, body: OptJson) -> Json<Value> {
    let p = body_or_empty(body);
    Json(
        a.create_session(
            p.get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Agent Debug Session"),
            p.get("selectedModelId")
                .and_then(|v| v.as_str())
                .map(String::from),
            p.get("webSearchEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        ),
    )
}

async fn get_session(State(a): App, Path(id): Path<String>) -> impl IntoResponse {
    resp(a.get_session(&id))
}

async fn patch_session(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    resp(a.patch_session(&id, &body_or_empty(body)))
}

async fn delete_session(State(a): App, Path(id): Path<String>) -> impl IntoResponse {
    resp(a.delete_session(&id))
}

async fn session_action(
    State(a): App,
    Path(seg): Path<String>,
    body: OptJson,
) -> impl IntoResponse {
    let (id, action) = split_action(&seg);
    match action.as_str() {
        "fork" => resp(a.fork_session(&id)),
        "revert" => {
            let p = body_or_empty(body);
            resp(a.revert_session(&id, p.get("messageId").and_then(|v| v.as_str())))
        }
        _ => (
            StatusCode::NOT_FOUND,
            Json(
                json!({ "error": { "code": "PATH_NOT_FOUND", "message": "unknown session action" } }),
            ),
        ),
    }
}

async fn set_session_model(
    State(a): App,
    Path(id): Path<String>,
    body: OptJson,
) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.set_session_model(
        &id,
        p.get("modelId").and_then(|v| v.as_str()).map(String::from),
    ))
}

async fn design_snapshot(State(a): App, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    Json(a.get_design_snapshot(q.get("sessionId").map(|s| s.as_str())))
}

async fn get_search_config(State(a): App) -> Json<Value> {
    Json(a.get_search_config())
}
async fn set_search_config(State(a): App, body: OptJson) -> Json<Value> {
    Json(a.set_search_config(&body_or_empty(body)))
}
async fn get_model_prefs(State(a): App) -> Json<Value> {
    Json(a.get_model_preferences())
}
async fn set_model_prefs(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.set_model_preferences(p.get("modelId").and_then(|v| v.as_str()).unwrap_or("")))
}

// ----- channels -----

async fn list_channels(State(a): App) -> Json<Value> {
    Json(a.list_channels())
}
async fn create_channel(State(a): App, body: OptJson) -> impl IntoResponse {
    resp(a.upsert_channel(&body_or_empty(body)))
}
async fn update_channel(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    let mut p = body_or_empty(body);
    if let Value::Object(ref mut m) = p {
        m.insert("id".to_string(), json!(id));
    }
    resp(a.upsert_channel(&p))
}
async fn delete_channel(State(a): App, Path(id): Path<String>) -> impl IntoResponse {
    resp(a.delete_channel(&id))
}
async fn fetch_models(State(a): App, body: OptJson) -> impl IntoResponse {
    resp(a.fetch_channel_models(&body_or_empty(body)).await)
}

// ----- permissions / skills -----

async fn get_perm(State(a): App, Path(id): Path<String>) -> Json<Value> {
    Json(a.get_permission_mode(&id))
}
async fn set_perm(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.set_permission_mode(&id, p.get("mode").and_then(|v| v.as_str()).unwrap_or("")))
}
async fn read_skill(State(a): App, Path(name): Path<String>) -> impl IntoResponse {
    resp(a.read_skill_content(&name))
}
async fn permission_action(State(a): App, Path(seg): Path<String>) -> impl IntoResponse {
    let (id, action) = split_action(&seg);
    match action.as_str() {
        "approve" => resp(a.resolve_permission(&id, true)),
        "deny" => resp(a.resolve_permission(&id, false)),
        _ => (
            StatusCode::NOT_FOUND,
            Json(
                json!({ "error": { "code": "PATH_NOT_FOUND", "message": "unknown permission action" } }),
            ),
        ),
    }
}

// ----- checkpoints -----

async fn list_checkpoints(State(a): App, Path(id): Path<String>) -> Json<Value> {
    Json(a.list_checkpoints(&id))
}
async fn create_checkpoint(State(a): App, Path(id): Path<String>, body: OptJson) -> Json<Value> {
    let p = body_or_empty(body);
    let paths = p.get("paths").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    });
    Json(a.create_checkpoint(
        &id,
        paths,
        p.get("label").and_then(|v| v.as_str()).unwrap_or(""),
    ))
}
async fn checkpoint_action(State(a): App, Path(seg): Path<String>) -> impl IntoResponse {
    let (id, action) = split_action(&seg);
    match action.as_str() {
        "rewind" => resp(a.rewind_checkpoint(&id)),
        _ => (
            StatusCode::NOT_FOUND,
            Json(
                json!({ "error": { "code": "PATH_NOT_FOUND", "message": "unknown checkpoint action" } }),
            ),
        ),
    }
}

// ----- SSE -----

async fn events_stream(
    State(a): App,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let from_seq = q
        .get("fromSeq")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    sse::session_stream(a, id, from_seq)
}

// ----- mcp -----

async fn mcp_status(State(a): App) -> impl IntoResponse {
    (StatusCode::SERVICE_UNAVAILABLE, Json(a.mcp_demo_status()))
}
async fn mcp_call(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(a.mcp_demo_call(
            p.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            p.get("arguments"),
        )),
    )
}

// ----- chat / plan -----

async fn generate_plan(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(
        a.generate_plan(
            &id,
            p.get("userInput").and_then(|v| v.as_str()).unwrap_or(""),
            p.get("contextWindow"),
        )
        .await,
    )
}

async fn ask_execute(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(
        a.ask_execute(
            &id,
            p.get("userInput").and_then(|v| v.as_str()).unwrap_or(""),
            p.get("contextWindow"),
            p.get("composerMode").and_then(|v| v.as_str()),
        )
        .await,
    )
}

async fn get_plan(State(a): App, Path(id): Path<String>) -> impl IntoResponse {
    resp(a.get_plan(&id))
}
async fn patch_plan(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    resp(a.patch_plan(&id, &body_or_empty(body)))
}
async fn plan_action(
    State(a): App,
    Path(id_seg): Path<String>,
    body: OptJson,
) -> impl IntoResponse {
    let (id, action) = split_action(&id_seg);
    match action.as_str() {
        "confirm" => resp(a.confirm_plan(&id)),
        "replan" => resp(a.replan(&id, &body_or_empty(body)).await),
        "execute" => resp(a.execute_plan(&id).await),
        _ => (
            StatusCode::NOT_FOUND,
            Json(
                json!({ "error": { "code": "PLAN_NOT_FOUND", "message": "unknown plan action" } }),
            ),
        ),
    }
}

// ----- todos -----

async fn get_todos(State(a): App, Path(id): Path<String>) -> Json<Value> {
    Json(a.get_todos(&id))
}
async fn add_todo(State(a): App, body: OptJson) -> impl IntoResponse {
    resp(a.add_todo(&body_or_empty(body)))
}
async fn patch_todo(State(a): App, Path(id): Path<String>, body: OptJson) -> impl IntoResponse {
    resp(a.patch_todo(&id, &body_or_empty(body)))
}
async fn batch_rerun(State(a): App, body: OptJson) -> impl IntoResponse {
    resp(a.batch_rerun_todos(&body_or_empty(body)))
}

// ----- runs -----

async fn get_run(State(a): App, Path(seg): Path<String>) -> impl IntoResponse {
    // GET /runs/{id} (no action suffix expected)
    let (id, _action) = split_action(&seg);
    resp(a.get_run(&id))
}
async fn run_action(State(a): App, Path(seg): Path<String>) -> impl IntoResponse {
    let (id, action) = split_action(&seg);
    let out = match action.as_str() {
        "pause" => a.pause_run(&id),
        "resume" => a.resume_run(&id),
        "cancel" => a.cancel_run(&id),
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(
                    json!({ "error": { "code": "RUN_NOT_FOUND", "message": "unknown run action" } }),
                ),
            )
        }
    };
    (StatusCode::OK, Json(out))
}
async fn get_run_logs(State(a): App, Path(id): Path<String>) -> impl IntoResponse {
    resp(a.get_run_logs(&id))
}
async fn get_run_metrics(State(a): App, Path(id): Path<String>) -> impl IntoResponse {
    resp(a.get_run_metrics(&id))
}
async fn run_todo_rerun(
    State(a): App,
    Path((run_id, seg)): Path<(String, String)>,
) -> impl IntoResponse {
    let (todo_id, _) = split_action(&seg);
    resp(a.rerun_todo(&run_id, &todo_id))
}
async fn run_node_rerun(
    State(a): App,
    Path((run_id, seg)): Path<(String, String)>,
) -> impl IntoResponse {
    let (node_id, _) = split_action(&seg);
    resp(a.rerun_node(&run_id, &node_id))
}

// ----- proposals -----

async fn list_proposals(
    State(a): App,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    resp(a.list_proposals(q.get("sessionId").map(|s| s.as_str()).unwrap_or("")))
}
async fn proposal_action(State(a): App, Path(seg): Path<String>) -> impl IntoResponse {
    let (id, action) = split_action(&seg);
    match action.as_str() {
        "apply" => resp(a.apply_proposal(&id)),
        "discard" => resp(a.discard_proposal(&id)),
        _ => (
            StatusCode::NOT_FOUND,
            Json(
                json!({ "error": { "code": "PROPOSAL_NOT_FOUND", "message": "unknown proposal action" } }),
            ),
        ),
    }
}

// ----- replay / workspace -----

async fn get_replay(State(a): App, Path(id): Path<String>) -> Json<Value> {
    Json(a.get_replay(&id))
}
async fn replay_since(
    State(a): App,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let from_seq = q
        .get("fromSeq")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let limit = q.get("limit").and_then(|s| s.parse::<usize>().ok());
    Json(a.replay_since(&id, from_seq, limit))
}
async fn workspace_tree(
    State(a): App,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let path = q.get("path").cloned().unwrap_or_default();
    let show_heavy = q.get("showHeavy").map(|v| v == "true").unwrap_or(false);
    resp(a.get_workspace_tree(&path, show_heavy))
}
async fn workspace_browse(
    State(a): App,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    resp(a.browse_directories(q.get("path").map(|s| s.as_str()).unwrap_or("")))
}
async fn workspace_root(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.set_workspace_root(p.get("path").and_then(|v| v.as_str()).unwrap_or("")))
}
async fn read_ws_file(
    State(a): App,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    resp(a.read_workspace_file(q.get("path").map(|s| s.as_str()).unwrap_or("")))
}
async fn write_ws_file(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.write_workspace_file(
        p.get("path").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("content").and_then(|v| v.as_str()).unwrap_or(""),
    ))
}
async fn revert_ws_file(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.revert_workspace_file(p.get("path").and_then(|v| v.as_str()).unwrap_or("")))
}
async fn read_local(State(a): App, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    resp(a.read_local_file(q.get("path").map(|s| s.as_str()).unwrap_or("")))
}
async fn write_local(State(a): App, body: OptJson) -> impl IntoResponse {
    let p = body_or_empty(body);
    resp(a.write_local_file(
        p.get("path").and_then(|v| v.as_str()).unwrap_or(""),
        p.get("content").and_then(|v| v.as_str()).unwrap_or(""),
    ))
}
