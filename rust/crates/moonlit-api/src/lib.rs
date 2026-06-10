//! `moonlit-api` — async Rust client for the Python agent-debug backend.
//!
//! Covers the same transport surface as `apps/agent-ide/public/api-client.jsx`:
//! REST, WebSocket event subscription with `fromSeq` continuation, and SSE.

use futures_util::{SinkExt, StreamExt};
use moonlit_core::models::*;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub const DEFAULT_API_BASE: &str = "http://127.0.0.1:8002";

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("http request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("websocket failed: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("backend returned empty success body for {0}")]
    EmptyBody(String),
    #[error("request failed ({status}) {path}: {message}")]
    Backend {
        status: u16,
        path: String,
        message: String,
    },
    #[error("send failed")]
    Send,
}

pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone)]
pub struct MoonlitAgentApi {
    base_url: Arc<str>,
    auth_token: Option<Arc<str>>,
    client: reqwest::Client,
}

impl Default for MoonlitAgentApi {
    fn default() -> Self {
        Self::new(DEFAULT_API_BASE)
    }
}

impl MoonlitAgentApi {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url: Arc::from(base_url),
            auth_token: None,
            client: reqwest::Client::new(),
        }
    }

    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        self.auth_token = (!token.is_empty()).then(|| Arc::from(token));
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<Value>,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(token) = &self.auth_token {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(AUTHORIZATION, value);
            }
        }
        let mut builder = self.client.request(method, &url).headers(headers);
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let response = builder.send().await?;
        let status = response.status();
        let text = response.text().await?;
        if status.is_success() && text.trim().is_empty() {
            return Err(ApiError::EmptyBody(path));
        }
        let payload: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        };
        if !status.is_success() {
            return Err(ApiError::Backend {
                status: status.as_u16(),
                path,
                message: backend_error_message(&payload),
            });
        }
        if let Some(error) = payload.get("error") {
            return Err(ApiError::Backend {
                status: status.as_u16(),
                path,
                message: backend_error_message(error),
            });
        }
        Ok(serde_json::from_value(payload)?)
    }

    async fn get<T: DeserializeOwned>(&self, path: impl Into<String>) -> Result<T> {
        self.request(reqwest::Method::GET, path.into(), None).await
    }

    async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        path: impl Into<String>,
        body: B,
    ) -> Result<T> {
        self.request(
            reqwest::Method::POST,
            path.into(),
            Some(serde_json::to_value(body)?),
        )
        .await
    }

    async fn put<T: DeserializeOwned, B: Serialize>(
        &self,
        path: impl Into<String>,
        body: B,
    ) -> Result<T> {
        self.request(
            reqwest::Method::PUT,
            path.into(),
            Some(serde_json::to_value(body)?),
        )
        .await
    }

    async fn patch<T: DeserializeOwned, B: Serialize>(
        &self,
        path: impl Into<String>,
        body: B,
    ) -> Result<T> {
        self.request(
            reqwest::Method::PATCH,
            path.into(),
            Some(serde_json::to_value(body)?),
        )
        .await
    }

    async fn delete<T: DeserializeOwned>(&self, path: impl Into<String>) -> Result<T> {
        self.request(reqwest::Method::DELETE, path.into(), None).await
    }

    pub async fn health(&self) -> Result<Value> {
        self.get("/health").await
    }

    pub async fn register(&self, payload: Value) -> Result<AuthResponse> {
        self.post("/api/agent-debug/auth/register", payload).await
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<AuthResponse> {
        self.post(
            "/api/agent-debug/auth/login",
            json!({ "email": email, "password": password }),
        )
        .await
    }

    pub async fn me(&self) -> Result<Value> {
        self.get("/api/agent-debug/auth/me").await
    }

    pub async fn update_profile(&self, patch: Value) -> Result<Value> {
        self.patch("/api/agent-debug/auth/profile", patch).await
    }

    pub async fn logout(&self) -> Result<Value> {
        self.post("/api/agent-debug/auth/logout", json!({})).await
    }

    pub async fn provider_status(&self) -> Result<Value> {
        self.get("/api/agent-debug/provider-status").await
    }

    pub async fn search_config(&self) -> Result<Value> {
        self.get("/api/agent-debug/search-config").await
    }

    pub async fn set_search_config(&self, config: Value) -> Result<Value> {
        self.put("/api/agent-debug/search-config", config).await
    }

    pub async fn snapshot(&self, session_id: Option<&str>) -> Result<DesignSnapshot> {
        let path = match session_id {
            Some(id) if !id.is_empty() => {
                format!("/api/agent-debug/design-snapshot?sessionId={}", enc(id))
            }
            _ => "/api/agent-debug/design-snapshot".to_string(),
        };
        self.get(path).await
    }

    pub async fn list_sessions(&self) -> Result<Value> {
        self.get("/api/agent-debug/sessions").await
    }

    pub async fn create_session(
        &self,
        title: Option<&str>,
        selected_model_id: Option<&str>,
        web_search_enabled: bool,
    ) -> Result<Value> {
        self.post(
            "/api/agent-debug/sessions",
            json!({
                "title": title,
                "selectedModelId": selected_model_id,
                "webSearchEnabled": web_search_enabled
            }),
        )
        .await
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/sessions/{}", enc(session_id)))
            .await
    }

    pub async fn patch_session(&self, session_id: &str, patch: Value) -> Result<Value> {
        self.patch(
            format!("/api/agent-debug/sessions/{}", enc(session_id)),
            patch,
        )
        .await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<Value> {
        self.delete(format!("/api/agent-debug/sessions/{}", enc(session_id)))
            .await
    }

    pub async fn fork_session(&self, session_id: &str) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/sessions/{}:fork", enc(session_id)),
            json!({}),
        )
        .await
    }

    pub async fn revert_session(
        &self,
        session_id: &str,
        message_id: Option<&str>,
        before: bool,
    ) -> Result<Value> {
        let mut body = json!({ "messageId": message_id });
        if before {
            body["mode"] = json!("before");
        }
        self.post(
            format!("/api/agent-debug/sessions/{}:revert", enc(session_id)),
            body,
        )
        .await
    }

    pub async fn ask_execute(
        &self,
        session_id: &str,
        user_input: &str,
        context_window: Option<Value>,
        composer_mode: &str,
    ) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/sessions/{}/ask:execute", enc(session_id)),
            json!({
                "userInput": user_input,
                "contextWindow": context_window,
                "composerMode": composer_mode
            }),
        )
        .await
    }

    pub async fn generate_plan(
        &self,
        session_id: &str,
        user_input: &str,
        context_window: Option<Value>,
    ) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/sessions/{}/plan:generate", enc(session_id)),
            json!({ "userInput": user_input, "contextWindow": context_window }),
        )
        .await
    }

    pub async fn execute_plan(&self, plan_id: &str) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/plans/{}:execute", enc(plan_id)),
            json!({}),
        )
        .await
    }

    pub async fn list_todos(&self, session_id: &str) -> Result<Value> {
        self.get(format!(
            "/api/agent-debug/sessions/{}/todos",
            enc(session_id)
        ))
        .await
    }

    pub async fn replay(&self, session_id: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/replay/{}", enc(session_id)))
            .await
    }

    pub async fn replay_since(
        &self,
        session_id: &str,
        from_seq: u64,
        limit: Option<u64>,
    ) -> Result<Value> {
        let mut path = format!(
            "/api/agent-debug/replay/{}/since?fromSeq={}",
            enc(session_id),
            from_seq
        );
        if let Some(limit) = limit {
            path.push_str(&format!("&limit={limit}"));
        }
        self.get(path).await
    }

    pub async fn list_models(&self) -> Result<Value> {
        self.get("/api/agent-debug/models").await
    }

    pub async fn model_preferences(&self) -> Result<Value> {
        self.get("/api/agent-debug/model-preferences").await
    }

    pub async fn set_model_preferences(&self, model_id: &str) -> Result<Value> {
        self.put(
            "/api/agent-debug/model-preferences",
            json!({ "modelId": model_id }),
        )
        .await
    }

    pub async fn set_session_model(&self, session_id: &str, model_id: &str) -> Result<Value> {
        self.put(
            format!("/api/agent-debug/sessions/{}/model", enc(session_id)),
            json!({ "modelId": model_id }),
        )
        .await
    }

    pub async fn run_control(&self, run_id: &str, action: &str) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/runs/{}:{}", enc(run_id), action),
            json!({}),
        )
        .await
    }

    pub async fn pause_run(&self, run_id: &str) -> Result<Value> {
        self.run_control(run_id, "pause").await
    }

    pub async fn resume_run(&self, run_id: &str) -> Result<Value> {
        self.run_control(run_id, "resume").await
    }

    pub async fn cancel_run(&self, run_id: &str) -> Result<Value> {
        self.run_control(run_id, "cancel").await
    }

    pub async fn rerun_todo(&self, run_id: &str, todo_id: &str) -> Result<Value> {
        self.post(
            format!(
                "/api/agent-debug/runs/{}/todos/{}:rerun",
                enc(run_id),
                enc(todo_id)
            ),
            json!({}),
        )
        .await
    }

    pub async fn rerun_node(&self, run_id: &str, node_id: &str) -> Result<Value> {
        self.post(
            format!(
                "/api/agent-debug/runs/{}/nodes/{}:rerun",
                enc(run_id),
                enc(node_id)
            ),
            json!({}),
        )
        .await
    }

    pub async fn workspace_info(&self) -> Result<Value> {
        self.get("/api/agent-debug/workspace/info").await
    }

    pub async fn workspace_tree(&self, path: &str, show_heavy: bool) -> Result<Value> {
        self.get(format!(
            "/api/agent-debug/workspace/tree?path={}&showHeavy={}",
            enc(path),
            show_heavy
        ))
        .await
    }

    pub async fn browse_directories(&self, path: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/workspace/browse?path={}", enc(path)))
            .await
    }

    pub async fn set_workspace_root(&self, path: &str) -> Result<Value> {
        self.post("/api/agent-debug/workspace/root", json!({ "path": path }))
            .await
    }

    pub async fn read_workspace_file(&self, path: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/workspace/file?path={}", enc(path)))
            .await
    }

    pub async fn write_workspace_file(&self, path: &str, content: &str) -> Result<Value> {
        self.post(
            "/api/agent-debug/workspace/file",
            json!({ "path": path, "content": content }),
        )
        .await
    }

    pub async fn read_local_file(&self, path: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/local-file?path={}", enc(path)))
            .await
    }

    pub async fn write_local_file(&self, path: &str, content: &str) -> Result<Value> {
        self.post(
            "/api/agent-debug/local-file",
            json!({ "path": path, "content": content }),
        )
        .await
    }

    pub async fn revert_workspace_file(&self, path: &str) -> Result<Value> {
        self.post("/api/agent-debug/workspace/revert", json!({ "path": path }))
            .await
    }

    pub async fn list_proposals(&self, session_id: &str) -> Result<Value> {
        self.get(format!(
            "/api/agent-debug/proposals?sessionId={}",
            enc(session_id)
        ))
        .await
    }

    pub async fn apply_proposal(&self, proposal_id: &str) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/proposals/{}:apply", enc(proposal_id)),
            json!({}),
        )
        .await
    }

    pub async fn discard_proposal(&self, proposal_id: &str) -> Result<Value> {
        self.post(
            format!(
                "/api/agent-debug/proposals/{}:discard",
                enc(proposal_id)
            ),
            json!({}),
        )
        .await
    }

    pub async fn passthrough_get(&self, path: &str) -> Result<Value> {
        self.get(path.to_string()).await
    }

    pub async fn passthrough_post(&self, path: &str, body: Value) -> Result<Value> {
        self.post(path.to_string(), body).await
    }

    // -------------------- 多渠道 / 模型配置 --------------------

    pub async fn list_provider_types(&self) -> Result<Value> {
        self.get("/api/agent-debug/provider-types").await
    }

    pub async fn list_channels(&self) -> Result<Value> {
        self.get("/api/agent-debug/channels").await
    }

    pub async fn fetch_channel_models(&self, payload: Value) -> Result<Value> {
        self.post("/api/agent-debug/channels:fetch-models", payload).await
    }

    pub async fn create_channel(&self, channel: Value) -> Result<Value> {
        self.post("/api/agent-debug/channels", channel).await
    }

    pub async fn update_channel(&self, channel_id: &str, channel: Value) -> Result<Value> {
        self.put(format!("/api/agent-debug/channels/{}", enc(channel_id)), channel)
            .await
    }

    pub async fn delete_channel(&self, channel_id: &str) -> Result<Value> {
        self.delete(format!("/api/agent-debug/channels/{}", enc(channel_id)))
            .await
    }

    // -------------------- 技能（磁盘 SKILL.md） --------------------

    pub async fn list_skills(&self) -> Result<Value> {
        self.get("/api/agent-debug/skills").await
    }

    pub async fn read_skill(&self, name: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/skills/{}", enc(name))).await
    }

    // -------------------- 权限模式 --------------------

    pub async fn get_permission_mode(&self, session_id: &str) -> Result<Value> {
        self.get(format!(
            "/api/agent-debug/sessions/{}/permission-mode",
            enc(session_id)
        ))
        .await
    }

    pub async fn set_permission_mode(&self, session_id: &str, mode: &str) -> Result<Value> {
        self.put(
            format!(
                "/api/agent-debug/sessions/{}/permission-mode",
                enc(session_id)
            ),
            json!({ "mode": mode }),
        )
        .await
    }

    pub async fn list_subagents(&self) -> Result<Value> {
        self.get("/api/agent-debug/subagents").await
    }

    // -------------------- 检查点 --------------------

    pub async fn list_checkpoints(&self, session_id: &str) -> Result<Value> {
        self.get(format!(
            "/api/agent-debug/sessions/{}/checkpoints",
            enc(session_id)
        ))
        .await
    }

    pub async fn create_checkpoint(
        &self,
        session_id: &str,
        paths: Vec<String>,
        label: &str,
    ) -> Result<Value> {
        self.post(
            format!(
                "/api/agent-debug/sessions/{}/checkpoints",
                enc(session_id)
            ),
            json!({ "paths": paths, "label": label }),
        )
        .await
    }

    pub async fn rewind_checkpoint(&self, checkpoint_id: &str) -> Result<Value> {
        self.post(
            format!(
                "/api/agent-debug/checkpoints/{}:rewind",
                enc(checkpoint_id)
            ),
            json!({}),
        )
        .await
    }

    // -------------------- Run 状态 / 日志 / 指标 --------------------

    pub async fn get_run(&self, run_id: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/runs/{}", enc(run_id)))
            .await
    }

    pub async fn get_run_logs(&self, run_id: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/runs/{}/logs", enc(run_id)))
            .await
    }

    pub async fn get_run_metrics(&self, run_id: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/runs/{}/metrics", enc(run_id)))
            .await
    }

    // -------------------- Plan 管理 --------------------

    pub async fn get_plan(&self, plan_id: &str) -> Result<Value> {
        self.get(format!("/api/agent-debug/plans/{}", enc(plan_id)))
            .await
    }

    pub async fn patch_plan(&self, plan_id: &str, patch: Value) -> Result<Value> {
        self.patch(format!("/api/agent-debug/plans/{}", enc(plan_id)), patch)
            .await
    }

    pub async fn confirm_plan(&self, plan_id: &str) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/plans/{}:confirm", enc(plan_id)),
            json!({}),
        )
        .await
    }

    pub async fn replan(&self, plan_id: &str, user_input: &str) -> Result<Value> {
        self.post(
            format!("/api/agent-debug/plans/{}:replan", enc(plan_id)),
            json!({ "userInput": user_input }),
        )
        .await
    }

    // -------------------- Todo 管理 --------------------

    pub async fn add_todo(&self, payload: Value) -> Result<Value> {
        self.post("/api/agent-debug/todos", payload).await
    }

    pub async fn patch_todo(&self, todo_id: &str, patch: Value) -> Result<Value> {
        self.patch(format!("/api/agent-debug/todos/{}", enc(todo_id)), patch)
            .await
    }

    pub async fn batch_rerun_todos(&self, payload: Value) -> Result<Value> {
        self.post("/api/agent-debug/todos:batch-rerun", payload).await
    }

    pub fn subscribe_events(&self, req: SubscribeRequest) -> EventSubscription {
        let (tx, rx) = mpsc::channel(512);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let client = self.clone();
        tokio::spawn(async move {
            let _ = client.event_loop(req, tx, shutdown_rx).await;
        });
        EventSubscription {
            events: rx,
            shutdown: shutdown_tx,
        }
    }

    async fn event_loop(
        &self,
        req: SubscribeRequest,
        tx: mpsc::Sender<EventFrame>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut last_seq = req.from_seq.unwrap_or(0);
        let mut backoff_ms = 500u64;
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let url = self.ws_url("/ws/agent-debug");
            match connect_async(url.as_str()).await {
                Ok((stream, _)) => {
                    backoff_ms = 500;
                    let (mut write, mut read) = stream.split();
                    let subscribe = json!({
                        "action": "subscribe",
                        "sessionId": req.session_id,
                        "fromSeq": last_seq,
                        "channels": req.channels,
                        "token": req.static_token,
                    });
                    write
                        .send(Message::Text(subscribe.to_string()))
                        .await
                        .map_err(ApiError::WebSocket)?;
                    loop {
                        tokio::select! {
                            _ = shutdown.changed() => {
                                let _ = write.close().await;
                                return Ok(());
                            }
                            message = read.next() => {
                                match message {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Some(frame) = parse_ws_text(&text)? {
                                            if let EventFrame::Event(evt) = &frame {
                                                if let Some(seq) = evt.seq {
                                                    if seq > last_seq {
                                                        last_seq = seq;
                                                    }
                                                }
                                            }
                                            tx.send(frame).await.map_err(|_| ApiError::Send)?;
                                        }
                                    }
                                    Some(Ok(Message::Binary(bytes))) => {
                                        if let Ok(text) = String::from_utf8(bytes) {
                                            if let Some(frame) = parse_ws_text(&text)? {
                                                tx.send(frame).await.map_err(|_| ApiError::Send)?;
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) | None => break,
                                    Some(Ok(_)) => {}
                                    Some(Err(err)) => return Err(ApiError::WebSocket(err)),
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    let _ = tx.send(EventFrame::TransportError(err.to_string())).await;
                }
            }
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_millis(backoff_ms.min(10_000))) => {}
            }
            backoff_ms = ((backoff_ms as f64) * 1.7).round().min(10_000.0) as u64;
        }
    }

    pub async fn stream_events_sse(
        &self,
        session_id: &str,
        from_seq: u64,
        tx: mpsc::Sender<DebugEvent>,
    ) -> Result<()> {
        let path = format!(
            "/api/agent-debug/sessions/{}/events/stream?fromSeq={}",
            enc(session_id),
            from_seq
        );
        let url = format!("{}{}", self.base_url, path);
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.auth_token {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(AUTHORIZATION, value);
            }
        }
        let mut stream = self.client.get(url).headers(headers).send().await?.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(idx) = buffer.find("\n\n") {
                let frame = buffer[..idx].to_string();
                buffer = buffer[idx + 2..].to_string();
                if let Some(evt) = parse_sse_frame(&frame)? {
                    tx.send(evt).await.map_err(|_| ApiError::Send)?;
                }
            }
        }
        Ok(())
    }

    fn ws_url(&self, path: &str) -> String {
        let base = self
            .base_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        format!("{base}{path}")
    }
}

