//! Composer card mirroring the legacy `Composer`: TodoStrip on top, input
//! shell, and the mode bar (`+` with its dropdown, amber mode chip, spacer,
//! globe, model dropdown, 26px round accent send / danger abort / mic).

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, MouseDownEvent};
use moonlit_uikit::{ToastKind, FONT_MONO_FALLBACK};

use super::icons::icon;
use super::{menu_item, sh1};
use crate::app::AgentIdeApp;
use crate::ComposerMode;

const MODES: &[(ComposerMode, &str, &str)] = &[
    (ComposerMode::Plan, "list-tree", "Plan"),
    (ComposerMode::Debug, "bug", "Debug"),
    (ComposerMode::Multitask, "split", "Multitask"),
    (ComposerMode::Ask, "message-square-text", "Ask"),
];

impl AgentIdeApp {
    pub(crate) fn render_composer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let streaming = self.state.chat.active_run_id.is_some();
        let has_text = !self.composer.read(cx).text().trim().is_empty();
        let mode = self.mode.clone();
        let mode_label = MODES.iter().find(|(m, _, _)| *m == mode).map(|(_, i, l)| (*i, *l));
        let model_label = self
            .selected_model
            .clone()
            .or_else(|| self.provider.as_ref().map(|(_, m)| m.clone()))
            .unwrap_or_else(|| "default".to_string());

