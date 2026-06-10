//! 26px statusbar mirroring the legacy `StatusBar`: API segment, provider
//! segment, git branch, active session progress, tokens, web-search toggle,
//! UTF-8, bottom-panel toggle.

use gpui::{div, prelude::*, px, Context, Div, MouseButton, MouseDownEvent};
use moonlit_uikit::FONT_MONO_FALLBACK;

use super::icons::icon;
use super::status_dot;
use crate::app::AgentIdeApp;

impl AgentIdeApp {
    pub(crate) fn render_statusbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let seg = |children: Div| {
            children
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .px(px(6.))
                .h_full()
        };

        let api_label = if self.connected { "Agent Debug online" } else { "Agent Debug offline" };
        let api_dot = if self.connected { t.dot_done } else { t.dot_blocked };
        // Legacy provider segment: "Live · <model>" / "Mock provider" / offline.
        let (provider_label, provider_dot) = match &self.provider {
            Some((mode, model)) if mode == "live" => (format!("Live · {model}"), t.dot_done),
            Some((mode, _)) if mode == "mock" => ("Mock provider".to_string(), t.dot_running),
            Some(_) => ("Provider offline".to_string(), t.dot_blocked),
            None => (
                if self.connected { "Provider unknown".to_string() } else { "Provider offline".to_string() },
                t.dot_blocked,
            ),
        };
        let model_label = self
            .provider
            .as_ref()
            .map(|(_, model)| model.clone())
            .unwrap_or_else(|| "default model".to_string());
        let branch = self.git_branch.clone().unwrap_or_else(|| "main".to_string());
        let session_title = self
            .state
            .active_session_id
            .as_ref()
            .and_then(|id| self.state.sessions.iter().find(|s| &s.id == id))
            .map(|s| if s.title.is_empty() { s.id.clone() } else { s.title.clone() })
            .unwrap_or_else(|| "Agent Build".to_string());
        let (done, total) = self
            .metrics
            .plan_progress
            .as_ref()
            .map(|p| {
                (
                    p.get("completed").and_then(|v| v.as_u64()).unwrap_or(0),
                    p.get("total").and_then(|v| v.as_u64()).unwrap_or(0),
                )
            })
            .unwrap_or_else(|| {
                let done = self
                    .state
                    .todos
                    .iter()
                    .filter(|td| matches!(td.status.as_str(), "completed" | "done"))
                    .count() as u64;
                (done, self.state.todos.len() as u64)
            });
        let tokens = self.metrics.total_tokens;
        let web = self.state.settings.web_search_enabled;

        div()
            .h(px(26.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .px(px(8.))
            .bg(t.bg_sunk)
            .border_t_1()
            .border_color(t.line)
            .text_size(px(11.))
            .text_color(t.text_3)
            .child(seg(div()).child(status_dot(api_dot)).child(api_label))
            .child(seg(div()).child(status_dot(provider_dot)).child(provider_label))
            .child(seg(div()).child(icon("git-branch", 11., t.text_3)).child(branch))
            .child(
                seg(div())
                    .text_color(t.accent)
                    .child(status_dot(t.dot_running))
                    .child(format!("{session_title} · {done}/{total}")),
            )
            .child(div().flex_1())
            .child(
                seg(div())
                    .font_family(FONT_MONO_FALLBACK)
                    .child(format!("{tokens} tokens")),
            )
            .child(
                seg(div())
                    .cursor_pointer()
                    .hover(move |s| s.text_color(t.text))
                    .child(format!("联网搜索 {}", if web { "开" } else { "关" }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| this.toggle_web_search(cx)),
                    ),
            )
            .child(seg(div()).child(model_label))
            .child(seg(div()).child("UTF-8"))
            .child(
                seg(div())
                    .cursor_pointer()
                    .hover(move |s| s.text_color(t.text))
                    .child(icon("panel-bottom", 11., t.text_3))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| this.toggle_bottom(cx)),
                    ),
            )
    }
}