#[derive(Debug, Clone)]
pub struct SubscribeRequest {
    pub session_id: String,
    pub from_seq: Option<u64>,
    pub channels: Option<Vec<String>>,
    /// Backend WS only validates this static token when `AGENT_DEBUG_AUTH_TOKEN`
    /// is set. JWT auth is used for REST, not WS.
    pub static_token: Option<String>,
}

pub struct EventSubscription {
    pub events: mpsc::Receiver<EventFrame>,
    shutdown: watch::Sender<bool>,
}

impl EventSubscription {
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }
}

#[derive(Debug, Clone)]
pub enum EventFrame {
    Event(DebugEvent),
    Subscribed { session_id: String, latest_seq: u64 },
    ReplayGap(Value),
    TransportError(String),
}

fn parse_ws_text(text: &str) -> Result<Option<EventFrame>> {
    let value: Value = serde_json::from_str(text)?;
    let frame_type = value.get("type").and_then(Value::as_str).unwrap_or_default();
    match frame_type {
        "ws.subscribed" => Ok(Some(EventFrame::Subscribed {
            session_id: value
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            latest_seq: value.get("latestSeq").and_then(Value::as_u64).unwrap_or(0),
        })),
        "ws.replay.gap" => Ok(Some(EventFrame::ReplayGap(value))),
        "" => Ok(None),
        _ => Ok(Some(EventFrame::Event(serde_json::from_value(value)?))),
    }
}

