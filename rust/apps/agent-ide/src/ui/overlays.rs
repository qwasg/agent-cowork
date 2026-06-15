//! Overlays: bottom-right toast stack with 3px tone bars, the centered
//! command palette, drawers, popovers and modals. The fullscreen settings
//! replica lives in [`super::settings`].

use gpui::{div, prelude::*, px, AnimationExt, Context, MouseButton, MouseDownEvent};
use moonlit_uikit::{ToastKind, FONT_SERIF};

use super::icons::icon;
use super::{float_surface, kbd, sh_float, status_dot};
use crate::app::{AgentIdeApp, ContextApplyTarget};

impl AgentIdeApp {
    // ---- command palette -----------------------------------------------------------

    /// Real palette: text input filtering, ↑↓ + Enter navigation, section
    /// headers, Esc closes (global binding).
    pub(crate) fn render_palette(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let commands = self.palette_commands(cx);
        let selected = self.palette_index.min(commands.len().saturating_sub(1));
        let mut list = div().flex().flex_col().max_h(px(380.)).overflow_hidden();
        let mut last_section = "";
        for (i, (id, icon_name, label, section)) in commands.iter().enumerate() {
            if *section != last_section {
                last_section = section;
                let section_label = match *section {
                    "agent" => "Agent",
                    "navigate" => "导航",
                    _ => "视图",
                };
                list = list.child(
                    div()
                        .px(px(14.))
                        .pt(px(8.))
                        .pb(px(2.))
                        .text_size(px(10.))
                        .text_color(t.text_4)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(section_label),
                );
            }
            let id = id.to_string();
            let is_sel = i == selected;
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .py(px(8.))
                    .text_size(px(13.))
                    .when(is_sel, |d| d.bg(t.accent_bg))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.accent_bg))
                    .child(icon(icon_name, 13., t.text_3))
                    .child(div().flex_1().child(label.to_string()))
                    .when(is_sel, |d| d.child(kbd("↵", &t)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.run_palette_command(&id, cx);
                        }),
                    ),
            );
        }
        if commands.is_empty() {
            list = list.child(
                div()
                    .p(px(16.))
                    .text_size(px(12.))
                    .text_color(t.text_4)
                    .child("没有匹配的命令"),
            );
        }

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .bg(gpui::rgba(0x2a27242e)) // rgba(42,39,36,0.18)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.palette_open = false;
                    cx.notify();
                }),
            )
            .child(
                float_surface(&t)
                    .mt(gpui::relative(0.15))
                    .w(px(600.))
                    .h_auto()
                    .flex()
                    .flex_col()
                    .rounded(px(12.))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _ev: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .px(px(14.))
                            .py(px(12.))
                            .border_b_1()
                            .border_color(t.line)
                            .child(icon("search", 14., t.text_3))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(14.))
                                    .child(self.palette_input.clone()),
                            )
                            .child(kbd("Esc", &t)),
                    )
                    .child(list),
            )
    }

    // ---- toasts ----------------------------------------------------------------

    /// Bottom-right toast stack: `bottom: 46px; right: 20px`, 3px tone bar.
    pub(crate) fn render_toasts(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let toasts: Vec<_> = self.state.toasts.items().to_vec();
        div()
            .absolute()
            .bottom(px(46.))
            .right(px(20.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(toasts.into_iter().map(|toast| {
                let (bar, dot) = match toast.kind {
                    ToastKind::Success => (t.sage, t.dot_done),
                    ToastKind::Error => (t.danger, t.dot_blocked),
                    ToastKind::Warning => (t.warn, t.dot_queued),
                    ToastKind::Info => (t.accent, t.dot_running),
                };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(0.))
                    .pr(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .shadow(sh_float())
                    .text_size(px(12.))
                    .overflow_hidden()
                    .child(div().w(px(3.)).h_full().bg(bar).flex_none())
                    .child(status_dot(dot))
                    .child(toast.title.clone())
                    // legacy `toastIn`: 180ms slide from translateX(8px)
                    .with_animation(
                        ("toast", toast.id as usize),
                        gpui::Animation::new(std::time::Duration::from_millis(180)),
                        |el, delta| el.opacity(delta).mr(px(8. * (1. - delta))),
                    )
            }))
    }

    // ---- context drawer ---------------------------------------------------------

    /// `ContextDrawer`: 420px fixed right (between titlebar and statusbar),
    /// filter chips, checkable entries, footer 取消 / 应用到 Composer/编辑内容.
    pub(crate) fn render_context_drawer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let filter = self.ctx_filter;
        let apply_to_edit = self.context_apply_target == ContextApplyTarget::EditMessage
            && self.editing_msg.is_some();
        let apply_label = if apply_to_edit {
            "应用到编辑内容"
        } else {
            "应用到 Composer"
        };
        let toast_label = if apply_to_edit {
            "已应用到编辑内容"
        } else {
            "已应用到 Composer"
        };
        let entries: Vec<(String, bool)> = self
            .state
            .workbench
            .workspace_tree
            .iter()
            .filter(|n| matches!(n.kind, crate::WorkspaceNodeKind::File))
            .filter(|n| match filter {
                "ts" => n.name.ends_with(".ts") || n.name.ends_with(".tsx"),
                "terminal" => false,
                "image" => {
                    n.name.ends_with(".png") || n.name.ends_with(".jpg") || n.name.ends_with(".svg")
                }
                _ => true,
            })
            .map(|n| (n.path.clone(), self.ctx_selected.contains(&n.path)))
            .collect();
        let selected_count = self.ctx_selected.len();
        let filter_chip = |id: &'static str, label: &'static str, cx: &mut Context<AgentIdeApp>| {
            let is_active = filter == id;
            div()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(if is_active { t.accent_ring } else { t.line })
                .bg(if is_active { t.accent_bg } else { t.bg_panel })
                .text_size(px(11.))
                .text_color(if is_active { t.accent } else { t.text_2 })
                .cursor_pointer()
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.ctx_filter = id;
                        cx.notify();
                    }),
                )
        };

        float_surface(&t)
            .absolute()
            .top(px(36.))
            .bottom(px(26.))
            .right(px(0.))
            .w(px(420.))
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(t.line_strong)
            // head
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("添加 Agent 上下文"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_3)
                            .child(format!("已选 {selected_count}")),
                    )
                    .child(
                        div()
                            .cursor_pointer()
                            .child(icon("x", 12., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.context_drawer_open = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            // filter chips
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .px(px(14.))
                    .py(px(8.))
                    .child(filter_chip("all", "全部", cx))
                    .child(filter_chip("ts", "仅 .ts·tsx", cx))
                    .child(filter_chip("terminal", "终端", cx))
                    .child(filter_chip("image", "参考图", cx)),
            )
            // entries
            .child(
                div()
                    .id("ctx-entries")
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .when(entries.is_empty(), |d| {
                        d.child(
                            div()
                                .p(px(20.))
                                .text_size(px(12.))
                                .text_color(t.text_3)
                                .child(
                                "暂无可选上下文。打开文件或同步工作区后，这里会出现可勾选条目。",
                            ),
                        )
                    })
                    .children(entries.into_iter().map(|(path, checked)| {
                        let toggle = path.clone();
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .px(px(14.))
                            .py(px(6.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child(if checked {
                                div()
                                    .w(px(14.))
                                    .h(px(14.))
                                    .rounded(px(3.))
                                    .bg(t.accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("check", 9., t.text_inv))
                                    .into_any_element()
                            } else {
                                div()
                                    .w(px(14.))
                                    .h(px(14.))
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(t.line_strong)
                                    .into_any_element()
                            })
                            .child(icon("file", 12., t.text_4))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_size(px(12.))
                                    .child(path.clone()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                    if !this.ctx_selected.remove(&toggle) {
                                        this.ctx_selected.insert(toggle.clone());
                                    }
                                    cx.notify();
                                }),
                            )
                    })),
            )
            // foot
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .h(px(26.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.line)
                            .text_size(px(12.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child("取消")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.context_drawer_open = false;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .h(px(26.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .bg(t.accent)
                            .text_size(px(12.))
                            .text_color(gpui::rgb(0xffffff))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.accent_soft))
                            .child(apply_label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                    let tokens: Vec<String> =
                                        this.ctx_selected.iter().map(|p| format!("@{p}")).collect();
                                    if !tokens.is_empty() {
                                        let target = this.context_apply_target
                                            == ContextApplyTarget::EditMessage
                                            && this.editing_msg.is_some();
                                        if target {
                                            let existing =
                                                this.edit_input.read(cx).text().to_string();
                                            let new_text =
                                                format!("{} {}", tokens.join(" "), existing)
                                                    .trim()
                                                    .to_string();
                                            this.edit_input
                                                .update(cx, |c, cx| c.set_text(new_text, cx));
                                        } else {
                                            let existing =
                                                this.composer.read(cx).text().to_string();
                                            let new_text =
                                                format!("{} {}", tokens.join(" "), existing)
                                                    .trim()
                                                    .to_string();
                                            this.composer
                                                .update(cx, |c, cx| c.set_text(new_text, cx));
                                        }
                                    }
                                    this.context_drawer_open = false;
                                    if this.context_apply_target == ContextApplyTarget::EditMessage
                                        && this.editing_msg.is_none()
                                    {
                                        this.context_apply_target = ContextApplyTarget::Composer;
                                    }
                                    this.toast(toast_label, ToastKind::Success, cx);
                                }),
                            ),
                    ),
            )
    }

    // ---- notifications drawer -----------------------------------------------------

    /// `NotificationsDrawer`: 380px fixed right with 全部/未读/提及 tabs and
    /// tone-bar rows.
    pub(crate) fn render_notifications(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let tab = self.notif_tab;
        let items: Vec<String> = match tab {
            "unread" | "mention" => Vec::new(),
            _ => self.logs.iter().rev().take(30).cloned().collect(),
        };
        let tab_btn = |id: &'static str, label: &'static str, cx: &mut Context<AgentIdeApp>| {
            let is_active = tab == id;
            div()
                .px(px(10.))
                .py(px(6.))
                .border_b_2()
                .border_color(if is_active {
                    t.accent
                } else {
                    gpui::rgba(0x00000000)
                })
                .text_size(px(12.))
                .text_color(if is_active { t.text } else { t.text_3 })
                .cursor_pointer()
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.notif_tab = id;
                        cx.notify();
                    }),
                )
        };
        float_surface(&t)
            .absolute()
            .top(px(36.))
            .bottom(px(26.))
            .right(px(0.))
            .w(px(380.))
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(t.line_strong)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(14.))
                    .pt(px(8.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(tab_btn("all", "全部", cx))
                    .child(tab_btn("unread", "未读", cx))
                    .child(tab_btn("mention", "提及 @我", cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .pb(px(6.))
                            .cursor_pointer()
                            .child(icon("x", 12., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.notifs_open = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("notif-list")
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .when(items.is_empty(), |d| {
                        d.child(
                            div()
                                .p(px(20.))
                                .text_size(px(12.))
                                .text_color(t.text_4)
                                .child("暂无通知。"),
                        )
                    })
                    .children(items.into_iter().map(|text| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.))
                            .border_b_1()
                            .border_color(t.line)
                            .overflow_hidden()
                            .child(div().w(px(3.)).h(px(44.)).flex_none().bg(t.accent))
                            .child(
                                div()
                                    .w(px(36.))
                                    .h(px(36.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(8.))
                                    .bg(t.accent_bg)
                                    .child(icon("bell", 14., t.accent)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .py(px(8.))
                                    .pr(px(12.))
                                    .text_size(px(12.))
                                    .text_color(t.text_2)
                                    .line_clamp(2)
                                    .child(text),
                            )
                    })),
            )
    }

    // ---- profile popover -------------------------------------------------------------

    /// `ProfilePopover`: 260px card anchored above the sidebar user card.
    pub(crate) fn render_profile_popover(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let m = &self.metrics;
        let stat = |label: &'static str, value: String| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(value),
                )
                .child(div().text_size(px(10.)).text_color(t.text_4).child(label))
        };
        float_surface(&t)
            .absolute()
            .bottom(px(48.))
            .left(px(52.))
            .w(px(260.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(40.))
                            .h(px(40.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(t.accent)
                            .text_color(gpui::rgb(0xffffff))
                            .text_size(px(16.))
                            .child("我"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("本地用户"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.text_4)
                                    .child("local@moonlit"),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .cursor_pointer()
                            .child(icon("x", 11., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.profile_open = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(t.bg_sunk)
                    .child(stat("Tokens", format!("{}", m.total_tokens)))
                    .child(stat("Tool calls", format!("{}", m.tool_calls)))
                    .child(stat("Subagents", format!("{}", m.subagents))),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.line)
                    .text_size(px(12.))
                    .text_color(t.danger)
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.danger_bg))
                    .child("登出")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                            this.profile_open = false;
                            this.toast("本地模式无需登出", ToastKind::Info, cx);
                        }),
                    ),
            )
    }

    // ---- about & shortcuts modals ---------------------------------------------------

    pub(crate) fn render_about_modal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        modal_overlay(t, cx, |this| this.about_open = false).child(
            float_surface(&t)
                .w(px(420.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(22.))
                .rounded(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .w(px(56.))
                                .h(px(56.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(12.))
                                .bg(t.text)
                                .text_color(t.text_inv)
                                .text_size(px(26.))
                                .font_family(FONT_SERIF)
                                .child("月"),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(16.))
                                        .font_family(FONT_SERIF)
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("月夜 · 文档编译助手"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(t.text_4)
                                        .child("v0.1.0 · GPUI Native"),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .text_size(px(12.))
                        .text_color(t.text_3)
                        .child(format!("后端：{}", self.api.base_url()))
                        .child("前端：GPUI (Rust) 原生复刻"),
                ),
        )
    }

    pub(crate) fn render_shortcuts_modal(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let group = |title: &'static str, rows: Vec<(&'static str, &'static str)>| {
            let mut g = div().flex().flex_col().gap(px(4.)).child(
                div()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(t.text_4)
                    .child(title),
            );
            for (label, keys) in rows {
                g = g.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(t.text_2)
                                .child(label),
                        )
                        .child(kbd(keys, &t)),
                );
            }
            g
        };
        modal_overlay(t, cx, |this| this.shortcuts_open = false).child(
            float_surface(&t)
                .w(px(580.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .p(px(22.))
                .rounded(px(12.))
                .child(
                    div()
                        .text_size(px(16.))
                        .font_family(FONT_SERIF)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("键盘快捷键"),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(24.))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(12.))
                                .child(group(
                                    "通用",
                                    vec![
                                        ("命令面板", "Ctrl+K"),
                                        ("新建会话", "Ctrl+Shift+N"),
                                        ("关闭浮层", "Esc"),
                                    ],
                                ))
                                .child(group("视图", vec![("切换底部面板", "Ctrl+J")])),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(12.))
                                .child(group(
                                    "Composer",
                                    vec![("发送", "Enter"), ("发送（设置）", "Ctrl+Enter")],
                                ))
                                .child(group(
                                    "编辑",
                                    vec![("复制 / 粘贴", "Ctrl+C / V"), ("全选", "Ctrl+A")],
                                )),
                        ),
                ),
        )
    }
}

/// Dimmed modal backdrop (`rgba(20,18,15,0.32)`), click outside to close.
fn modal_overlay(
    t: moonlit_uikit::Tokens,
    cx: &mut Context<AgentIdeApp>,
    on_close: fn(&mut AgentIdeApp),
) -> gpui::Div {
    let _ = t;
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x14120f52))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                on_close(this);
                cx.notify();
            }),
        )
}
