//! GPUI desktop shell for the Moonlit Agent IDE.
//!
//! 1:1 replica of the legacy React frontend (`apps/agent-ide`): 36px titlebar,
//! five-column body (sessions 256 | 9 | chat 360 | 9 | main | 9 | inspector
//! 288), 26px statusbar, Claude cream palette with the amber accent. The view
//! layer lives in [`crate::ui`]; this module owns state and API plumbing.

use futures_util::StreamExt;
use gpui::{
    div, prelude::*, px, App, Application, Bounds, Context, Entity, FocusHandle, Focusable,
    SharedString, Window, WindowBounds, WindowOptions,
};
use moonlit_api::{EventFrame, MoonlitAgentApi, SubscribeRequest};
use moonlit_core::models::{DebugSession, DesignSnapshot};
use moonlit_core::store::keys;
use moonlit_core::ConfigStore;
use moonlit_uikit::{
    register_text_input_keybindings, TextInput, TextInputEvent, ToastKind, Tokens, FONT_SANS,
};

use crate::ui::pane_divider;
use crate::{AgentIdeState, ChatRole, ComposerMode, ProposalView, TabKind, WorkspaceNode, WorkspaceNodeKind};

/// Tokio runtime handle, set by `main` before the GPUI loop starts.
pub static RUNTIME: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();

gpui::actions!(
    agent_ide,
    [TogglePalette, NewSessionAction, ToggleBottomAction, CloseOverlays, SaveFileAction]
);

/// Cross-thread message from background API tasks to the UI.
pub(crate) enum AppMsg {
    Connected(bool),
    Snapshot(Box<DesignSnapshot>),
    Event(moonlit_core::models::DebugEvent),
    SessionCreated(DebugSession),
    Status(String),
    Provider { mode: String, model: String },
    Tree { branch: Option<String>, nodes: Vec<WorkspaceNode> },
    FileLoaded { path: String, content: String },
    Models(Vec<moonlit_core::models::AgentModelOption>),
    Readme(Option<String>),
    TreeChildren { parent: String, nodes: Vec<WorkspaceNode> },
    LoggedIn(Box<moonlit_core::models::AuthResponse>),
    LoginFailed(String),
    // ---- settings ----
    /// `auth/me` style raw profile payload (`{user: {...}}` or the user itself).
    Profile(Option<Box<serde_json::Value>>),
    /// `update_profile` finished: refreshed profile or error.
    ProfileSaved {
        result: Result<Box<serde_json::Value>, String>,
        ok_toast: String,
        close_acct: bool,
    },
    /// Generic settings toast (ok / error tone).
    SettingsToast(String, bool),
    Channels { providers: Vec<serde_json::Value>, channels: Vec<serde_json::Value> },
    ChannelsList(Vec<serde_json::Value>),
    /// Channel form saved: refreshed list (clears the draft) or error string.
    ChannelSaved(Result<Vec<serde_json::Value>, String>),
    /// `channels:fetch-models` result for the open draft.
    ChannelModels(Result<Vec<(String, String, bool)>, String>),
    Tavily(Box<TavilyDraft>),
    TavilySaved(Result<Box<TavilyDraft>, String>),
    Skills(Vec<serde_json::Value>),
    SkillContent { name: String, content: String },
    // ---- live workbench refetch results ----
    /// Plan bundle refetched after a `plan.*` event.
    PlanBundle(Option<serde_json::Value>),
    /// Proposal list refetched after an `agent.code_edit.*` event.
    ProposalsList(Option<Vec<moonlit_core::models::Proposal>>),
    /// Swarm state refetched after a `swarm.*` event.
    SwarmState(Option<serde_json::Value>),
    /// Aggregate metrics refetched after a completion event.
    MetricsUpdate(Option<moonlit_core::models::DesignMetrics>),
    /// Run logs/metrics fetched on demand for the bottom panel.
    RunLogs(Vec<String>),
    /// Permission mode synced from the backend.
    PermissionMode(String),
    /// Checkpoint list for the active session.
    Checkpoints(Vec<serde_json::Value>),
}

/// Re-fetch the proposal list for a session and push it as `ProposalsList`.
async fn refetch_proposals(
    api: &MoonlitAgentApi,
    session: Option<&str>,
    tx: &futures_channel::mpsc::UnboundedSender<AppMsg>,
) {
    let Some(session) = session else { return };
    let list = api.list_proposals(session).await.ok().and_then(|v| {
        v.get("proposals")
            .cloned()
            .and_then(|p| serde_json::from_value::<Vec<moonlit_core::models::Proposal>>(p).ok())
    });
    let _ = tx.unbounded_send(AppMsg::ProposalsList(list));
}

/// Map backend proposals into diff views + aligned proposal-id list.
fn proposals_to_views(
    proposals: &[moonlit_core::models::Proposal],
) -> (Vec<ProposalView>, Vec<String>) {
    let mut views = Vec::new();
    let mut pids = Vec::new();
    for p in proposals {
        for c in &p.changes {
            views.push(ProposalView::new(
                c.change_id.clone().unwrap_or_else(|| format!("{}:{}", p.id, c.path)),
                c.path.clone(),
                p.summary.clone(),
                &c.original_content,
                &c.proposed_content,
            ));
            pids.push(p.id.clone());
        }
    }
    (views, pids)
}

fn parse_session_response(value: serde_json::Value) -> Result<DebugSession, String> {
    let session_value = value.get("session").cloned().unwrap_or(value);
    let session = serde_json::from_value::<DebugSession>(session_value)
        .map_err(|err| format!("会话响应解析失败: {err}"))?;
    if session.id.is_empty() {
        return Err("会话响应缺少 session.id".into());
    }
    Ok(session)
}

/// Draft for the channel form (模型页), mirroring `channelToDraft`.
/// Text fields (名称 / Base URL / API Key / model id+name) live in the
/// settings `TextInput` pool; this carries the non-text state.
pub(crate) struct ChannelDraft {
    pub id: Option<String>,
    pub provider: String,
    pub api_key_set: bool,
    pub enabled: bool,
    /// Enabled flag per model row (texts in the input pool, `ch:m{i}:id/name`).
    pub model_enabled: Vec<bool>,
    pub fetching_models: bool,
}

/// Tavily 搜索配置 draft, mirroring `SearchApiSection`.
#[derive(Clone)]
pub(crate) struct TavilyDraft {
    pub enabled: bool,
    pub api_key_set: bool,
    pub topic: String,
    pub search_depth: String,
    pub time_range: String,
    pub extract_depth: String,
}

/// One field of a CRUD modal (规则/技能/子Agent/命令/MCP sections).
pub(crate) struct CrudField {
    pub key: &'static str,
    pub label: &'static str,
    pub required: bool,
    pub placeholder: &'static str,
    pub kind: CrudFieldKind,
}