fn parse_sse_frame(frame: &str) -> Result<Option<DebugEvent>> {
    if frame.trim().is_empty() || frame.trim_start().starts_with(':') {
        return Ok(None);
    }
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start());
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&data_lines.join("\n"))?))
}

fn backend_error_message(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    let envelope = value
        .get("error")
        .or_else(|| value.get("detail").and_then(|d| d.get("error")))
        .or_else(|| value.get("detail"))
        .unwrap_or(value);
    if let Some(s) = envelope.as_str() {
        return s.to_string();
    }
    envelope
        .get("message")
        .or_else(|| envelope.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("backend error")
        .to_string()
}

fn enc(input: &str) -> String {
    // Enough for path/query ids used by this backend without pulling a full URL
    // crate into the hot client crate.
    input
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enc_percent_encodes_spaces_and_slashes() {
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn parse_subscribed_frame() {
        let frame = parse_ws_text(r#"{"type":"ws.subscribed","sessionId":"s","latestSeq":7}"#)
            .unwrap()
            .unwrap();
        match frame {
            EventFrame::Subscribed {
                session_id,
                latest_seq,
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(latest_seq, 7);
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn parse_event_frame() {
        let frame =
            parse_ws_text(r#"{"seq":1,"type":"agent.token.stream.delta","payload":{"delta":"x"}}"#)
                .unwrap()
                .unwrap();
        match frame {
            EventFrame::Event(evt) => {
                assert_eq!(evt.event_type, "agent.token.stream.delta");
                assert_eq!(evt.payload_str("delta"), Some("x"));
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn parse_sse_data_frame() {
        let evt = parse_sse_frame(
            "id: 1\nevent: agent.token.stream.delta\ndata: {\"seq\":1,\"type\":\"agent.token.stream.delta\",\"payload\":{\"delta\":\"x\"}}\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(evt.seq, Some(1));
        assert_eq!(evt.payload_str("delta"), Some("x"));
    }
}