        // `.composer-wrap`: padding 10px 16px 14px + dashed top divider
        div()
            .flex_none()
            .pt(px(10.))
            .px(px(16.))
            .pb(px(14.))
            .border_t_1()
            .border_dashed()
            .border_color(t.line)
            .child(
                // `.composer` card (relative so the dropdowns can float above)
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .shadow(sh1())
                    .child(self.render_todo_strip(cx))
                    // input shell
                    .child(
                        div()
                            .pt(px(8.))
                            .px(px(14.))
                            .pb(px(6.))
                            .text_size(px(13.5))
                            .child(self.composer.clone()),
                    )
                    // mode bar
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .pb(px(8.))
                            // `+` circle 26x26 with dropdown
                            .child(
                                div()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if self.add_menu_open { t.accent_ring } else { t.line })
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("plus", 14., t.text_2))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.add_menu_open = !this.add_menu_open;
                                            this.model_menu_open = false;
                                            cx.notify();
                                        }),
                                    ),
                            )
                            // amber mode chip (non-default modes)
                            .when_some(mode_label, |d, (icon_name, label)| {
                                d.child(
                                    div()
                                        .h(px(22.))
                                        .px(px(8.))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(4.))
                                        .rounded_full()
                                        .bg(t.accent_bg)
                                        .text_color(t.accent)
                                        .text_size(px(11.))
                                        .child(icon(icon_name, 11., t.accent))
                                        .child(label)
                                        .child(
                                            div()
                                                .cursor_pointer()
                                                .child(icon("x", 10., t.accent))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                                        this.set_mode(ComposerMode::Build, cx)
                                                    }),
                                                ),
                                        ),
                                )
                            })
                            .child(div().flex_1())
                            // globe (web search)
                            .child(
                                div()
                                    .h(px(22.))
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon(
                                        "globe",
                                        11.,
                                        if self.state.settings.web_search_enabled { t.accent } else { t.text_3 },
                                    ))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.toggle_web_search(cx)
                                        }),
                                    ),
                            )
                            // model chip with dropdown
                            .child(
                                div()
                                    .h(px(22.))
                                    .px(px(6.))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.))
                                    .rounded(px(6.))
                                    .text_size(px(11.))
                                    .text_color(t.text_2)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("sparkles", 11., t.text_2))
                                    .child(div().max_w(px(140.)).truncate().child(model_label))
                                    .child(icon("chevron-down", 10., t.text_3))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.model_menu_open = !this.model_menu_open;
                                            this.add_menu_open = false;
                                            cx.notify();
                                        }),
                                    ),
                            )
                            // send / abort / mic
                            .child(if streaming {
                                div()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(t.danger)
                                    .cursor_pointer()
                                    .child(icon("square", 11., t.text_inv))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.abort_run(cx)
                                        }),
                                    )
                                    .into_any_element()
                            } else if has_text {
                                div()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(t.accent)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.accent_soft))
                                    .child(icon("arrow-up", 13., t.text_inv))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.send(cx)
                                        }),
                                    )
                                    .into_any_element()
                            } else {
                                div()
                                    .w(px(26.))
                                    .h(px(26.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("mic", 13., t.text_3))
                                    .into_any_element()
                            }),
                    )
                    .when(self.add_menu_open, |d| d.child(self.render_add_menu(cx)))
                    .when(self.model_menu_open, |d| d.child(self.render_model_menu(cx))),
            )
    }

    /// `.cmb-add-menu`: floats above the mode bar (min-w 196, radius 10);
    /// modes, then a separator, then placeholder groups.
    fn render_add_menu(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let current = self.mode.clone();
        let mut menu = div()
            .absolute()
            .bottom(px(40.))
            .left(px(8.))
            .min_w(px(196.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.line_strong)
            .bg(t.bg_panel)
            .shadow(super::sh_float());
        for (mode, icon_name, label) in MODES {
            let is_active = *mode == current;
            let mode = mode.clone();
            menu = menu.child(
                menu_item(*icon_name, label, is_active, &t)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.set_mode(mode.clone(), cx);
                            this.add_menu_open = false;
                            cx.notify();
                        }),
                    ),
            );
        }
        menu = menu.child(div().h(px(1.)).my(px(4.)).bg(t.line));
        menu = menu.child(
            menu_item("folder-tree", "添加上下文…", false, &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.add_menu_open = false;
                    this.context_drawer_open = true;
                    cx.notify();
                }),
            ),
        );
        for (icon_name, label) in [
            ("file", "Image"),
            ("sparkles", "Models"),
            ("book-open", "Skills"),
            ("plug", "MCP Servers"),
        ] {
            menu = menu.child(
                menu_item(icon_name, label, false, &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.add_menu_open = false;
                        this.toast(format!("{label} 即将上线"), ToastKind::Info, cx);
                    }),
                ),
            );
        }
        menu.into_any_element()
    }

    /// `.cmb-add-menu--right`: model picker (min-w 214) with title + provider
    /// meta rows fed by `GET /models`.
    fn render_model_menu(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let selected = self
            .selected_model
            .clone()
            .or_else(|| self.provider.as_ref().map(|(_, m)| m.clone()));
        let mut menu = div()
            .absolute()
            .bottom(px(40.))
            .right(px(40.))
            .min_w(px(214.))
            .max_h(px(320.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.line_strong)
            .bg(t.bg_panel)
            .shadow(super::sh_float())
            .overflow_hidden();
        if self.models.is_empty() {
            menu = menu.child(
                div()
                    .p(px(10.))
                    .text_size(px(11.5))
                    .text_color(t.text_4)
                    .child("暂无可用模型"),
            );
        }
        for m in self.models.iter().take(12) {
            let is_active = selected.as_deref() == Some(m.id.as_str());
            let id = m.id.clone();
            let title = if m.label.is_empty() { m.id.clone() } else { m.label.clone() };
            let provider = m.provider.clone().unwrap_or_default();
            menu = menu.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_active, |d| d.bg(t.accent_bg))
                    .hover(move |s| s.bg(t.bg_selection))
                    .child(icon("sparkles", 11., t.text_3))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(12.)).truncate().child(title))
                            .when(!provider.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(t.text_4)
                                        .truncate()
                                        .child(provider),
                                )
                            }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.pick_model(id.clone(), cx)
                        }),
                    ),
            );
        }
        menu.into_any_element()
    }

    /// Legacy `TodoStrip` on top of the composer card: head with progress and
    /// "打开看板 ↗", body listing todo rows (collapsed shows just the head).
    fn render_todo_strip(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let todos = &self.state.todos;
        if todos.is_empty() {
            return div().into_any_element();
        }
        let done = todos
            .iter()
            .filter(|td| matches!(td.status.as_str(), "completed" | "done"))
            .count();
        let total = todos.len();
        let open = self.todo_strip_open;
        let progress = if total == 0 { 0. } else { done as f32 / total as f32 };

        let mut strip = div()
            .flex()
            .flex_col()
            .px(px(12.))
            .pt(px(8.))
            .pb(if open { px(4.) } else { px(8.) })
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .text_color(t.text_3)
                    .child(icon("list-todo", 12., t.text_3))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("TODO"))
                    .child(
                        div()
                            .font_family(FONT_MONO_FALLBACK)
                            .text_size(px(10.5))
                            .child(format!("{done}/{total}")),
                    )
                    // 120x4 progress bar
                    .child(
                        div()
                            .w(px(120.))
                            .h(px(4.))
                            .rounded_full()
                            .bg(t.bg_active)
                            .child(
                                div()
                                    .w(gpui::relative(progress))
                                    .h_full()
                                    .rounded_full()
                                    .bg(t.accent),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_color(t.accent)
                            .cursor_pointer()
                            .child("打开看板 ↗")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.open_tab("todo", cx)
                                }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child(icon(if open { "chevron-down" } else { "chevron-up" }, 11., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.todo_strip_open = !this.todo_strip_open;
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        if open {
            for td in todos.iter().take(4) {
                let is_done = matches!(td.status.as_str(), "completed" | "done");
                let is_running = matches!(td.status.as_str(), "running" | "in_progress");
                strip = strip.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .py(px(3.))
                        .text_size(px(11.5))
                        .child(if is_done {
                            icon("check", 11., t.sage).into_any_element()
                        } else if is_running {
                            super::status_dot(t.dot_running).into_any_element()
                        } else {
                            div()
                                .w(px(10.))
                                .h(px(10.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(t.line_strong)
                                .into_any_element()
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_color(if is_done { t.text_4 } else { t.text_2 })
                                .when(is_done, |d| d.line_through())
                                .child(if td.title.is_empty() { td.id.clone() } else { td.title.clone() }),
                        ),
                );
            }
        }
        strip.into_any_element()
    }
}