pub(crate) enum CrudFieldKind {
    Text,
    Textarea,
    Select(&'static [(&'static str, &'static str)]),
}

/// Open CRUD modal state (storage-backed local lists, legacy `CrudSection`).
pub(crate) struct CrudState {
    pub storage_key: String,
    pub title: &'static str,
    pub add_label: &'static str,
    pub fields: &'static [CrudField],
    /// `None` = creating; `Some(id)` = editing.
    pub editing_id: Option<String>,
    /// Current values of select-type fields.
    pub selects: std::collections::HashMap<&'static str, String>,
}

/// Run the Agent IDE GPUI application on the current (main) thread.
pub fn run(api: MoonlitAgentApi) {
    Application::new()
        .with_assets(crate::ui::icons::Assets)
        .run(move |cx: &mut App| {
            register_text_input_keybindings(cx);
            // Global shortcuts matching the legacy frontend.
            cx.bind_keys([
                gpui::KeyBinding::new("ctrl-k", TogglePalette, None),
                gpui::KeyBinding::new("ctrl-shift-n", NewSessionAction, None),
                gpui::KeyBinding::new("ctrl-j", ToggleBottomAction, None),
                gpui::KeyBinding::new("ctrl-s", SaveFileAction, None),
                gpui::KeyBinding::new("escape", CloseOverlays, None),
            ]);
            let bounds = Bounds::centered(None, gpui::size(px(1440.0), px(900.0)), cx);
            let api = api.clone();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| AgentIdeApp::new(api.clone(), window, cx)),
            )
            .unwrap();
            cx.activate(true);

            if std::env::var("MOONLIT_SMOKE").is_ok() {
                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    let _ = cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
}

pub struct AgentIdeApp {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) api: MoonlitAgentApi,
    pub(crate) store: Option<ConfigStore>,
    pub(crate) state: AgentIdeState,
    pub(crate) t: Tokens,
    pub(crate) composer: Entity<TextInput>,
    /// User message currently in inline edit mode (ChatMessage id).
    pub(crate) editing_msg: Option<String>,
    /// Inline editor for "edit a historical user message & resend".
    pub(crate) edit_input: Entity<TextInput>,
    pub(crate) sidebar_search: Entity<TextInput>,
    pub(crate) ws_search: Entity<TextInput>,
    pub(crate) mode: ComposerMode,
    pub(crate) status: SharedString,
    pub(crate) connected: bool,
    /// Active workbench tab: plan / todo / diff / swarm / readme / file:<path>.
    pub(crate) active_tab: String,
    pub(crate) sessions_collapsed: bool,
    pub(crate) chat_collapsed: bool,
    pub(crate) inspector_collapsed: bool,
    /// Resizable pane widths (sessions, chat, inspector) and bottom height,
    /// persisted like the legacy `moonlit:paneSizes` / `moonlit:bottomH`.
    pub(crate) pane_w: (f32, f32, f32),
    pub(crate) bottom_h: f32,
    /// Which divider is being dragged: "sessions"|"chat"|"inspector"|"bottom".
    pub(crate) dragging: Option<&'static str>,
    pub(crate) settings_open: bool,
    pub(crate) palette_open: bool,
    pub(crate) palette_input: Entity<TextInput>,
    pub(crate) palette_index: usize,
    /// Dark theme toggle (legacy View → Theme).
    pub(crate) dark: bool,
    /// Plan inspector view: "tree" | "dag" | "timeline" | "diff".
    pub(crate) plan_view: &'static str,
    /// Diff page: index of the proposal currently shown.
    pub(crate) diff_index: usize,
    /// Proposal ids aligned with `state.workbench.proposals`.
    pub(crate) proposal_pids: Vec<String>,
    /// Workspace README fetched through the REST gateway.
    pub(crate) readme: Option<String>,
    pub(crate) show_all_home: bool,
    /// Per-block expand override (key `msg_id:block_idx`); default follows
    /// the legacy rule "expanded = isLast".
    pub(crate) expanded_blocks: std::collections::HashMap<String, bool>,
    /// Long assistant texts expanded past the 140px collapse.
    pub(crate) expanded_msgs: std::collections::HashSet<String>,
    /// Block id of the subagent shown in the half-cover overlay.
    pub(crate) subagent_overlay: Option<String>,
    pub(crate) add_menu_open: bool,
    pub(crate) model_menu_open: bool,
    pub(crate) models: Vec<moonlit_core::models::AgentModelOption>,
    pub(crate) selected_model: Option<String>,
    pub(crate) todo_strip_open: bool,
    pub(crate) renaming_session: Option<String>,
    pub(crate) rename_input: Entity<TextInput>,
    pub(crate) ws_menu_open: bool,
    pub(crate) user_menu_open: bool,
    pub(crate) profile_open: bool,
    pub(crate) notifs_open: bool,
    pub(crate) notif_tab: &'static str,
    pub(crate) about_open: bool,
    pub(crate) shortcuts_open: bool,
    pub(crate) context_drawer_open: bool,
    pub(crate) ctx_filter: &'static str,
    pub(crate) ctx_selected: std::collections::HashSet<String>,
    /// Which titlebar menu dropdown is open ("file"/"edit"/"view"/"help").
    pub(crate) menubar_open: Option<&'static str>,
    /// Chat-head dropdown: "replay" | "more".
    pub(crate) chathead_menu: Option<&'static str>,
    /// Provider status from `GET /provider-status`: ("live"|"mock"|"offline", model id).
    pub(crate) provider: Option<(String, String)>,
    pub(crate) metrics: moonlit_core::models::DesignMetrics,
    pub(crate) git_branch: Option<String>,
    pub(crate) logs: Vec<String>,
    /// Recent raw events for the Agent Logs panel (capped at 300).
    pub(crate) events: Vec<moonlit_core::models::DebugEvent>,
    /// Expanded directories in the inspector tree (relPath keys).
    pub(crate) expanded_dirs: std::collections::HashSet<String>,
    pub(crate) term_input: Entity<TextInput>,
    pub(crate) term_history: Vec<String>,
    /// Auth gate (legacy `AuthGate` / LoginScreen).
    pub(crate) authed: bool,
    pub(crate) auth_email: Entity<TextInput>,
    pub(crate) auth_password: Entity<TextInput>,
    pub(crate) login_error: Option<String>,
    pub(crate) login_busy: bool,
    pub(crate) auth_user: Option<String>,
    // ---- settings (设置界面) ----
    /// Open `set-select` dropdown id — mutually exclusive popups.
    pub(crate) settings_menu: Option<String>,
    /// Slider track geometry `(origin_x, width)` recorded at paint time.
    pub(crate) slider_geom: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<&'static str, (f32, f32)>>>,
    /// On-demand `TextInput` pool for settings fields, keyed `acct:name` etc.
    pub(crate) settings_inputs: std::collections::HashMap<String, Entity<TextInput>>,
    /// 账户卡 expanded / saving (通用页).
    pub(crate) acct_open: bool,
    pub(crate) acct_saving: bool,
    /// Raw profile payload from `auth/me` / `update_profile`.
    pub(crate) auth_profile: Option<serde_json::Value>,
    pub(crate) profile_loading: bool,
    /// 模型页: channels + provider types (`/channels`, `/provider-types`).
    pub(crate) channels: Vec<serde_json::Value>,
    pub(crate) provider_types: Vec<serde_json::Value>,
    pub(crate) channels_loaded: bool,
    pub(crate) channels_loading: bool,
    pub(crate) channel_draft: Option<ChannelDraft>,
    pub(crate) channel_saving: bool,
    /// Tavily 搜索配置 (None = not loaded yet).
    pub(crate) tavily: Option<TavilyDraft>,
    pub(crate) tavily_loading: bool,
    pub(crate) tavily_saving: bool,
    /// 已发现技能 (None = not loaded yet).
    pub(crate) skills: Option<Vec<serde_json::Value>>,
    pub(crate) skills_loading: bool,
    pub(crate) skill_open: Option<String>,
    pub(crate) skill_previews: std::collections::HashMap<String, String>,
    /// Open CRUD modal (规则/工具页).
    pub(crate) crud_modal: Option<CrudState>,
    pub(crate) tx: futures_channel::mpsc::UnboundedSender<AppMsg>,
    pub(crate) event_task: Option<tokio::task::JoinHandle<()>>,
    // ---- live workbench: active plan + refetch dedup guards ----
    /// Active plan id (from snapshot / `plan.created`), for `plan.*` refetch.
    pub(crate) active_plan_id: Option<String>,
    pub(crate) plan_refetch_inflight: bool,
    pub(crate) proposals_refetch_inflight: bool,
    pub(crate) swarm_refetch_inflight: bool,
    pub(crate) metrics_refetch_inflight: bool,
    /// Backend permission mode for the active session ("bypass"/"plan"/"auto").
    pub(crate) permission_mode: String,
    /// Checkpoints for the active session (raw payloads).
    pub(crate) checkpoints: Vec<serde_json::Value>,
}

impl AgentIdeApp {
    fn new(api: MoonlitAgentApi, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "Add more optional details…");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let edit_input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "编辑消息并重新发送…");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let sidebar_search = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "搜索会话…");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let ws_search = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "搜索");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let rename_input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let palette_input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "输入命令…");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let term_input = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let auth_email = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "you@example.com");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let auth_password = cx.new(|cx| {
            let mut input = TextInput::new(cx, "", "••••••");
            input.set_accent(
                Tokens::claude_light().accent,
                Tokens::claude_light().bg_selection,
            );
            input
        });
        let store = ConfigStore::open_for_app("MoonlitAgentIde").ok();

        // Enter or Ctrl+Enter sends depending on the setting (legacy parity);
        // Changed repaints the send/mic toggle.
        cx.subscribe(&composer, |this: &mut Self, _input, event, cx| match event {
            TextInputEvent::Submit(_) => {
                if !this.state.settings.submit_with_ctrl_enter {
                    this.send(cx);
                }
            }
            TextInputEvent::SubmitCtrl(_) => {
                if this.state.settings.submit_with_ctrl_enter {
                    this.send(cx);
                }
            }
            TextInputEvent::Changed(_) => cx.notify(),
        })
        .detach();
        // Inline message editor: Enter / Ctrl+Enter both resend the edit.
        cx.subscribe(&edit_input, |this: &mut Self, _input, event, cx| match event {
            TextInputEvent::Submit(_) | TextInputEvent::SubmitCtrl(_) => {
                this.resend_edited(cx);
            }
            TextInputEvent::Changed(_) => cx.notify(),
        })
        .detach();
        cx.subscribe(&sidebar_search, |_this: &mut Self, _input, event, cx| {
            if let TextInputEvent::Changed(_) = event {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&ws_search, |_this: &mut Self, _input, event, cx| {
            if let TextInputEvent::Changed(_) = event {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&rename_input, |this: &mut Self, _input, event, cx| {
            if let TextInputEvent::Submit(text) = event {
                this.commit_rename(text.clone(), cx);
            }
        })
        .detach();
        cx.subscribe(&palette_input, |this: &mut Self, _input, event, cx| match event {
            TextInputEvent::Changed(_) => {
                this.palette_index = 0;
                cx.notify();
            }
            TextInputEvent::Submit(_) | TextInputEvent::SubmitCtrl(_) => {
                this.run_palette_selection(cx);
            }
        })
        .detach();
        // Demo terminal: echo commands locally (legacy fake shell).
        cx.subscribe(&term_input, |this: &mut Self, input, event, cx| {
            if let TextInputEvent::Submit(text) = event {
                let cmd = text.trim().to_string();
                input.update(cx, |i, cx| i.set_text("", cx));
                if cmd.is_empty() {
                    return;
                }
                if cmd == "clear" {
                    this.term_history.clear();
                } else {
                    this.term_history.push(format!("workspace ❯ {cmd}"));
                    this.term_history.push(format!("'{cmd}'：演示终端，仅回显。"));
                }
                cx.notify();
            }
        })
        .detach();

        let (tx, mut rx) = futures_channel::mpsc::unbounded::<AppMsg>();
        cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                if this
                    .update(cx, |app, cx| {
                        app.handle_msg(msg, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let mut state = AgentIdeState::default();
        if let Some(store) = &store {
            let root = store.get_string_or(keys::WORKSPACE_ROOT, "");
            if !root.is_empty() {
                state.workbench.workspace_tree = build_tree(std::path::Path::new(&root), 0);
            }
        }

        let mut app = Self {
            focus_handle: cx.focus_handle(),
            api,
            store,
            state,
            t: Tokens::claude_light(),
            composer,
            editing_msg: None,
            edit_input,
            sidebar_search,
            ws_search,
            mode: ComposerMode::Build,
            status: "正在连接后端…".into(),
            connected: false,
            // Legacy default: no tabs open, workbench shows the empty hero.
            active_tab: String::new(),
            sessions_collapsed: false,
            chat_collapsed: false,
            inspector_collapsed: false,
            pane_w: (256., 360., 288.),
            bottom_h: 260.,
            dragging: None,
            settings_open: false,
            palette_open: false,
            palette_input,
            palette_index: 0,
            dark: false,
            plan_view: "tree",
            diff_index: 0,
            proposal_pids: Vec::new(),
            readme: None,
            show_all_home: false,
            expanded_blocks: Default::default(),
            expanded_msgs: Default::default(),
            subagent_overlay: None,
            add_menu_open: false,
            model_menu_open: false,
            models: Vec::new(),
            selected_model: None,
            todo_strip_open: true,
            renaming_session: None,
            rename_input,
            ws_menu_open: false,
            user_menu_open: false,
            profile_open: false,
            notifs_open: false,
            notif_tab: "all",
            about_open: false,
            shortcuts_open: false,
            context_drawer_open: false,
            ctx_filter: "all",
            ctx_selected: Default::default(),
            menubar_open: None,
            chathead_menu: None,
            provider: None,
            metrics: Default::default(),
            git_branch: None,
            logs: Vec::new(),
            events: Vec::new(),
            expanded_dirs: Default::default(),
            term_input,
            term_history: Vec::new(),
            authed: false,
            auth_email,
            auth_password,
            login_error: None,
            login_busy: false,
            auth_user: None,
            settings_menu: None,
            slider_geom: Default::default(),
            settings_inputs: Default::default(),
            acct_open: false,
            acct_saving: false,
            auth_profile: None,
            profile_loading: false,
            channels: Vec::new(),
            provider_types: Vec::new(),
            channels_loaded: false,
            channels_loading: false,
            channel_draft: None,
            channel_saving: false,
            tavily: None,
            tavily_loading: false,
            tavily_saving: false,
            skills: None,
            skills_loading: false,
            skill_open: None,
            skill_previews: Default::default(),
            crud_modal: None,
            tx,
            event_task: None,
            active_plan_id: None,
            plan_refetch_inflight: false,
            proposals_refetch_inflight: false,
            swarm_refetch_inflight: false,
            metrics_refetch_inflight: false,
            permission_mode: "auto".into(),
            checkpoints: Vec::new(),
        };
        // Restore persisted pane sizes (legacy `moonlit:paneSizes` / bottomH).
        if let Some(store) = &app.store {
            if let Some(sizes) = store.get_string(keys::PANE_SIZES) {
                if let Ok(v) = serde_json::from_str::<Vec<f32>>(&sizes) {
                    if v.len() == 3 {
                        app.pane_w = (v[0], v[1], v[2]);
                    }
                }
            }
            if let Some(h) = store.get_string(keys::BOTTOM_HEIGHT) {
                if let Ok(h) = h.parse::<f32>() {
                    app.bottom_h = h.clamp(120., 520.);
                }
            }
            // Settings persistence (legacy localStorage keys).
            if let Some(page) = store.get_string("moonlit:settingsPage") {
                app.state.settings.page = crate::SettingsPage::from_id(page.trim_matches('"'));
            }
            if let Some(v) = store.get_bool(keys::AUTO_APPROVE) {
                app.state.settings.auto_approve = v;
            }
            if let Some(v) = store.get_bool("moonlit:s:submitCtrl") {
                app.state.settings.submit_with_ctrl_enter = v;
            }
            if store.get_string_or("moonlit:settings:theme", "auto") == "dark" {
                app.dark = true;
                app.t = Tokens::claude_dark();
            }
        }
        if app.dark {
            let (accent, selection) = (app.t.accent, app.t.bg_selection);
            for input in [
                &app.composer,
                &app.edit_input,
                &app.sidebar_search,
                &app.ws_search,
                &app.rename_input,
                &app.palette_input,
                &app.term_input,
                &app.auth_email,
                &app.auth_password,
            ] {
                input.update(cx, |i, _| i.set_accent(accent, selection));
            }
        }
        // Stored token → skip the login gate and authenticate the client.
        let stored_token = app
            .store
            .as_ref()
            .and_then(|s| s.get_string(keys::AUTH_TOKEN))
            .filter(|t| !t.is_empty());
        if let Some(token) = stored_token {
            app.api = app.api.clone().with_auth_token(token);
            app.authed = true;
            app.bootstrap();
        } else if std::env::var("MOONLIT_SKIP_LOGIN").is_ok() || std::env::var("MOONLIT_SMOKE").is_ok() {
            // Dev bypass, mirroring the legacy `?skipLogin=1` (`makeDebugUser`).
            app.authed = true;
            app.auth_user = Some("本地调试".into());
            app.auth_profile = Some(serde_json::json!({
                "id": "debug-skip-user",
                "email": "debug@local.test",
                "displayName": "本地调试",
                "workspace": "debug-workspace",
                "avatar": "调",
                "plan": { "tier": "debug", "label": "Debug", "priceLabel": "Local" },
                "debugBypass": true,
            }));
            app.bootstrap();
        }
        // Smoke/screenshot hook: jump straight into the settings fullscreen.
        if std::env::var("MOONLIT_OPEN_SETTINGS").is_ok() {
            app.settings_open = true;
        }
        if let Ok(page) = std::env::var("MOONLIT_SETTINGS_PAGE") {
            app.state.settings.page = crate::SettingsPage::from_id(&page);
        }
        app
    }

    // ---- background plumbing ------------------------------------------------

    fn bootstrap(&mut self) {
        let Some(handle) = RUNTIME.get() else {
            self.status = "无 Tokio 运行时".into();
            return;
        };
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let ok = api.health().await.is_ok();
            let _ = tx.unbounded_send(AppMsg::Connected(ok));
            match api.snapshot(None).await {
                Ok(snap) => {
                    let _ = tx.unbounded_send(AppMsg::Snapshot(Box::new(snap)));
                }
                Err(err) => {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("快照失败: {err}")));
                }
            }
            if let Ok(status) = api.provider_status().await {
                let mode = status.get("mode").and_then(|v| v.as_str()).unwrap_or("offline").to_string();
                let model = status
                    .get("openaiCompatible")
                    .and_then(|v| v.get("model"))
                    .and_then(|v| v.as_str())
                    .or_else(|| status.get("defaultModelId").and_then(|v| v.as_str()))
                    .unwrap_or("default")
                    .to_string();
                let _ = tx.unbounded_send(AppMsg::Provider { mode, model });
            }
            if let Ok(tree) = api.workspace_tree("", false).await {
                let branch = tree.get("gitBranch").and_then(|v| v.as_str()).map(str::to_string);
                let nodes = parse_backend_tree(&tree);
                let _ = tx.unbounded_send(AppMsg::Tree { branch, nodes });
            }
            let readme = api
                .read_workspace_file("README.md")
                .await
                .ok()
                .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(str::to_string));
            let _ = tx.unbounded_send(AppMsg::Readme(readme));
            if let Ok(models) = api.list_models().await {
                let list = models
                    .get("models")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                serde_json::from_value::<moonlit_core::models::AgentModelOption>(m.clone()).ok()
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let _ = tx.unbounded_send(AppMsg::Models(list));
            }
            // Profile feeds the settings nav foot / 通用 / 套餐页.
            let profile = api.me().await.ok();
            let _ = tx.unbounded_send(AppMsg::Profile(profile.map(Box::new)));
        });
    }

    fn handle_msg(&mut self, msg: AppMsg, cx: &mut Context<Self>) {
        match msg {
            AppMsg::Connected(ok) => {
                self.connected = ok;
                self.status = if ok {
                    "Agent Debug online".into()
                } else {
                    "Agent Debug offline".into()
                };
            }
            AppMsg::Provider { mode, model } => {
                self.provider = Some((mode, model));
            }
            AppMsg::Tree { branch, nodes } => {
                self.git_branch = branch;
                if !nodes.is_empty() {
                    self.state.workbench.workspace_tree = nodes;
                }
            }
            AppMsg::FileLoaded { path, content } => {
                self.state.workbench.open_file(path.clone(), content);
                self.active_tab = format!("file:{path}");
            }
            AppMsg::Models(models) => {
                self.models = models;
            }
            AppMsg::Readme(content) => {
                self.readme = content;
            }
            AppMsg::LoggedIn(auth) => {
                self.login_busy = false;
                self.login_error = None;
                self.auth_user = Some(if auth.user.display_name.is_empty() {
                    auth.user.email.clone()
                } else {
                    auth.user.display_name.clone()
                });
                if let Some(store) = &self.store {
                    let _ = store.set_string(keys::AUTH_TOKEN, &auth.token);
                }
                self.api = self.api.clone().with_auth_token(auth.token.clone());
                self.authed = true;
                self.bootstrap();
            }
            AppMsg::LoginFailed(msg) => {
                self.login_busy = false;
                self.login_error = Some(msg);
            }
            AppMsg::TreeChildren { parent, nodes } => {
                fn insert(tree: &mut [WorkspaceNode], parent: &str, nodes: &[WorkspaceNode]) -> bool {
                    for node in tree.iter_mut() {
                        if node.path == parent {
                            node.children = nodes.to_vec();
                            return true;
                        }
                        if insert(&mut node.children, parent, nodes) {
                            return true;
                        }
                    }
                    false
                }
                insert(&mut self.state.workbench.workspace_tree, &parent, &nodes);
            }
            AppMsg::Snapshot(snap) => {
                let active = snap.active_session.as_ref().map(|s| s.id.clone());
                self.active_plan_id = snap
                    .active_session
                    .as_ref()
                    .and_then(|s| s.active_plan_id.clone())
                    .or_else(|| {
                        snap.plan_bundle
                            .as_ref()
                            .and_then(|b| b.get("plan").and_then(|p| p.get("id")))
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                self.metrics = snap.metrics.clone();
                self.state.workbench.plan_bundle = snap.plan_bundle.clone();
                self.state.workbench.swarm = snap.swarm.clone();
                let (views, pids) = proposals_to_views(&snap.proposals);
                self.state.workbench.proposals = views;
                self.proposal_pids = pids;
                self.diff_index = 0;
                let skip = snap.events.len().saturating_sub(300);
                self.events = snap.events.iter().skip(skip).cloned().collect();
                self.state.hydrate_snapshot(*snap);
                self.status = format!("已加载 {} 个会话", self.state.sessions.len()).into();
                if let Some(id) = active {
                    self.subscribe(id);
                }
            }
            AppMsg::Event(evt) => {
                self.events.push(evt.clone());
                if self.events.len() > 300 {
                    self.events.remove(0);
                }
                let evt_type = evt.event_type.clone();
                // Track the active plan id from plan lifecycle events so the
                // `plan.*` refetch can target the right plan.
                if evt_type == "plan.created" || evt_type == "plan.replanned" {
                    if let Some(id) = evt.payload.get("id").and_then(|v| v.as_str()) {
                        self.active_plan_id = Some(id.to_string());
                    }
                }
                self.state.apply_event(evt);
                self.refetch_after_event(&evt_type);
            }
            AppMsg::SessionCreated(session) => {
                let id = session.id.clone();
                if !self.state.sessions.iter().any(|s| s.id == id) {
                    self.state.sessions.insert(0, session);
                }
                self.state.active_session_id = Some(id.clone());
                self.state.chat.messages.clear();
                self.toast("已新建会话", ToastKind::Success, cx);
                if let Some(store) = &self.store {
                    let _ = store.set_string(keys::SELECTED_SESSION, &id);
                }
                self.subscribe(id);
            }
            AppMsg::Status(s) => {
                self.logs.push(s.clone());
                if self.logs.len() > 500 {
                    self.logs.remove(0);
                }
                self.status = s.into();
            }
            // ---- settings ----
            AppMsg::Profile(value) => {
                self.profile_loading = false;
                if let Some(value) = value {
                    let user = value.get("user").cloned().unwrap_or(*value);
                    if let Some(name) = user.get("displayName").and_then(|v| v.as_str()) {
                        if !name.is_empty() {
                            self.auth_user = Some(name.to_string());
                        }
                    }
                    self.auth_profile = Some(user);
                }
            }
            AppMsg::ProfileSaved { result, ok_toast, close_acct } => {
                self.acct_saving = false;
                match result {
                    Ok(value) => {
                        let user = value.get("user").cloned().unwrap_or(*value);
                        if let Some(name) = user.get("displayName").and_then(|v| v.as_str()) {
                            if !name.is_empty() {
                                self.auth_user = Some(name.to_string());
                            }
                        }
                        self.auth_profile = Some(user);
                        if close_acct {
                            self.acct_open = false;
                        }
                        self.toast(ok_toast, ToastKind::Success, cx);
                    }
                    Err(err) => self.toast(format!("保存失败：{err}"), ToastKind::Error, cx),
                }
            }
            AppMsg::SettingsToast(msg, ok) => {
                self.toast(msg, if ok { ToastKind::Success } else { ToastKind::Error }, cx);
            }
            AppMsg::Channels { providers, channels } => {
                self.channels_loading = false;
                self.channels_loaded = true;
                self.provider_types = providers;
                self.channels = channels;
            }
            AppMsg::ChannelsList(channels) => {
                self.channels_loaded = true;
                self.channels = channels;
            }
            AppMsg::ChannelSaved(result) => {
                self.channel_saving = false;
                match result {
                    Ok(channels) => {
                        self.channels = channels;
                        self.channel_draft = None;
                        self.toast("渠道已保存", ToastKind::Success, cx);
                    }
                    Err(err) => self.toast(format!("保存失败：{err}"), ToastKind::Error, cx),
                }
            }
            AppMsg::ChannelModels(result) => {
                if let Some(draft) = &mut self.channel_draft {
                    draft.fetching_models = false;
                }
                match result {
                    Ok(models) => {
                        let count = models.len();
                        self.set_channel_model_rows(models, cx);
                        self.toast(format!("已获取 {count} 个模型"), ToastKind::Success, cx);
                    }
                    Err(err) => self.toast(format!("获取模型失败：{err}"), ToastKind::Error, cx),
                }
            }
            AppMsg::Tavily(draft) => {
                self.tavily_loading = false;
                self.tavily = Some(*draft);
            }
            AppMsg::TavilySaved(result) => {
                self.tavily_saving = false;
                match result {
                    Ok(draft) => {
                        self.tavily = Some(*draft);
                        if let Some(input) = self.settings_inputs.get("tavily:key") {
                            input.update(cx, |i, cx| i.set_text("", cx));
                        }
                        self.toast("Tavily 搜索配置已保存", ToastKind::Success, cx);
                    }
                    Err(err) => self.toast(format!("保存失败：{err}"), ToastKind::Error, cx),
                }
            }
            AppMsg::Skills(items) => {
                self.skills_loading = false;
                self.skills = Some(items);
            }
            AppMsg::SkillContent { name, content } => {
                self.skill_previews.insert(name, content);
            }
            AppMsg::PlanBundle(bundle) => {
                self.plan_refetch_inflight = false;
                if let Some(b) = bundle {
                    if let Some(id) = b.get("plan").and_then(|p| p.get("id")).and_then(|v| v.as_str()) {
                        self.active_plan_id = Some(id.to_string());
                    }
                    self.state.workbench.plan_bundle = Some(b);
                }
            }
            AppMsg::ProposalsList(list) => {
                self.proposals_refetch_inflight = false;
                if let Some(list) = list {
                    let (views, pids) = proposals_to_views(&list);
                    self.state.workbench.proposals = views;
                    self.proposal_pids = pids;
                    if self.diff_index >= self.proposal_pids.len() {
                        self.diff_index = 0;
                    }
                }
            }
            AppMsg::SwarmState(state) => {
                self.swarm_refetch_inflight = false;
                if let Some(s) = state {
                    self.state.workbench.swarm = Some(s.get("swarm").cloned().unwrap_or(s));
                }
            }
            AppMsg::MetricsUpdate(metrics) => {
                self.metrics_refetch_inflight = false;
                if let Some(m) = metrics {
                    self.metrics = m;
                }
            }
            AppMsg::RunLogs(lines) => {
                for line in lines {
                    self.logs.push(line);
                }
                if self.logs.len() > 500 {
                    let skip = self.logs.len() - 500;
                    self.logs.drain(0..skip);
                }
            }
            AppMsg::PermissionMode(mode) => {
                self.permission_mode = mode;
            }
            AppMsg::Checkpoints(items) => {
                self.checkpoints = items;
            }
        }
    }

    /// After a structural WS event, refetch the affected workbench slice with a
    /// per-category in-flight guard so bursts of node/tool events collapse into
    /// a single request. Chat stays event-driven and is never re-hydrated here.
    fn refetch_after_event(&mut self, evt_type: &str) {
        let Some(handle) = RUNTIME.get() else { return };
        let Some(session) = self.state.active_session_id.clone() else { return };

        if evt_type.starts_with("plan.") && !self.plan_refetch_inflight {
            if let Some(plan_id) = self.active_plan_id.clone() {
                self.plan_refetch_inflight = true;
                let api = self.api.clone();
                let tx = self.tx.clone();
                handle.spawn(async move {
                    let bundle = api.get_plan(&plan_id).await.ok();
                    let _ = tx.unbounded_send(AppMsg::PlanBundle(bundle));
                });
            }
        }

        if evt_type.starts_with("agent.code_edit.") && !self.proposals_refetch_inflight {
            self.proposals_refetch_inflight = true;
            let api = self.api.clone();
            let tx = self.tx.clone();
            let s = session.clone();
            handle.spawn(async move {
                let list = api.list_proposals(&s).await.ok().and_then(|v| {
                    v.get("proposals")
                        .cloned()
                        .and_then(|p| serde_json::from_value::<Vec<moonlit_core::models::Proposal>>(p).ok())
                });
                let _ = tx.unbounded_send(AppMsg::ProposalsList(list));
            });
        }

        if evt_type.starts_with("swarm.") && !self.swarm_refetch_inflight {
            self.swarm_refetch_inflight = true;
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                let state = api.passthrough_get("/api/agent-debug/swarm/state").await.ok();
                let _ = tx.unbounded_send(AppMsg::SwarmState(state));
            });
        }

        if matches!(evt_type, "agent.completed" | "plan.node.completed")
            && !self.metrics_refetch_inflight
        {
            self.metrics_refetch_inflight = true;
            let api = self.api.clone();
            let tx = self.tx.clone();
            let s = session.clone();
            handle.spawn(async move {
                let metrics = api.snapshot(Some(&s)).await.ok().map(|snap| snap.metrics);
                let _ = tx.unbounded_send(AppMsg::MetricsUpdate(metrics));
            });
        }
    }

    pub(crate) fn subscribe(&mut self, session_id: String) {
        if let Some(h) = self.event_task.take() {
            h.abort();
        }
        let Some(handle) = RUNTIME.get() else { return };
        let api = self.api.clone();
        let tx = self.tx.clone();
        let task = handle.spawn(async move {
            let mut sub = api.subscribe_events(SubscribeRequest {
                session_id,
                from_seq: None,
                channels: None,
                static_token: None,
            });
            while let Some(frame) = sub.events.recv().await {
                match frame {
                    EventFrame::Event(evt) => {
                        let _ = tx.unbounded_send(AppMsg::Event(evt));
                    }
                    EventFrame::TransportError(e) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("事件流中断: {e}")));
                    }
                    _ => {}
                }
            }
        });
        self.event_task = Some(task);
    }

    // ---- toasts ---------------------------------------------------------------

    pub(crate) fn toast(&mut self, title: impl Into<String>, kind: ToastKind, cx: &mut Context<Self>) {
        let id = self.state.toasts.push(title, None, kind);
        // 2.8s auto-dismiss, matching the legacy toast lifetime.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2800))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.state.toasts.dismiss(id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- actions ------------------------------------------------------------

    pub(crate) fn new_session(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = RUNTIME.get() else { return };
        let api = self.api.clone();
        let tx = self.tx.clone();
        let web_search = self.state.settings.web_search_enabled;
        let model = self.selected_model.clone();
        handle.spawn(async move {
            match api
                .create_session(Some("新会话"), model.as_deref(), web_search)
                .await
            {
                Ok(value) => match parse_session_response(value) {
                    Ok(session) => {
                        let _ = tx.unbounded_send(AppMsg::SessionCreated(session));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(err));
                    }
                },
                Err(err) => {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("新建会话失败: {err}")));
                }
            }
        });
        cx.notify();
    }

    // ---- plan actions -------------------------------------------------------

    pub(crate) fn confirm_active_plan(&mut self, cx: &mut Context<Self>) {
        let Some(plan_id) = self.active_plan_id.clone() else {
            self.toast("当前没有计划", ToastKind::Error, cx);
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.confirm_plan(&plan_id).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已确认计划".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("确认计划失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn execute_active_plan(&mut self, cx: &mut Context<Self>) {
        let Some(plan_id) = self.active_plan_id.clone() else {
            self.toast("当前没有计划", ToastKind::Error, cx);
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.execute_plan(&plan_id).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已开始执行计划".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("执行计划失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn replan_active_plan(&mut self, cx: &mut Context<Self>) {
        let Some(plan_id) = self.active_plan_id.clone() else {
            self.toast("当前没有计划", ToastKind::Error, cx);
            return;
        };
        let input = self.composer.read(cx).text().trim().to_string();
        if input.is_empty() {
            self.toast("请在输入框填写重新规划的要求", ToastKind::Error, cx);
            return;
        }
        self.composer.update(cx, |c, cx| c.set_text("", cx));
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.replan(&plan_id, &input).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已重新规划".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("重新规划失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn rerun_todo_action(&mut self, todo_id: String, cx: &mut Context<Self>) {
        let Some(run_id) = self.state.chat.active_run_id.clone() else {
            self.toast("没有正在运行的 Run", ToastKind::Error, cx);
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.rerun_todo(&run_id, &todo_id).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已重跑待办".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("重跑待办失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn rerun_plan_node(&mut self, node_id: String, cx: &mut Context<Self>) {
        let Some(run_id) = self.state.chat.active_run_id.clone() else {
            self.toast("没有正在运行的 Run", ToastKind::Error, cx);
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.rerun_node(&run_id, &node_id).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已重跑计划节点".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("重跑节点失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn select_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.renaming_session = None;
        self.editing_msg = None;
        self.state.active_session_id = Some(id.clone());
        if let Some(store) = &self.store {
            let _ = store.set_string(keys::SELECTED_SESSION, &id);
        }
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            let pid = id.clone();
            handle.spawn(async move {
                match api.snapshot(Some(&pid)).await {
                    Ok(snap) => {
                        let _ = tx.unbounded_send(AppMsg::Snapshot(Box::new(snap)));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("快照失败: {err}")));
                    }
                }
                if let Ok(v) = api.get_permission_mode(&pid).await {
                    if let Some(mode) = v.get("mode").and_then(|m| m.as_str()) {
                        let _ = tx.unbounded_send(AppMsg::PermissionMode(mode.to_string()));
                    }
                }
                let cps = api
                    .list_checkpoints(&pid)
                    .await
                    .ok()
                    .and_then(|v| v.get("checkpoints").and_then(|c| c.as_array()).cloned())
                    .unwrap_or_default();
                let _ = tx.unbounded_send(AppMsg::Checkpoints(cps));
            });
        }
        cx.notify();
    }

    pub(crate) fn set_permission_mode_backend(&mut self, mode: String, cx: &mut Context<Self>) {
        self.permission_mode = mode.clone();
        if let (Some(session), Some(handle)) =
            (self.state.active_session_id.clone(), RUNTIME.get())
        {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api.set_permission_mode(&session, &mode).await {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("权限模式同步失败: {err}")));
                }
            });
        } else {
            self.toast("请先选择会话", ToastKind::Error, cx);
        }
        cx.notify();
    }

    pub(crate) fn make_checkpoint(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.state.active_session_id.clone() else {
            self.toast("请先选择会话", ToastKind::Error, cx);
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.create_checkpoint(&session, Vec::new(), "手动检查点").await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已创建检查点".into()));
                        let cps = api
                            .list_checkpoints(&session)
                            .await
                            .ok()
                            .and_then(|v| v.get("checkpoints").and_then(|c| c.as_array()).cloned())
                            .unwrap_or_default();
                        let _ = tx.unbounded_send(AppMsg::Checkpoints(cps));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("创建检查点失败: {err}")));
                    }
                }
            });
        }
        self.toast("正在创建检查点…", ToastKind::Info, cx);
    }

    pub(crate) fn rewind_to_checkpoint(&mut self, checkpoint_id: String, cx: &mut Context<Self>) {
        let session = self.state.active_session_id.clone();
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.rewind_checkpoint(&checkpoint_id).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已回溯到检查点".into()));
                        // The backend truncated the event stream; re-pull the
                        // snapshot so chat/plan/diff reflect the rewound state.
                        if let Some(session) = session {
                            if let Ok(snap) = api.snapshot(Some(&session)).await {
                                let _ = tx.unbounded_send(AppMsg::Snapshot(Box::new(snap)));
                            }
                            let cps = api
                                .list_checkpoints(&session)
                                .await
                                .ok()
                                .and_then(|v| v.get("checkpoints").and_then(|c| c.as_array()).cloned())
                                .unwrap_or_default();
                            let _ = tx.unbounded_send(AppMsg::Checkpoints(cps));
                        }
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("回溯失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some(session_id) = self.state.active_session_id.clone() else {
            self.toast("请先新建或选择会话", ToastKind::Error, cx);
            return;
        };
        // Optimistic local echo. The replayed composer.user.message event
        // replaces this local entry once the backend assigns a run id.
        self.state.chat.push_local_user(text.clone());
        self.composer.update(cx, |c, cx| c.set_text("", cx));

        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            let mode = self.mode.as_str().to_string();
            handle.spawn(async move {
                match api.ask_execute(&session_id, &text, None, &mode).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已提交，等待响应…".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("提交失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn abort_run(&mut self, cx: &mut Context<Self>) {
        let Some(run_id) = self.state.chat.active_run_id.clone() else { return };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api.cancel_run(&run_id).await {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("中止失败: {err}")));
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn set_mode(&mut self, mode: ComposerMode, cx: &mut Context<Self>) {
        // Legacy COMPOSER_PLACEHOLDER_BY_MODE.
        let placeholder = match mode {
            ComposerMode::Build => "Add more optional details…",
            ComposerMode::Plan => "Plan and design before coding…",
            ComposerMode::Debug => "Investigate and reproduce a bug…",
            ComposerMode::Multitask => "Run multiple subagents in parallel…",
            ComposerMode::Ask => "Ask a question about your codebase…",
        };
        self.composer.update(cx, |c, cx| c.set_placeholder(placeholder, cx));
        self.mode = mode;
        cx.notify();
    }

    // ---- auth -------------------------------------------------------------------

    pub(crate) fn do_login(&mut self, cx: &mut Context<Self>) {
        let email = self.auth_email.read(cx).text().trim().to_string();
        let password = self.auth_password.read(cx).text().to_string();
        if email.is_empty() || password.is_empty() {
            self.login_error = Some("请输入邮箱与密码".into());
            cx.notify();
            return;
        }
        self.login_busy = true;
        self.login_error = None;
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.login(&email, &password).await {
                    Ok(auth) => {
                        let _ = tx.unbounded_send(AppMsg::LoggedIn(Box::new(auth)));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::LoginFailed(format!("登录失败：{err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    /// 跳过登录（调试）: unauthenticated bootstrap, matching the current
    /// dev behavior.
    pub(crate) fn skip_login(&mut self, cx: &mut Context<Self>) {
        self.authed = true;
        self.auth_user = Some("本地用户".into());
        self.bootstrap();
        cx.notify();
    }

    // ---- palette / overlays / theme -------------------------------------------

    /// (id, icon, label, section) command registry, filtered by the palette query.
    pub(crate) fn palette_commands(&self, cx: &App) -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
        const ALL: &[(&str, &str, &str, &str)] = &[
            ("session.new", "sparkles", "New Agent", "agent"),
            ("workspace.open", "folder", "Open Workspace", "agent"),
            ("tab.plan", "list-tree", "打开 Plan", "navigate"),
            ("tab.todo", "list-checks", "打开 Todo", "navigate"),
            ("tab.diff", "git-compare", "打开 Diff", "navigate"),
            ("tab.swarm", "network", "打开 Swarm", "navigate"),
            ("tab.readme", "file-text", "打开 README", "navigate"),
            ("settings.open", "settings-2", "打开设置", "view"),
            ("panel.toggle", "panel-bottom", "切换底部面板", "view"),
            ("theme.toggle", "sparkles", "切换主题", "view"),
            ("help.shortcuts", "message-square-text", "键盘快捷键", "view"),
            ("help.about", "user-round", "关于月夜", "view"),
        ];
        let query = self.palette_input.read(cx).text().to_lowercase();
        ALL.iter()
            .filter(|(id, _, label, _)| {
                query.is_empty() || label.to_lowercase().contains(&query) || id.contains(&query)
            })
            .copied()
            .collect()
    }

    pub(crate) fn run_palette_command(&mut self, id: &str, cx: &mut Context<Self>) {
        self.palette_open = false;
        match id {
            "session.new" => self.new_session(cx),
            "workspace.open" => self.open_workspace(cx),
            "settings.open" => self.settings_open = true,
            "panel.toggle" => self.toggle_bottom(cx),
            "theme.toggle" => self.toggle_theme(cx),
            "help.shortcuts" => self.shortcuts_open = true,
            "help.about" => self.about_open = true,
            tab if tab.starts_with("tab.") => {
                self.open_tab(tab.trim_start_matches("tab.").to_string(), cx)
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn run_palette_selection(&mut self, cx: &mut Context<Self>) {
        let commands = self.palette_commands(cx);
        if let Some((id, _, _, _)) = commands.get(self.palette_index.min(commands.len().saturating_sub(1))) {
            let id = id.to_string();
            self.run_palette_command(&id, cx);
        }
    }

    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_index = 0;
        self.palette_input.update(cx, |input, cx| input.set_text("", cx));
        let handle = self.palette_input.read(cx).focus_handle_clone();
        window.focus(&handle);
        cx.notify();
    }

    /// Esc: close every overlay/menu (legacy global Escape).
    pub(crate) fn close_overlays(&mut self, cx: &mut Context<Self>) {
        self.palette_open = false;
        self.settings_open = false;
        self.profile_open = false;
        self.notifs_open = false;
        self.about_open = false;
        self.shortcuts_open = false;
        self.menubar_open = None;
        self.chathead_menu = None;
        self.add_menu_open = false;
        self.model_menu_open = false;
        self.ws_menu_open = false;
        self.user_menu_open = false;
        self.subagent_overlay = None;
        self.renaming_session = None;
        self.editing_msg = None;
        self.context_drawer_open = false;
        self.settings_menu = None;
        self.crud_modal = None;
        cx.notify();
    }

    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        self.t = if self.dark { Tokens::claude_dark() } else { Tokens::claude_light() };
        let (accent, selection) = (self.t.accent, self.t.bg_selection);
        for input in [&self.composer, &self.edit_input, &self.sidebar_search, &self.ws_search, &self.rename_input, &self.palette_input] {
            input.update(cx, |i, _| i.set_accent(accent, selection));
        }
        for input in self.settings_inputs.values() {
            input.update(cx, |i, _| i.set_accent(accent, selection));
        }
        cx.notify();
    }

    pub(crate) fn fork_session(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_session_id.clone() else { return };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.fork_session(&id).await {
                    Ok(value) => match parse_session_response(value) {
                        Ok(session) => {
                            let _ = tx.unbounded_send(AppMsg::SessionCreated(session));
                        }
                        Err(err) => {
                            let _ = tx.unbounded_send(AppMsg::Status(format!("分支{err}")));
                        }
                    },
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("分支失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn revert_to(&mut self, message_id: String, cx: &mut Context<Self>) {
        let Some(id) = self.state.active_session_id.clone() else { return };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.revert_session(&id, Some(&message_id), false).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已回退到所选消息".into()));
                        if let Ok(snap) = api.snapshot(Some(&id)).await {
                            let _ = tx.unbounded_send(AppMsg::Snapshot(Box::new(snap)));
                        }
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("回退失败: {err}")));
                    }
                }
            });
        }
        cx.notify();
    }

    /// Enter inline edit mode on a historical user message.
    pub(crate) fn start_edit_message(
        &mut self,
        message_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.chat.active_run_id.is_some() {
            self.toast("当前有任务运行中，请先等待完成或中止", ToastKind::Error, cx);
            return;
        }
        let Some(text) = self
            .state
            .chat
            .messages
            .iter()
            .find(|m| m.id == message_id)
            .map(|m| m.text.clone())
        else {
            return;
        };
        self.edit_input.update(cx, |i, cx| i.set_text(text, cx));
        self.editing_msg = Some(message_id);
        let handle = self.edit_input.read(cx).focus_handle_clone();
        window.focus(&handle);
        cx.notify();
    }

    pub(crate) fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_msg = None;
        cx.notify();
    }

    /// "Edit = revert + resend": truncate the session to *before* the edited
    /// user message on the backend, then re-run `ask:execute` with new text.
    pub(crate) fn resend_edited(&mut self, cx: &mut Context<Self>) {
        let Some(message_id) = self.editing_msg.clone() else { return };
        let text = self.edit_input.read(cx).text().trim().to_string();
        if text.is_empty() {
            self.toast("请输入新的消息内容", ToastKind::Error, cx);
            return;
        }
        let Some(session_id) = self.state.active_session_id.clone() else {
            self.toast("请先选择会话", ToastKind::Error, cx);
            return;
        };
        self.editing_msg = None;
        // Optimistic local echo: drop the edited message and its tail, then
        // re-append the new text. The backend events re-sync the real state.
        self.state.chat.truncate_at(&message_id);
        self.state.chat.push_local_user(text.clone());

        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            let mode = self.mode.as_str().to_string();
            // "local-*" ids are optimistic echoes that never reached the
            // backend event stream, so there is nothing to revert.
            let needs_revert = !message_id.starts_with("local-");
            handle.spawn(async move {
                if needs_revert {
                    if let Err(err) = api.revert_session(&session_id, Some(&message_id), true).await {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("回退失败: {err}")));
                        if let Ok(snap) = api.snapshot(Some(&session_id)).await {
                            let _ = tx.unbounded_send(AppMsg::Snapshot(Box::new(snap)));
                        }
                        return;
                    }
                }
                match api.ask_execute(&session_id, &text, None, &mode).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已回退并重新发送".into()));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("重发失败: {err}")));
                        if let Ok(snap) = api.snapshot(Some(&session_id)).await {
                            let _ = tx.unbounded_send(AppMsg::Snapshot(Box::new(snap)));
                        }
                    }
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn copy_messages(&mut self, cx: &mut Context<Self>) {
        let text = self
            .state
            .chat
            .messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    ChatRole::User => "User",
                    ChatRole::Assistant => "Agent",
                    ChatRole::System => "System",
                };
                format!("[{role}] {}", m.text)
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.toast("已复制全部消息", ToastKind::Success, cx);
    }

    pub(crate) fn pin_session(&mut self, id: String, pinned: bool, cx: &mut Context<Self>) {
        if let Some(s) = self.state.sessions.iter_mut().find(|s| s.id == id) {
            s.pinned = pinned;
        }
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api.patch_session(&id, serde_json::json!({ "pinned": pinned })).await {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("置顶失败: {err}")));
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn delete_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.sessions.retain(|s| s.id != id);
        if self.state.active_session_id.as_deref() == Some(id.as_str()) {
            self.state.active_session_id = None;
            self.state.chat.messages.clear();
        }
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api.delete_session(&id).await {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("删除会话失败: {err}")));
                }
            });
        }
        self.toast("已删除会话", ToastKind::Success, cx);
    }

    pub(crate) fn start_rename(&mut self, id: String, cx: &mut Context<Self>) {
        let title = self
            .state
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.rename_input.update(cx, |input, cx| input.set_text(title, cx));
        self.renaming_session = Some(id);
        cx.notify();
    }

    pub(crate) fn commit_rename(&mut self, title: String, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_session.take() else { return };
        let title = title.trim().to_string();
        if title.is_empty() {
            cx.notify();
            return;
        }
        if let Some(s) = self.state.sessions.iter_mut().find(|s| s.id == id) {
            s.title = title.clone();
            s.title_manually_set = true;
        }
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api
                    .patch_session(&id, serde_json::json!({ "title": title, "titleManuallySet": true }))
                    .await
                {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("重命名失败: {err}")));
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn pick_model(&mut self, model_id: String, cx: &mut Context<Self>) {
        self.selected_model = Some(model_id.clone());
        self.model_menu_open = false;
        let Some(session_id) = self.state.active_session_id.clone() else {
            cx.notify();
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api.set_session_model(&session_id, &model_id).await {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("切换模型失败: {err}")));
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn open_tab(&mut self, id: impl Into<String>, cx: &mut Context<Self>) {
        let id = id.into();
        let builtin = match id.as_str() {
            "plan" => Some(crate::BuiltinTab::Plan),
            "todo" => Some(crate::BuiltinTab::Todo),
            "diff" => Some(crate::BuiltinTab::Diff),
            "swarm" => Some(crate::BuiltinTab::Swarm),
            "readme" => Some(crate::BuiltinTab::Readme),
            _ => None,
        };
        if let Some(kind) = builtin {
            self.state.workbench.open_builtin(kind);
        }
        self.active_tab = id;
        cx.notify();
    }

    pub(crate) fn open_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.state.workbench.workspace_tree = build_tree(&dir, 0);
        if let Some(store) = &self.store {
            let _ = store.set_string(keys::WORKSPACE_ROOT, dir.to_string_lossy().to_string());
        }
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            let path = dir.to_string_lossy().to_string();
            handle.spawn(async move {
                if let Err(err) = api.set_workspace_root(&path).await {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("设置工作区失败: {err}")));
                }
            });
        }
        self.toast(format!("工作区: {}", dir.display()), ToastKind::Info, cx);
        cx.notify();
    }

    pub(crate) fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        // Workspace-relative paths come from the backend tree; read them
        // through the REST gateway like the legacy frontend does.
        if std::path::Path::new(&path).is_absolute() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    self.state.workbench.open_file(path.clone(), content);
                    self.active_tab = format!("file:{path}");
                }
                Err(err) => self.toast(format!("读取失败: {err}"), ToastKind::Error, cx),
            }
            cx.notify();
            return;
        }
        let Some(handle) = RUNTIME.get() else { return };
        let api = self.api.clone();
        let tx = self.tx.clone();
        let rel = path.clone();
        handle.spawn(async move {
            match api.read_workspace_file(&rel).await {
                Ok(value) => {
                    let content = value
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let _ = tx.unbounded_send(AppMsg::FileLoaded { path: rel, content });
                }
                Err(err) => {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("读取失败: {err}")));
                }
            }
        });
        cx.notify();
    }

    /// Write the active file tab's buffer back to the workspace (Ctrl+S).
    pub(crate) fn save_active_file(&mut self, cx: &mut Context<Self>) {
        if !self.active_tab.starts_with("file:") {
            return;
        }
        let tab_id = self.active_tab.clone();
        let path = tab_id.trim_start_matches("file:").to_string();
        let content = self
            .state
            .workbench
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .and_then(|t| match &t.kind {
                TabKind::File(buf) => Some(buf.text().to_string()),
                _ => None,
            });
        let Some(content) = content else { return };
        let is_abs = std::path::Path::new(&path).is_absolute();
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            let p = path.clone();
            handle.spawn(async move {
                let res = if is_abs {
                    api.write_local_file(&p, &content).await
                } else {
                    api.write_workspace_file(&p, &content).await
                };
                match res {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("已保存 {p}")));
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("保存失败: {err}")));
                    }
                }
            });
        }
        if let Some(tab) = self.state.workbench.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.dirty = false;
        }
        self.toast("正在保存…", ToastKind::Info, cx);
    }

    pub(crate) fn close_tab(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.workbench.close_tab(&id);
        if self.active_tab == id {
            self.active_tab = self
                .state
                .workbench
                .tabs
                .last()
                .map(|t| t.id.clone())
                .unwrap_or_default();
        }
        cx.notify();
    }

    pub(crate) fn toggle_bottom(&mut self, cx: &mut Context<Self>) {
        self.state.workbench.bottom_panel.open = !self.state.workbench.bottom_panel.open;
        cx.notify();
    }

    pub(crate) fn set_bottom_tab(&mut self, tab: crate::BottomPanelTab, cx: &mut Context<Self>) {
        self.state.workbench.bottom_panel.active = tab;
        self.state.workbench.bottom_panel.open = true;
        cx.notify();
    }

    /// Lazy-expand a directory in the inspector tree via the REST gateway.
    pub(crate) fn toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if self.expanded_dirs.contains(&path) {
            self.expanded_dirs.remove(&path);
            cx.notify();
            return;
        }
        self.expanded_dirs.insert(path.clone());
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Ok(tree) = api.workspace_tree(&path, false).await {
                    let nodes = parse_backend_tree(&tree);
                    let _ = tx.unbounded_send(AppMsg::TreeChildren { parent: path, nodes });
                }
            });
        }
        cx.notify();
    }

    pub(crate) fn run_control(&mut self, action: &'static str, cx: &mut Context<Self>) {
        let Some(run_id) = self.state.chat.active_run_id.clone() else { return };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                let result = match action {
                    "pause" => api.pause_run(&run_id).await,
                    "resume" => api.resume_run(&run_id).await,
                    _ => api.cancel_run(&run_id).await,
                };
                if let Err(err) = result {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("运行控制失败: {err}")));
                }
            });
        }
        cx.notify();
    }

    /// Fetch the active run's structured logs + metrics into the Output panel.
    pub(crate) fn fetch_run_logs(&mut self, cx: &mut Context<Self>) {
        let Some(run_id) = self.state.chat.active_run_id.clone() else {
            self.toast("没有正在运行的 Run", ToastKind::Error, cx);
            return;
        };
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                let mut lines = Vec::new();
                if let Ok(v) = api.get_run_logs(&run_id).await {
                    if let Some(arr) = v.get("logs").and_then(|l| l.as_array()) {
                        for entry in arr {
                            let msg = entry
                                .get("message")
                                .or_else(|| entry.get("text"))
                                .and_then(|m| m.as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| entry.to_string());
                            let level = entry
                                .get("level")
                                .and_then(|l| l.as_str())
                                .unwrap_or("info");
                            lines.push(format!("[{level}] {msg}"));
                        }
                    }
                }
                if let Ok(v) = api.get_run_metrics(&run_id).await {
                    lines.push(format!("[metrics] {v}"));
                }
                if lines.is_empty() {
                    lines.push("该 Run 暂无日志".to_string());
                }
                let _ = tx.unbounded_send(AppMsg::RunLogs(lines));
            });
        }
        cx.notify();
    }

    pub(crate) fn refresh_readme(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                let readme = api
                    .read_workspace_file("README.md")
                    .await
                    .ok()
                    .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(str::to_string));
                let _ = tx.unbounded_send(AppMsg::Readme(readme));
            });
        }
        cx.notify();
    }

    pub(crate) fn apply_proposal_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(pid) = self.proposal_pids.get(idx).cloned() else { return };
        let session = self.state.active_session_id.clone();
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.apply_proposal(&pid).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已应用修改".into()));
                        refetch_proposals(&api, session.as_deref(), &tx).await;
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("应用失败: {err}")));
                    }
                }
            });
        }
        self.toast("正在应用修改…", ToastKind::Info, cx);
    }

    pub(crate) fn discard_proposal_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(pid) = self.proposal_pids.get(idx).cloned() else { return };
        let session = self.state.active_session_id.clone();
        if let Some(handle) = RUNTIME.get() {
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                match api.discard_proposal(&pid).await {
                    Ok(_) => {
                        let _ = tx.unbounded_send(AppMsg::Status("已拒绝提案".into()));
                        refetch_proposals(&api, session.as_deref(), &tx).await;
                    }
                    Err(err) => {
                        let _ = tx.unbounded_send(AppMsg::Status(format!("拒绝失败: {err}")));
                    }
                }
            });
        }
        self.toast("已拒绝提案", ToastKind::Info, cx);
    }

    pub(crate) fn toggle_web_search(&mut self, cx: &mut Context<Self>) {
        self.state.settings.web_search_enabled = !self.state.settings.web_search_enabled;
        let enabled = self.state.settings.web_search_enabled;
        // Persist on the active session so the backend toggles the search tool.
        if let (Some(session), Some(handle)) =
            (self.state.active_session_id.clone(), RUNTIME.get())
        {
            if let Some(s) = self.state.sessions.iter_mut().find(|s| s.id == session) {
                s.web_search_enabled = enabled;
            }
            let api = self.api.clone();
            let tx = self.tx.clone();
            handle.spawn(async move {
                if let Err(err) = api
                    .patch_session(&session, serde_json::json!({ "webSearchEnabled": enabled }))
                    .await
                {
                    let _ = tx.unbounded_send(AppMsg::Status(format!("联网开关同步失败: {err}")));
                }
            });
        }
        self.toast(
            if enabled { "当前会话已启用联网搜索工具" } else { "当前会话已关闭联网搜索工具" },
            ToastKind::Success,
            cx,
        );
    }

    // ---- settings store accessors (`moonlit:s:*`, 即改即存) ---------------------

    pub(crate) fn s_bool(&self, key: &str, default: bool) -> bool {
        self.store
            .as_ref()
            .and_then(|s| s.get_bool(key))
            .unwrap_or(default)
    }

    pub(crate) fn s_set_bool(&self, key: &str, v: bool) {
        if let Some(store) = &self.store {
            let _ = store.set(key, serde_json::Value::Bool(v));
        }
    }

    /// Flip a boolean setting in place.
    pub(crate) fn s_flip(&self, key: &str, default: bool) {
        self.s_set_bool(key, !self.s_bool(key, default));
    }

    pub(crate) fn s_str(&self, key: &str, default: &str) -> String {
        self.store
            .as_ref()
            .map(|s| s.get_string_or(key, default))
            .unwrap_or_else(|| default.to_string())
    }

    pub(crate) fn s_set_str(&self, key: &str, v: &str) {
        if let Some(store) = &self.store {
            let _ = store.set_string(key, v);
        }
    }

    pub(crate) fn s_int(&self, key: &str, default: i64) -> i64 {
        self.store
            .as_ref()
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(default)
    }

    pub(crate) fn s_set_int(&self, key: &str, v: i64) {
        if let Some(store) = &self.store {
            let _ = store.set(key, serde_json::Value::from(v));
        }
    }

    /// Fetch-or-create a pooled settings `TextInput` (draft fields).
    pub(crate) fn settings_input(
        &mut self,
        key: &str,
        placeholder: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.settings_inputs.get(key) {
            return input.clone();
        }
        let (accent, selection) = (self.t.accent, self.t.bg_selection);
        let placeholder = placeholder.to_string();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx, "", placeholder);
            i.set_accent(accent, selection);
            i
        });
        self.settings_inputs.insert(key.to_string(), input.clone());
        input
    }

    /// Pooled input bound to a store key: initialized from the store and
    /// persisted on every change (legacy `useStickyState` text fields).
    pub(crate) fn settings_pinput(
        &mut self,
        store_key: &'static str,
        placeholder: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.settings_inputs.get(store_key) {
            return input.clone();
        }
        let (accent, selection) = (self.t.accent, self.t.bg_selection);
        let initial = self.s_str(store_key, "");
        let placeholder = placeholder.to_string();
        let input = cx.new(|cx| {
            let mut i = TextInput::new(cx, initial, placeholder);
            i.set_accent(accent, selection);
            i
        });
        cx.subscribe(&input, move |this: &mut Self, input, event, cx| {
            if let TextInputEvent::Changed(_) = event {
                let text = input.read(cx).text().to_string();
                this.s_set_str(store_key, &text);
            }
        })
        .detach();
        self.settings_inputs.insert(store_key.to_string(), input.clone());
        input
    }

    /// Resolve a slider drag at window-x into its bound setting.
    pub(crate) fn apply_slider(&mut self, id: &str, x: f32) {
        let geom = { self.slider_geom.borrow().get(id).copied() };
        let Some((ox, w)) = geom else { return };
        if w <= 0. {
            return;
        }
        let ratio = ((x - ox) / w).clamp(0., 1.);
        match id {
            "slider-hue" => self.s_set_int("moonlit:s:hue", (ratio * 360.).round() as i64),
            "slider-intensity" => self.s_set_int("moonlit:s:intensity", (ratio * 100.).round() as i64),
            _ => {}
        }
    }

    // ---- settings shell / data --------------------------------------------------

    pub(crate) fn set_settings_page(&mut self, page: crate::SettingsPage, cx: &mut Context<Self>) {
        self.state.settings.page = page;
        self.settings_menu = None;
        self.s_set_str("moonlit:settingsPage", page.as_id());
        self.ensure_settings_data();
        cx.notify();
    }

    /// Lazy-load the backend data the current settings page needs.
    pub(crate) fn ensure_settings_data(&mut self) {
        if self.authed && self.auth_profile.is_none() && !self.profile_loading {
            self.load_profile();
        }
        match self.state.settings.page {
            crate::SettingsPage::Models => {
                if !self.channels_loaded && !self.channels_loading {
                    self.load_channels();
                }
                if self.tavily.is_none() && !self.tavily_loading {
                    self.load_tavily();
                }
            }
            crate::SettingsPage::Rules => {
                if self.skills.is_none() && !self.skills_loading {
                    self.load_skills();
                }
            }
            _ => {}
        }
    }

    /// 外观页主题 select: auto / light / dark.
    pub(crate) fn set_theme_choice(&mut self, value: &str, cx: &mut Context<Self>) {
        self.s_set_str("moonlit:settings:theme", value);
        let dark = value == "dark";
        if dark != self.dark {
            self.toggle_theme(cx);
        } else {
            cx.notify();
        }
    }

    fn load_profile(&mut self) {
        let Some(handle) = RUNTIME.get() else { return };
        self.profile_loading = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let profile = api.me().await.ok();
            let _ = tx.unbounded_send(AppMsg::Profile(profile.map(Box::new)));
        });
    }

    /// `update_profile` + refresh; powers 账户卡保存 / 套餐升降级 / 月度上限.
    pub(crate) fn save_profile_patch(
        &mut self,
        patch: serde_json::Value,
        ok_toast: &str,
        close_acct: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = RUNTIME.get() else { return };
        if close_acct {
            self.acct_saving = true;
        }
        let api = self.api.clone();
        let tx = self.tx.clone();
        let ok_toast = ok_toast.to_string();
        handle.spawn(async move {
            let result = match api.update_profile(patch).await {
                Ok(v) => {
                    let refreshed = api.me().await.unwrap_or(v);
                    Ok(Box::new(refreshed))
                }
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.unbounded_send(AppMsg::ProfileSaved { result, ok_toast, close_acct });
        });
        cx.notify();
    }

    /// 通用页账户卡: 保存 button.
    pub(crate) fn save_account(&mut self, cx: &mut Context<Self>) {
        let name = self
            .settings_inputs
            .get("acct:name")
            .map(|i| i.read(cx).text().trim().to_string())
            .unwrap_or_default();
        let workspace = self
            .settings_inputs
            .get("acct:ws")
            .map(|i| i.read(cx).text().trim().to_string())
            .unwrap_or_default();
        let avatar = self
            .settings_inputs
            .get("acct:avatar")
            .map(|i| i.read(cx).text().trim().chars().take(1).collect::<String>())
            .unwrap_or_default();
        let avatar = if avatar.is_empty() { name.chars().take(1).collect() } else { avatar };
        self.save_profile_patch(
            serde_json::json!({ "displayName": name, "workspace": workspace, "avatar": avatar }),
            "账户资料已更新",
            true,
            cx,
        );
    }

    /// 通用页登出: clear the token and fall back to the login gate.
    pub(crate) fn logout(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            let _ = store.remove(keys::AUTH_TOKEN);
        }
        self.api = MoonlitAgentApi::new(self.api.base_url().to_string());
        self.authed = false;
        self.auth_user = None;
        self.auth_profile = None;
        self.settings_open = false;
        self.acct_open = false;
        cx.notify();
    }

    // ---- 模型页: 渠道 + Tavily ----------------------------------------------------

    fn load_channels(&mut self) {
        let Some(handle) = RUNTIME.get() else { return };
        self.channels_loading = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let providers = api
                .list_provider_types()
                .await
                .ok()
                .and_then(|v| v.get("providerTypes").and_then(|p| p.as_array()).cloned())
                .unwrap_or_default();
            let channels = api
                .list_channels()
                .await
                .ok()
                .and_then(|v| v.get("channels").and_then(|c| c.as_array()).cloned())
                .unwrap_or_default();
            let _ = tx.unbounded_send(AppMsg::Channels { providers, channels });
        });
    }

    /// Open an empty channel form (添加渠道).
    pub(crate) fn new_channel(&mut self, cx: &mut Context<Self>) {
        if self.channel_draft.is_some() {
            return;
        }
        for (key, ph) in [("ch:name", "例如 DeepSeek 主力"), ("ch:base", "https://..."), ("ch:key", "sk-...")] {
            let input = self.settings_input(key, ph, cx);
            input.update(cx, |i, cx| i.set_text("", cx));
        }
        self.channel_draft = Some(ChannelDraft {
            id: None,
            provider: "deepseek".into(),
            api_key_set: false,
            enabled: true,
            model_enabled: Vec::new(),
            fetching_models: false,
        });
        cx.notify();
    }

    /// Open the form pre-filled from an existing channel row (编辑).
    pub(crate) fn edit_channel(&mut self, ch: serde_json::Value, cx: &mut Context<Self>) {
        let models: Vec<serde_json::Value> = ch
            .get("models")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let base = ch.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("").to_string();
        for (key, ph, val) in [
            ("ch:name", "例如 DeepSeek 主力", name),
            ("ch:base", "https://...", base),
            ("ch:key", "已配置（输入新 Key 以覆盖）", String::new()),
        ] {
            let input = self.settings_input(key, ph, cx);
            input.update(cx, |i, cx| i.set_text(val, cx));
        }
        let mut enabled_flags = Vec::new();
        for (i, m) in models.iter().enumerate() {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mname = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let id_input = self.settings_input(&format!("ch:m{i}:id"), "模型 ID（如 deepseek-chat）", cx);
            id_input.update(cx, |inp, cx| inp.set_text(id, cx));
            let name_input = self.settings_input(&format!("ch:m{i}:name"), "显示名（可选）", cx);
            name_input.update(cx, |inp, cx| inp.set_text(mname, cx));
            enabled_flags.push(m.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true));
        }
        self.channel_draft = Some(ChannelDraft {
            id: ch.get("id").and_then(|v| v.as_str()).map(str::to_string),
            provider: ch.get("provider").and_then(|v| v.as_str()).unwrap_or("custom").to_string(),
            api_key_set: ch.get("apiKeySet").and_then(|v| v.as_bool()).unwrap_or(false),
            enabled: ch.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            model_enabled: enabled_flags,
            fetching_models: false,
        });
        cx.notify();
    }

    /// Collect the channel form into the legacy payload shape.
    fn channel_payload(&self, cx: &Context<Self>) -> serde_json::Value {
        let draft = self.channel_draft.as_ref().expect("channel draft open");
        let read = |key: &str| {
            self.settings_inputs
                .get(key)
                .map(|i| i.read(cx).text().trim().to_string())
                .unwrap_or_default()
        };
        let name = read("ch:name");
        let mut models = Vec::new();
        for (i, enabled) in draft.model_enabled.iter().enumerate() {
            let id = read(&format!("ch:m{i}:id"));
            if id.is_empty() {
                continue;
            }
            models.push(serde_json::json!({
                "id": id,
                "name": read(&format!("ch:m{i}:name")),
                "enabled": enabled,
            }));
        }
        let mut payload = serde_json::json!({
            "name": if name.is_empty() { draft.provider.clone() } else { name },
            "provider": draft.provider,
            "baseUrl": read("ch:base"),
            "models": models,
            "enabled": draft.enabled,
        });
        let key = read("ch:key");
        if !key.is_empty() {
            payload["apiKey"] = serde_json::Value::String(key);
        }
        payload
    }

    /// 保存渠道 (create or update) and refresh the list.
    pub(crate) fn save_channel(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = RUNTIME.get() else { return };
        let Some(draft) = &self.channel_draft else { return };
        if draft.provider.is_empty() {
            self.toast("请选择供应商", ToastKind::Error, cx);
            return;
        }
        let payload = self.channel_payload(cx);
        let id = draft.id.clone();
        self.channel_saving = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let saved = match &id {
                Some(id) => api.update_channel(id, payload).await,
                None => api.create_channel(payload).await,
            };
            let result = match saved {
                Ok(_) => {
                    let channels = api
                        .list_channels()
                        .await
                        .ok()
                        .and_then(|v| v.get("channels").and_then(|c| c.as_array()).cloned())
                        .unwrap_or_default();
                    Ok(channels)
                }
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.unbounded_send(AppMsg::ChannelSaved(result));
        });
        cx.notify();
    }

    /// Row toggle: PUT the channel back with `enabled` flipped.
    pub(crate) fn toggle_channel_enabled(&mut self, ch: serde_json::Value, cx: &mut Context<Self>) {
        let Some(handle) = RUNTIME.get() else { return };
        let Some(id) = ch.get("id").and_then(|v| v.as_str()).map(str::to_string) else { return };
        let enabled = ch.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let mut payload = serde_json::json!({
            "name": ch.get("name").cloned().unwrap_or_default(),
            "provider": ch.get("provider").cloned().unwrap_or_default(),
            "baseUrl": ch.get("baseUrl").cloned().unwrap_or_default(),
            "models": ch.get("models").cloned().unwrap_or(serde_json::json!([])),
            "enabled": !enabled,
        });
        if payload["baseUrl"].is_null() {
            payload["baseUrl"] = serde_json::Value::String(String::new());
        }
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            match api.update_channel(&id, payload).await {
                Ok(_) => {
                    let channels = api
                        .list_channels()
                        .await
                        .ok()
                        .and_then(|v| v.get("channels").and_then(|c| c.as_array()).cloned())
                        .unwrap_or_default();
                    let _ = tx.unbounded_send(AppMsg::ChannelsList(channels));
                }
                Err(err) => {
                    let _ = tx.unbounded_send(AppMsg::SettingsToast(format!("切换失败:{err}"), false));
                }
            }
        });
        cx.notify();
    }

    pub(crate) fn remove_channel(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(handle) = RUNTIME.get() else { return };
        if self.channel_draft.as_ref().and_then(|d| d.id.as_deref()) == Some(id.as_str()) {
            self.channel_draft = None;
        }
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            match api.delete_channel(&id).await {
                Ok(_) => {
                    let _ = tx.unbounded_send(AppMsg::SettingsToast("渠道已删除".into(), true));
                    let channels = api
                        .list_channels()
                        .await
                        .ok()
                        .and_then(|v| v.get("channels").and_then(|c| c.as_array()).cloned())
                        .unwrap_or_default();
                    let _ = tx.unbounded_send(AppMsg::ChannelsList(channels));
                }
                Err(err) => {
                    let _ = tx.unbounded_send(AppMsg::SettingsToast(format!("删除失败:{err}"), false));
                }
            }
        });
        cx.notify();
    }

    pub(crate) fn add_model_row(&mut self, cx: &mut Context<Self>) {
        let idx = self.channel_draft.as_ref().map(|d| d.model_enabled.len()).unwrap_or(0);
        for (suffix, ph) in [("id", "模型 ID（如 deepseek-chat）"), ("name", "显示名（可选）")] {
            let input = self.settings_input(&format!("ch:m{idx}:{suffix}"), ph, cx);
            input.update(cx, |i, cx| i.set_text("", cx));
        }
        if let Some(draft) = &mut self.channel_draft {
            draft.model_enabled.push(true);
        }
        cx.notify();
    }

    pub(crate) fn remove_model_row(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.channel_draft else { return };
        if idx >= draft.model_enabled.len() {
            return;
        }
        let count = draft.model_enabled.len();
        draft.model_enabled.remove(idx);
        // Shift the texts of subsequent rows up by one.
        for i in idx..count - 1 {
            for suffix in ["id", "name"] {
                let next = self
                    .settings_inputs
                    .get(&format!("ch:m{}:{suffix}", i + 1))
                    .map(|e| e.read(cx).text().to_string())
                    .unwrap_or_default();
                if let Some(input) = self.settings_inputs.get(&format!("ch:m{i}:{suffix}")) {
                    input.update(cx, |inp, cx| inp.set_text(next, cx));
                }
            }
        }
        cx.notify();
    }

    /// 「从供应商获取」: POST channels:fetch-models with the current form.
    pub(crate) fn fetch_channel_models(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = RUNTIME.get() else { return };
        let Some(draft) = &self.channel_draft else { return };
        let key = self
            .settings_inputs
            .get("ch:key")
            .map(|i| i.read(cx).text().trim().to_string())
            .unwrap_or_default();
        if key.is_empty() && !draft.api_key_set {
            self.toast("请先输入 API Key，或保存渠道后再获取模型列表", ToastKind::Error, cx);
            return;
        }
        let mut payload = serde_json::json!({
            "provider": draft.provider,
            "baseUrl": self
                .settings_inputs
                .get("ch:base")
                .map(|i| i.read(cx).text().trim().to_string())
                .unwrap_or_default(),
        });
        if let Some(id) = &draft.id {
            payload["channelId"] = serde_json::Value::String(id.clone());
        }
        if !key.is_empty() {
            payload["apiKey"] = serde_json::Value::String(key);
        }
        if let Some(draft) = &mut self.channel_draft {
            draft.fetching_models = true;
        }
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let result = match api.fetch_channel_models(payload).await {
                Ok(res) if res.get("success").and_then(|v| v.as_bool()) != Some(false) => {
                    let models = res
                        .get("models")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    let id = m.get("id")?.as_str()?.to_string();
                                    let name = m
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(&id)
                                        .to_string();
                                    let enabled =
                                        m.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                                    Some((id, name, enabled))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Ok(models)
                }
                Ok(res) => Err(res
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("获取模型失败")
                    .to_string()),
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.unbounded_send(AppMsg::ChannelModels(result));
        });
        cx.notify();
    }

    /// Replace the draft's model rows with a fetched list.
    pub(crate) fn set_channel_model_rows(
        &mut self,
        models: Vec<(String, String, bool)>,
        cx: &mut Context<Self>,
    ) {
        let mut flags = Vec::new();
        for (i, (id, name, enabled)) in models.into_iter().enumerate() {
            let id_input = self.settings_input(&format!("ch:m{i}:id"), "模型 ID（如 deepseek-chat）", cx);
            id_input.update(cx, |inp, cx| inp.set_text(id, cx));
            let name_input = self.settings_input(&format!("ch:m{i}:name"), "显示名（可选）", cx);
            name_input.update(cx, |inp, cx| inp.set_text(name, cx));
            flags.push(enabled);
        }
        if let Some(draft) = &mut self.channel_draft {
            draft.model_enabled = flags;
        }
        cx.notify();
    }

    fn load_tavily(&mut self) {
        let Some(handle) = RUNTIME.get() else { return };
        self.tavily_loading = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let config = api
                .search_config()
                .await
                .ok()
                .and_then(|v| v.get("config").cloned())
                .unwrap_or(serde_json::json!({}));
            let _ = tx.unbounded_send(AppMsg::Tavily(Box::new(tavily_from_config(&config))));
        });
    }

    /// 「保存 Tavily 配置」.
    pub(crate) fn save_tavily(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = RUNTIME.get() else { return };
        let Some(draft) = self.tavily.clone() else { return };
        let api_key = self
            .settings_inputs
            .get("tavily:key")
            .map(|i| i.read(cx).text().trim().to_string())
            .unwrap_or_default();
        self.tavily_saving = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let payload = serde_json::json!({
                "enabled": draft.enabled,
                "provider": "tavily",
                "apiKey": api_key,
                "topic": draft.topic,
                "searchDepth": draft.search_depth,
                "timeRange": draft.time_range,
                "extractDepth": draft.extract_depth,
            });
            let result = match api.set_search_config(payload).await {
                Ok(res) => {
                    let config = res.get("config").cloned().unwrap_or(serde_json::json!({}));
                    Ok(Box::new(tavily_from_config(&config)))
                }
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.unbounded_send(AppMsg::TavilySaved(result));
        });
        cx.notify();
    }

    // ---- 规则页: 已发现技能 -------------------------------------------------------

    pub(crate) fn load_skills(&mut self) {
        let Some(handle) = RUNTIME.get() else { return };
        self.skills_loading = true;
        let api = self.api.clone();
        let tx = self.tx.clone();
        handle.spawn(async move {
            let items = api
                .list_skills()
                .await
                .ok()
                .and_then(|v| v.get("items").and_then(|i| i.as_array()).cloned())
                .unwrap_or_default();
            let _ = tx.unbounded_send(AppMsg::Skills(items));
        });
    }

    /// 预览/收起 a discovered skill, lazily reading its SKILL.md.
    pub(crate) fn toggle_skill(&mut self, name: String, cx: &mut Context<Self>) {
        if self.skill_open.as_deref() == Some(name.as_str()) {
            self.skill_open = None;
            cx.notify();
            return;
        }
        self.skill_open = Some(name.clone());
        if !self.skill_previews.contains_key(&name) {
            if let Some(handle) = RUNTIME.get() {
                let api = self.api.clone();
                let tx = self.tx.clone();
                handle.spawn(async move {
                    let content = match api.read_skill(&name).await {
                        Ok(res) => res
                            .get("skill")
                            .and_then(|s| s.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("（无内容）")
                            .to_string(),
                        Err(err) => format!("加载失败：{err}"),
                    };
                    let _ = tx.unbounded_send(AppMsg::SkillContent { name, content });
                });
            }
        }
        cx.notify();
    }

    // ---- CRUD sections (ConfigStore JSON arrays) ---------------------------------

    pub(crate) fn crud_items(&self, storage_key: &str) -> Vec<serde_json::Value> {
        self.store
            .as_ref()
            .and_then(|s| s.get(storage_key))
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    }

    /// Open the CRUD modal for create (`item = None`) or edit.
    pub(crate) fn open_crud(
        &mut self,
        storage_key: String,
        title: &'static str,
        add_label: &'static str,
        fields: &'static [CrudField],
        item: Option<&serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        let mut selects = std::collections::HashMap::new();
        for f in fields {
            let value = item
                .and_then(|it| it.get(f.key))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match &f.kind {
                CrudFieldKind::Select(options) => {
                    let default = options.first().map(|(v, _)| v.to_string()).unwrap_or_default();
                    selects.insert(f.key, value.unwrap_or(default));
                }
                _ => {
                    let input = self.settings_input(&format!("crud:{}", f.key), f.placeholder, cx);
                    input.update(cx, |i, cx| i.set_text(value.unwrap_or_default(), cx));
                }
            }
        }
        self.crud_modal = Some(CrudState {
            storage_key,
            title,
            add_label,
            fields,
            editing_id: item
                .and_then(|it| it.get("id"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            selects,
        });
        cx.notify();
    }

    /// CRUD modal「保存」: validate required fields, write the store array.
    pub(crate) fn save_crud(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.crud_modal else { return };
        let mut entry = serde_json::Map::new();
        let mut missing = Vec::new();
        for f in state.fields {
            let value = match &f.kind {
                CrudFieldKind::Select(_) => state.selects.get(f.key).cloned().unwrap_or_default(),
                _ => self
                    .settings_inputs
                    .get(&format!("crud:{}", f.key))
                    .map(|i| i.read(cx).text().trim().to_string())
                    .unwrap_or_default(),
            };
            if f.required && value.is_empty() {
                missing.push(f.label);
            }
            entry.insert(f.key.to_string(), serde_json::Value::String(value));
        }
        if !missing.is_empty() {
            self.toast(format!("请填写：{}", missing.join("、")), ToastKind::Error, cx);
            return;
        }
        let storage_key = state.storage_key.clone();
        let editing_id = state.editing_id.clone();
        let mut items = self.crud_items(&storage_key);
        match editing_id {
            Some(id) => {
                entry.insert("id".into(), serde_json::Value::String(id.clone()));
                let updated = serde_json::Value::Object(entry);
                for item in items.iter_mut() {
                    if item.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                        *item = updated.clone();
                    }
                }
            }
            None => {
                let id = format!(
                    "it_{:x}_{:04x}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0),
                    std::process::id() & 0xffff
                );
                entry.insert("id".into(), serde_json::Value::String(id));
                items.push(serde_json::Value::Object(entry));
            }
        }
        if let Some(store) = &self.store {
            let _ = store.set(&storage_key, serde_json::Value::Array(items));
        }
        self.crud_modal = None;
        self.toast("已保存", ToastKind::Success, cx);
    }

    pub(crate) fn remove_crud_item(&mut self, storage_key: &str, id: &str, cx: &mut Context<Self>) {
        let items: Vec<serde_json::Value> = self
            .crud_items(storage_key)
            .into_iter()
            .filter(|it| it.get("id").and_then(|v| v.as_str()) != Some(id))
            .collect();
        if let Some(store) = &self.store {
            let _ = store.set(storage_key, serde_json::Value::Array(items));
        }
        self.toast("已删除", ToastKind::Info, cx);
    }
}

/// Parse a backend search-config payload into a [`TavilyDraft`].
fn tavily_from_config(config: &serde_json::Value) -> TavilyDraft {
    let s = |key: &str, default: &str| {
        config
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    };
    TavilyDraft {
        enabled: config.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        api_key_set: config.get("apiKeySet").and_then(|v| v.as_bool()).unwrap_or(false),
        topic: s("topic", "general"),
        search_depth: s("searchDepth", "basic"),
        time_range: s("timeRange", ""),
        extract_depth: s("extractDepth", "basic"),
    }
}

impl Focusable for AgentIdeApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AgentIdeApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        if !self.authed {
            return self.render_login(cx).into_any_element();
        }
        let mut root = div()
            .relative()
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(|this, _: &TogglePalette, window, cx| {
                if this.palette_open {
                    this.palette_open = false;
                    cx.notify();
                } else {
                    this.open_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewSessionAction, _w, cx| this.new_session(cx)))
            .on_action(cx.listener(|this, _: &ToggleBottomAction, _w, cx| this.toggle_bottom(cx)))
            .on_action(cx.listener(|this, _: &SaveFileAction, _w, cx| this.save_active_file(cx)))
            .on_action(cx.listener(|this, _: &CloseOverlays, _w, cx| this.close_overlays(cx)))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _w, cx| {
                if !this.palette_open {
                    return;
                }
                let count = this.palette_commands(cx).len();
                match ev.keystroke.key.as_str() {
                    "down" if count > 0 => {
                        this.palette_index = (this.palette_index + 1) % count;
                        cx.notify();
                    }
                    "up" if count > 0 => {
                        this.palette_index = (this.palette_index + count - 1) % count;
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            // pane / bottom-panel drag resizing
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                let Some(kind) = this.dragging else { return };
                let x: f32 = ev.position.x.into();
                let y: f32 = ev.position.y.into();
                let total_w: f32 = window.viewport_size().width.into();
                let total_h: f32 = window.viewport_size().height.into();
                match kind {
                    "sessions" => this.pane_w.0 = x.clamp(200., 360.),
                    "chat" => this.pane_w.1 = (x - this.pane_w.0 - 9.).clamp(300., 560.),
                    "inspector" => this.pane_w.2 = (total_w - x - 4.5).clamp(240., 420.),
                    s if s.starts_with("slider-") => this.apply_slider(s, x),
                    _ => this.bottom_h = (total_h - y - 26.).clamp(120., 520.),
                }
                cx.notify();
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _ev: &gpui::MouseUpEvent, _w, cx| {
                    if this.dragging.take().is_some() {
                        if let Some(store) = &this.store {
                            let sizes = serde_json::to_string(&[
                                this.pane_w.0,
                                this.pane_w.1,
                                this.pane_w.2,
                            ])
                            .unwrap_or_default();
                            let _ = store.set_string(keys::PANE_SIZES, sizes);
                            let _ = store.set_string(keys::BOTTOM_HEIGHT, format!("{}", this.bottom_h));
                        }
                        cx.notify();
                    }
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(FONT_SANS)
            .text_size(px(13.0))
            .child(self.render_titlebar(cx))
            .child(self.render_body(cx))
            .child(self.render_statusbar(cx));

        if self.context_drawer_open {
            root = root.child(self.render_context_drawer(cx));
        }
        if self.notifs_open {
            root = root.child(self.render_notifications(cx));
        }
        if self.profile_open {
            root = root.child(self.render_profile_popover(cx));
        }
        if self.settings_open {
            root = root.child(self.render_settings(cx));
        }
        if self.about_open {
            root = root.child(self.render_about_modal(cx));
        }
        if self.shortcuts_open {
            root = root.child(self.render_shortcuts_modal(cx));
        }
        if self.palette_open {
            root = root.child(self.render_palette(cx));
        }
        root = root.child(self.render_toasts(cx));
        root.into_any_element()
    }
}

impl AgentIdeApp {
    /// Legacy `.body`: sessions 256 | 9 | chat 360 | 9 | main minmax(420,1fr)
    /// | 9 | inspector 288.
    fn render_body(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let mut body = div().flex().flex_row().flex_1().min_h(px(0.));

        if !self.sessions_collapsed {
            body = body.child(self.render_sidebar(cx));
        }
        body = body.child(pane_divider(
            "sessions",
            self.sessions_collapsed,
            &t,
            |this, _w, cx| {
                this.sessions_collapsed = !this.sessions_collapsed;
                cx.notify();
            },
            cx,
        ));

        if !self.chat_collapsed {
            body = body.child(self.render_chat_column(cx));
        }
        body = body.child(pane_divider(
            "chat",
            self.chat_collapsed,
            &t,
            |this, _w, cx| {
                this.chat_collapsed = !this.chat_collapsed;
                cx.notify();
            },
            cx,
        ));

        body = body.child(self.render_main(cx));

        body = body.child(pane_divider(
            "inspector",
            !self.inspector_collapsed,
            &t,
            |this, _w, cx| {
                this.inspector_collapsed = !this.inspector_collapsed;
                cx.notify();
            },
            cx,
        ));
        if !self.inspector_collapsed {
            body = body.child(self.render_inspector(cx));
        }
        body
    }
}

/// Map a backend `GET /workspace/tree` payload (flat entry list) into
/// [`WorkspaceNode`]s. Hidden entries are skipped; directories stay collapsed
/// like the legacy tree's initial state.
pub(crate) fn parse_backend_tree(tree: &serde_json::Value) -> Vec<WorkspaceNode> {
    let Some(entries) = tree.get("entries").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|e| {
            let name = e.get("name")?.as_str()?.to_string();
            let rel = e.get("relPath").and_then(|v| v.as_str()).unwrap_or(&name).to_string();
            let is_dir = e.get("kind").and_then(|v| v.as_str()) == Some("dir");
            let git = e.get("gitStatus").and_then(|v| v.as_str()).map(str::to_string);
            Some(WorkspaceNode {
                name,
                path: rel,
                kind: if is_dir { WorkspaceNodeKind::Directory } else { WorkspaceNodeKind::File },
                children: Vec::new(),
                git,
            })
        })
        .collect()
}

/// Build a bounded workspace tree by walking the filesystem.
pub(crate) fn build_tree(dir: &std::path::Path, depth: usize) -> Vec<WorkspaceNode> {
    if depth > 4 {
        return Vec::new();
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| {
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        (!is_dir, e.file_name().to_string_lossy().to_lowercase())
    });
    let mut nodes = Vec::new();
    for entry in entries.into_iter().take(200) {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "target" | ".git" | "node_modules" | "dist" | ".next") {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        nodes.push(WorkspaceNode {
            name,
            path: path.to_string_lossy().to_string(),
            kind: if is_dir { WorkspaceNodeKind::Directory } else { WorkspaceNodeKind::File },
            children: if is_dir { build_tree(&path, depth + 1) } else { Vec::new() },
            git: None,
        });
    }
    nodes
}
