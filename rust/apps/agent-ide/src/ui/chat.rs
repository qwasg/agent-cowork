//! 360px chat column mirroring the legacy `ChatColumn`: 38px head with title /
//! provider chip / mode chip / icon buttons, message stream (right-aligned
//! white cards for the user, full-width bubble-less assistant messages with a
//! 22px 「月」avatar and an `AgentActionTimeline`), and the composer host at
//! the bottom.
//!
//! Timeline display rules (Cursor-style, text-first):
//! - The final answer / summary text is always rendered in full as plain
//!   markdown — it is the deliverable and never auto-collapses.
//! - Intermediate narration text renders inline as plain markdown too.
//! - Every other step (reasoning / tools / subagents) collapses to a single
//!   gray text line with no icon, chevron or card frame; clicking a line
//!   expands its detail beneath it.
//! - Consecutive completed same-kind tool calls aggregate into one text line
//!   （「读取 5 个文件」）; the lone pulse dot marks a still-running step.

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, MouseDownEvent};
use moonlit_uikit::{compute_line_diff, DiffTag, ToastKind, FONT_MONO_FALLBACK, FONT_SERIF};

use super::composer::composer_mode_meta;
use super::icons::icon;
use super::{chip, float_surface, ibtn, sh1, status_dot};
use crate::app::{AgentIdeApp, ContextApplyTarget};
use crate::{BlockStatus, ChatBlock, ChatMessage, ChatRole, MessageStatus};

/// Non-final answer texts longer than this collapse to 140px behind a manual
/// 展开/收起 toggle (legacy `.msg-agent-text--collapsed`).
const COLLAPSE_CHARS: usize = 700;

impl AgentIdeApp {
    pub(crate) fn render_chat_column(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let mut col = div()
            .relative()
            .w(px(self.pane_w.1))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(self.render_chat_head(cx))
            .child(self.render_chat_body(cx))
            .child(self.render_composer(cx));
        if self.subagent_overlay.is_some() {
            col = col.child(self.render_subagent_overlay(cx));
        }
        col
    }

    /// `SubagentOverlay`: lightweight detail card floating above the composer.
    fn render_subagent_overlay(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let Some(target) = self.subagent_overlay.clone() else {
            return div().into_any_element();
        };
        let block = self.state.chat.messages.iter().rev().find_map(|m| {
            m.blocks.iter().find_map(|b| match b {
                ChatBlock::Subagent {
                    id,
                    label,
                    prompt,
                    summary,
                    status,
                    ..
                } if *id == target => {
                    Some((label.clone(), prompt.clone(), summary.clone(), *status))
                }
                _ => None,
            })
        });
        let Some((label, prompt, summary, status)) = block else {
            return div().into_any_element();
        };
        let (badge, badge_color, badge_bg) = match status {
            BlockStatus::Running => ("运行中", t.accent, t.accent_bg),
            BlockStatus::Done => ("已完成", t.sage, t.bg_sunk),
            BlockStatus::Error => ("失败", t.danger, t.bg_sunk),
        };
        float_surface(&t)
            .absolute()
            .bottom(px(118.))
            .left(px(14.))
            .right(px(14.))
            .flex()
            .flex_col()
            .max_h(px(360.))
            .rounded(px(10.))
            .shadow(vec![gpui::BoxShadow {
                color: gpui::rgba(0x2a272426).into(),
                offset: gpui::point(px(0.), px(-10.)),
                blur_radius: px(30.),
                spread_radius: px(0.),
            }])
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .w(px(20.))
                            .h(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(t.accent_bg)
                            .child(icon("git-fork", 11., t.accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .bg(badge_bg)
                            .text_size(px(10.5))
                            .text_color(badge_color)
                            .child(badge),
                    )
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child(icon("x", 12., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.subagent_overlay = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("subagent-overlay-body")
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(12.))
                    .max_h(px(316.))
                    .overflow_y_scroll()
                    .when(!prompt.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(t.text_4)
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("PROMPT"),
                                )
                                .child(
                                    div()
                                        .px(px(10.))
                                        .py(px(8.))
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(t.line)
                                        .bg(t.bg_sunk)
                                        .text_size(px(12.))
                                        .text_color(t.text_2)
                                        .child(prompt),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(t.text_4)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("SUMMARY"),
                            )
                            .child(if summary.is_empty() {
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.text_4)
                                    .child(if status == BlockStatus::Running {
                                        "子 agent 正在工作，完成后会在这里显示摘要。"
                                    } else {
                                        "无摘要输出。"
                                    })
                                    .into_any_element()
                            } else {
                                render_markdown_flat(&summary, &t, false)
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_chat_head(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let busy = self.state.chat.active_run_id.is_some();
        let session = self
            .state
            .active_session_id
            .as_ref()
            .and_then(|id| self.state.sessions.iter().find(|s| &s.id == id));
        let title = session
            .map(|s| {
                if s.title.is_empty() {
                    s.id.clone()
                } else {
                    s.title.clone()
                }
            })
            .unwrap_or_else(|| "会话".to_string());
        let provider_chip = match &self.provider {
            Some((mode, _)) if mode == "live" => "Live",
            Some((mode, _)) if mode == "mock" => "Mock",
            _ => "offline",
        };
        let mode_chip = session
            .and_then(|s| s.mode.clone())
            .unwrap_or_else(|| self.mode.as_str().to_string());

        div()
            .relative()
            .h(px(38.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .min_w(px(0.))
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if busy {
                        super::pulse_dot("chat-head-dot", t.dot_running).into_any_element()
                    } else {
                        status_dot(t.dot_done).into_any_element()
                    })
                    .child(div().truncate().child(title)),
            )
            .child(chip(provider_chip, &t))
            .child(chip(mode_chip, &t))
            .child(div().flex_1())
            .child(ibtn(
                "git-branch",
                13.,
                &t,
                |this, _w, cx| {
                    this.fork_session(cx);
                    this.toast("已基于当前会话创建分支", ToastKind::Info, cx);
                },
                cx,
            ))
            .child(ibtn(
                "history",
                13.,
                &t,
                |this, _w, cx| {
                    this.chathead_menu = if this.chathead_menu == Some("replay") {
                        None
                    } else {
                        Some("replay")
                    };
                    cx.notify();
                },
                cx,
            ))
            .child(ibtn(
                "more-horizontal",
                13.,
                &t,
                |this, _w, cx| {
                    this.chathead_menu = if this.chathead_menu == Some("more") {
                        None
                    } else {
                        Some("more")
                    };
                    cx.notify();
                },
                cx,
            ))
            .when(self.chathead_menu == Some("replay"), |d| {
                d.child(self.render_replay_menu(cx))
            })
            .when(self.chathead_menu == Some("more"), |d| {
                d.child(self.render_more_menu(cx))
            })
    }

    /// Replay dropdown: pick a user message to revert the session to.
    fn render_replay_menu(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let users: Vec<(String, String)> = self
            .state
            .chat
            .messages
            .iter()
            .filter(|m| m.role == ChatRole::User)
            .map(|m| (m.id.clone(), m.text.chars().take(24).collect::<String>()))
            .collect();
        let mut menu = float_surface(&t)
            .absolute()
            .top(px(36.))
            .right(px(8.))
            .w(px(220.))
            .max_h(px(280.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .overflow_hidden();
        if users.is_empty() {
            menu = menu.child(
                div()
                    .p(px(10.))
                    .text_size(px(11.5))
                    .text_color(t.text_4)
                    .child("暂无可回退的消息"),
            );
        }
        for (id, preview) in users.into_iter().rev().take(8) {
            menu = menu.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg_selection))
                    .child(icon("history", 11., t.text_3))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(format!("回退到：{preview}")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.chathead_menu = None;
                            this.revert_to(id.clone(), cx);
                        }),
                    ),
            );
        }
        menu.into_any_element()
    }

    /// More menu: Fork Chat / Copy Messages / Copy Request ID.
    fn render_more_menu(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        float_surface(&t)
            .absolute()
            .top(px(36.))
            .right(px(8.))
            .w(px(180.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .child(
                super::menu_item("git-branch", "Fork Chat", false, &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.chathead_menu = None;
                        this.fork_session(cx);
                    }),
                ),
            )
            .child(
                super::menu_item("copy", "Copy Messages", false, &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.chathead_menu = None;
                        this.copy_messages(cx);
                    }),
                ),
            )
            .child(
                super::menu_item("copy", "Copy Request ID", false, &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.chathead_menu = None;
                        let id = this
                            .state
                            .chat
                            .active_run_id
                            .clone()
                            .or_else(|| this.state.active_session_id.clone())
                            .unwrap_or_default();
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(id));
                        this.toast("已复制", ToastKind::Success, cx);
                    }),
                ),
            )
            .into_any_element()
    }

    fn render_chat_body(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let messages = self.state.chat.messages.clone();
        let model_label = self
            .selected_model
            .as_ref()
            .map(|id| self.model_display_label(id))
            .or_else(|| self.provider.as_ref().map(|(_, model)| model.clone()))
            .unwrap_or_default();
        let last_idx = messages.len().saturating_sub(1);
        div()
            .id("chat-body")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .pt(px(16.))
            .px(px(16.))
            .pb(px(8.))
            .overflow_y_scroll()
            .when(messages.is_empty(), |d| {
                d.child(
                    div()
                        .p(px(24.))
                        .text_size(px(12.))
                        .text_color(t.text_3)
                        .text_center()
                        .child("尚无任何消息。选择左侧会话后发送，或先点击「新建」创建会话。"),
                )
            })
            .children(messages.iter().enumerate().map(|(i, m)| match m.role {
                ChatRole::User => self.render_user_message(m, cx),
                _ => self.render_agent_message(m, i == last_idx, &model_label, cx),
            }))
    }

    /// User message as an editable composer-style card (Cursor parity):
    /// text on top, then a bottom bar with mode / model chips and a send
    /// button. Clicking the card enters edit mode; resending reverts the
    /// session to before this message and re-runs with the new text.
    fn render_user_message(&self, m: &ChatMessage, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let editing = self.editing_msg.as_deref() == Some(m.id.as_str());
        let model_label = self
            .selected_model
            .as_deref()
            .map(|id| self.model_display_label(id))
            .or_else(|| self.provider.as_ref().map(|(_, model)| model.clone()))
            .unwrap_or_else(|| "default".to_string());
        let (mode_icon, mode_label) = composer_mode_meta(&self.mode);

        let mut card = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .pt(px(10.))
            .pb(px(8.))
            .rounded(px(12.))
            .bg(t.bg_panel)
            .border_1()
            .border_color(if editing { t.accent_ring } else { t.line })
            .shadow(sh1());

        // ---- head: timestamp + edit hint -----------------------------------
        if !m.time.is_empty() || editing {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .text_color(t.text_3)
                    .when(!m.time.is_empty(), |d| d.child(m.time.clone()))
                    .child(div().flex_1())
                    .when(editing, |d| d.child("编辑后重新发送，将回退此后的对话")),
            );
        }

        // ---- body: static text or the inline editor ------------------------
        if editing {
            card = card.child(div().text_size(px(13.5)).child(self.edit_input.clone()));
        } else {
            let edit_id = m.id.clone();
            card = card.child(
                div()
                    .text_size(px(13.))
                    .cursor_pointer()
                    .child(m.text.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, w, cx| {
                            this.start_edit_message(edit_id.clone(), w, cx);
                        }),
                    ),
            );
        }

        // ---- bottom bar: mode chip / model chip / actions -------------------
        let mode_chip = div()
            .h(px(20.))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .rounded_full()
            .bg(t.bg_sunk)
            .text_size(px(11.))
            .text_color(t.text_3)
            .child(icon(mode_icon, 11., t.text_3))
            .child(mode_label);
        let mode_chip = if editing {
            mode_chip
                .cursor_pointer()
                .border_1()
                .border_color(if self.edit_add_menu_open {
                    t.accent_ring
                } else {
                    t.line
                })
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon("chevron-down", 10., t.text_4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.edit_add_menu_open = !this.edit_add_menu_open;
                        this.edit_model_menu_open = false;
                        this.add_menu_open = false;
                        this.model_menu_open = false;
                        cx.notify();
                    }),
                )
        } else {
            let edit_id = m.id.clone();
            mode_chip
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon("chevron-down", 10., t.text_4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, w, cx| {
                        if this.start_edit_message(edit_id.clone(), w, cx) {
                            this.edit_add_menu_open = true;
                            this.edit_model_menu_open = false;
                            this.add_menu_open = false;
                            this.model_menu_open = false;
                            cx.notify();
                        }
                    }),
                )
        };
        let model_chip = div()
            .h(px(20.))
            .px(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .rounded(px(6.))
            .text_size(px(11.))
            .text_color(t.text_3)
            .child(icon("sparkles", 11., t.text_3))
            .child(div().max_w(px(140.)).truncate().child(model_label));
        let model_chip = if editing {
            model_chip
                .cursor_pointer()
                .border_1()
                .border_color(if self.edit_model_menu_open {
                    t.accent_ring
                } else {
                    gpui::rgba(0x00000000)
                })
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon("chevron-down", 10., t.text_4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.edit_model_menu_open = !this.edit_model_menu_open;
                        this.edit_add_menu_open = false;
                        this.add_menu_open = false;
                        this.model_menu_open = false;
                        cx.notify();
                    }),
                )
        } else {
            let edit_id = m.id.clone();
            model_chip
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon("chevron-down", 10., t.text_4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, w, cx| {
                        if this.start_edit_message(edit_id.clone(), w, cx) {
                            this.edit_model_menu_open = true;
                            this.edit_add_menu_open = false;
                            this.add_menu_open = false;
                            this.model_menu_open = false;
                            cx.notify();
                        }
                    }),
                )
        };
        let context_button = if editing {
            div()
                .w(px(24.))
                .h(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon("paperclip", 12., t.text_3))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                        this.context_apply_target = ContextApplyTarget::EditMessage;
                        this.context_drawer_open = true;
                        this.edit_add_menu_open = false;
                        this.edit_model_menu_open = false;
                        this.add_menu_open = false;
                        this.model_menu_open = false;
                        cx.notify();
                    }),
                )
                .into_any_element()
        } else {
            let edit_id = m.id.clone();
            div()
                .w(px(24.))
                .h(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon("paperclip", 12., t.text_4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, w, cx| {
                        if this.start_edit_message(edit_id.clone(), w, cx) {
                            this.context_apply_target = ContextApplyTarget::EditMessage;
                            this.context_drawer_open = true;
                            this.edit_add_menu_open = false;
                            this.edit_model_menu_open = false;
                            this.add_menu_open = false;
                            this.model_menu_open = false;
                            cx.notify();
                        }
                    }),
                )
                .into_any_element()
        };

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .pt(px(2.))
            .child(mode_chip)
            .child(model_chip)
            .child(div().flex_1())
            .child(context_button);

        if editing {
            // cancel (x) + accent send
            bar = bar
                .child(
                    div()
                        .w(px(24.))
                        .h(px(24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(t.line)
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.bg_hover))
                        .child(icon("x", 11., t.text_3))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                this.cancel_edit(cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .w(px(24.))
                        .h(px(24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(t.accent)
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.accent_soft))
                        .child(icon("arrow-up", 12., t.text_inv))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                this.resend_edited(cx);
                            }),
                        ),
                );
        } else {
            // muted send affordance (activates on click via edit mode)
            bar = bar.child({
                let edit_id = m.id.clone();
                div()
                    .w(px(24.))
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(t.bg_active)
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg_hover))
                    .child(icon("arrow-up", 12., t.text_3))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, w, cx| {
                            this.start_edit_message(edit_id.clone(), w, cx);
                        }),
                    )
            });
        }
        card = card.child(bar);
        if editing {
            card = card
                .when(self.edit_add_menu_open, |d| {
                    d.child(self.render_edit_mode_menu(cx))
                })
                .when(self.edit_model_menu_open, |d| {
                    d.child(self.render_edit_model_menu(cx))
                });
        }

        if !editing {
            card = card.hover(move |s| s.border_color(t.accent_ring));
        }

        card.into_any_element()
    }

    /// Mirrors the composer `+` menu (shared agent-kind + mode sections) so
    /// both dropdowns always expose the same options.
    fn render_edit_mode_menu(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let mut menu = float_surface(&t)
            .mt(px(2.))
            .min_w(px(196.))
            .max_w(px(240.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(10.));
        menu = menu.children(self.agent_kind_menu_items(cx));
        menu = menu.child(div().h(px(1.)).my(px(4.)).bg(t.line));
        menu = menu.children(self.mode_menu_items(cx));
        menu.into_any_element()
    }

    fn render_edit_model_menu(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let selected = self
            .selected_model
            .clone()
            .or_else(|| self.provider.as_ref().map(|(_, m)| m.clone()));
        let mut menu = float_surface(&t)
            .mt(px(2.))
            .min_w(px(214.))
            .max_h(px(260.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(10.))
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
            let title = if m.label.is_empty() {
                m.id.clone()
            } else {
                m.label.clone()
            };
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
                            this.pick_model(id.clone(), cx);
                            this.edit_model_menu_open = false;
                        }),
                    ),
            );
        }
        menu.into_any_element()
    }

    /// `.msg-agent`: 11px head (avatar / Agent / dot / model label / time) +
    /// the action timeline.
    fn render_agent_message(
        &self,
        m: &ChatMessage,
        _is_last: bool,
        model_label: &str,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let streaming = m.status == MessageStatus::Streaming;
        let status_color = match m.status {
            MessageStatus::Streaming => t.dot_running,
            MessageStatus::Failed => t.dot_blocked,
            MessageStatus::Cancelled => t.dot_idle,
            MessageStatus::Completed => t.dot_done,
        };
        let mut body = div().flex().flex_col().gap(px(4.));
        // ---- head: avatar 月 / Agent / dot / model / time ----------------------
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .mb(px(2.))
                .text_size(px(11.))
                .text_color(t.text_3)
                .child(
                    div()
                        .w(px(22.))
                        .h(px(22.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(t.text)
                        .text_color(t.text_inv)
                        .text_size(px(11.))
                        .font_family(FONT_SERIF)
                        .child("月"),
                )
                .child("Agent")
                .child(status_dot(status_color))
                .when(!model_label.is_empty(), |d| {
                    d.child(model_label.to_string())
                })
                .child(div().flex_1())
                .when(!m.time.is_empty(), |d| d.child(m.time.clone())),
        );

        // ---- action timeline ----------------------------------------------------
        // Fallback for snapshot-hydrated messages without blocks.
        let blocks: Vec<ChatBlock> = if m.blocks.is_empty() {
            let mut v = Vec::new();
            if !m.reasoning.is_empty() {
                v.push(ChatBlock::Reasoning {
                    text: m.reasoning.clone(),
                });
            }
            if !m.text.is_empty() {
                v.push(ChatBlock::Text {
                    text: m.text.clone(),
                });
            }
            v
        } else {
            m.blocks.clone()
        };
        let last_block = blocks.len().saturating_sub(1);
        // The final answer can be followed by bookkeeping/tool-close events, so
        // treat the last Text block in the message as the final summary.
        let final_text_idx = blocks
            .iter()
            .rposition(|b| matches!(b, ChatBlock::Text { .. }));

        // Milestone blocks (reasoning / todo updates / subagents / narration)
        // render standalone; consecutive plain tool calls merge into an
        // activity segment summarized by a single gray text line
        // （「编辑 3 个文件，2 次搜索 +73 -15」）. The timeline stays flat: there is
        // no per-turn fold — only the final answer renders as full markdown.
        let timeline = build_timeline(&blocks);

        for item in &timeline {
            match item {
                TimelineItem::Block(bi) => {
                    let bi = *bi;
                    let block = &blocks[bi];
                    let is_trailing = bi == last_block;
                    match block {
                        ChatBlock::Text { text } if Some(bi) == final_text_idx => {
                            // Final answer: plain markdown, always visible.
                            body = body.child(self.render_text_block(
                                &m.id,
                                text,
                                streaming && is_trailing,
                                cx,
                            ));
                        }
                        ChatBlock::Text { text } => {
                            // Intermediate narration: inline plain markdown, no
                            // card wrapper.
                            body = body.child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(t.text_2)
                                    .child(render_markdown_flat(text, &t, false)),
                            );
                        }
                        ChatBlock::Reasoning { text } => {
                            let key = format!("{}:{}", m.id, bi);
                            let live = streaming && is_trailing;
                            let detail = div()
                                .text_size(px(12.))
                                .text_color(t.text_4)
                                .child(text.clone())
                                .into_any_element();
                            body = body.child(self.render_summary_line(
                                key,
                                format!("思考 · {}", reasoning_summary(text)),
                                0,
                                0,
                                0,
                                live,
                                Some(detail),
                                cx,
                            ));
                        }
                        ChatBlock::Tool {
                            id,
                            name,
                            args,
                            result,
                            status,
                            mcp,
                        } if name == "write_todos" => {
                            // Todo update milestone.
                            let key = format!("tool:{id}");
                            let detail =
                                self.render_tool_body(args, result, *status, mcp.is_some());
                            body = body.child(self.render_summary_line(
                                key,
                                format!("待办 · {}", todo_milestone_label(args)),
                                0,
                                0,
                                0,
                                *status == BlockStatus::Running,
                                Some(detail),
                                cx,
                            ));
                        }
                        ChatBlock::Tool { .. } => {
                            // Standalone tool (ordinary tools normally live
                            // inside activity segments).
                            body = body.child(self.render_tool_line(block, cx));
                        }
                        ChatBlock::Subagent {
                            id, label, status, ..
                        } => {
                            body = body.child(self.render_subagent_card(
                                id,
                                label,
                                *status,
                                model_label,
                                cx,
                            ));
                        }
                    }
                }
                TimelineItem::Activity(indices) => {
                    body = body.child(self.render_activity_segment(&m.id, &blocks, indices, cx));
                }
            }
        }

        // streaming caret at the very end (legacy `.stream-caret`, 1s blink)
        if streaming {
            body = body.child(super::stream_caret("stream-caret", t.accent));
        }
        body.into_any_element()
    }

    /// Pure-text timeline row (Cursor-style): one gray line with no icon,
    /// chevron or card frame. Optional inline `+N` / `-N` diff counts and an
    /// error suffix sit right after the text; a running step carries the lone
    /// pulse dot. When `detail` is present the row is clickable and expands the
    /// detail beneath it (indented, frameless).
    #[allow(clippy::too_many_arguments)]
    fn render_summary_line(
        &self,
        key: String,
        text: String,
        added: usize,
        removed: usize,
        errors: usize,
        running: bool,
        detail: Option<AnyElement>,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let expandable = detail.is_some();
        let expanded = expandable && *self.expanded_blocks.get(&key).unwrap_or(&false);
        let toggle_key = key.clone();
        let next = !expanded;

        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .py(px(2.))
            .text_size(px(12.5))
            .text_color(if errors > 0 { t.danger } else { t.text_3 })
            .child(div().min_w(px(0.)).overflow_hidden().truncate().child(text));
        if added > 0 {
            row = row.child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(t.sage)
                    .child(format!("+{added}")),
            );
        }
        if removed > 0 {
            row = row.child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(t.danger)
                    .child(format!("-{removed}")),
            );
        }
        if errors > 0 {
            row = row.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(t.danger)
                    .child(format!("· {errors} 失败")),
            );
        }
        if running {
            row = row.child(super::pulse_dot(
                gpui::SharedString::from(format!("{key}:dot")),
                t.dot_running,
            ));
        }
        if expandable {
            row = row
                .cursor_pointer()
                .hover(move |s| s.text_color(t.text_2))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.expanded_blocks.insert(toggle_key.clone(), next);
                        cx.notify();
                    }),
                );
        }
        let mut wrap = div().flex().flex_col().child(row);
        if expanded {
            if let Some(detail) = detail {
                wrap = wrap.child(
                    div()
                        .ml(px(8.))
                        .pl(px(8.))
                        .pt(px(2.))
                        .pb(px(4.))
                        .child(detail),
                );
            }
        }
        wrap.into_any_element()
    }

    /// Cross-kind activity summary row（「编辑 3 个文件，2 次搜索，执行 1 条
    /// 命令 +73 -15」）. Click expands to the individual tool cards; while a
    /// tool inside the segment is still running the row carries a pulse dot
    /// plus the current action label.
    fn render_activity_segment(
        &self,
        msg_id: &str,
        blocks: &[ChatBlock],
        indices: &[usize],
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let stats = segment_stats(blocks, indices);
        let key = indices
            .first()
            .and_then(|&bi| match &blocks[bi] {
                ChatBlock::Tool { id, .. } => Some(format!("seg:{id}")),
                _ => None,
            })
            .unwrap_or_else(|| format!("seg:{msg_id}:{}", indices.first().copied().unwrap_or(0)));
        // Current action label when the trailing tool is still running.
        let running_label = indices.iter().rev().find_map(|&bi| match &blocks[bi] {
            ChatBlock::Tool {
                status: BlockStatus::Running,
                name,
                mcp,
                ..
            } => Some(format!("正在{}…", tool_visual(name, mcp).1)),
            _ => None,
        });
        let running = running_label.is_some();
        let text = match &running_label {
            Some(label) => format!("{} · {label}", segment_phrase(&stats)),
            None => segment_phrase(&stats),
        };

        // Expanded detail: one frameless text line per tool / aggregated run.
        let mut list = div().flex().flex_col();
        for run in group_segment_items(blocks, indices) {
            if run.len() >= 2 {
                let mut first_id = String::new();
                let mut phrase = String::new();
                if let Some(ChatBlock::Tool { id, name, mcp, .. }) =
                    run.first().map(|&bi| &blocks[bi])
                {
                    first_id = id.clone();
                    phrase = group_phrase(name, mcp, run.len());
                }
                let errors = run
                    .iter()
                    .filter(|&&bi| {
                        matches!(
                            &blocks[bi],
                            ChatBlock::Tool {
                                status: BlockStatus::Error,
                                ..
                            }
                        )
                    })
                    .count();
                let mut inner = div().flex().flex_col();
                for &bi in &run {
                    inner = inner.child(self.render_tool_line(&blocks[bi], cx));
                }
                list = list.child(self.render_summary_line(
                    format!("group:{first_id}"),
                    phrase,
                    0,
                    0,
                    errors,
                    false,
                    Some(inner.into_any_element()),
                    cx,
                ));
                continue;
            }
            let Some(&bi) = run.first() else { continue };
            list = list.child(self.render_tool_line(&blocks[bi], cx));
        }

        self.render_summary_line(
            key,
            text,
            stats.added,
            stats.removed,
            stats.errors,
            running,
            Some(list.into_any_element()),
            cx,
        )
    }

    /// One tool rendered as a frameless gray text line（「读取文件 · a.rs」）;
    /// expands to its args / result body.
    fn render_tool_line(&self, block: &ChatBlock, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let ChatBlock::Tool {
            id,
            name,
            args,
            result,
            status,
            mcp,
        } = block
        else {
            return div().into_any_element();
        };
        let (_, label) = tool_visual(name, mcp);
        let summary = tool_summary(args, result, *status);
        let line = if summary.is_empty() {
            label
        } else {
            format!("{label} · {summary}")
        };
        let body = self.render_tool_body(args, result, *status, mcp.is_some());
        self.render_summary_line(
            format!("tool:{id}"),
            line,
            0,
            0,
            usize::from(*status == BlockStatus::Error),
            *status == BlockStatus::Running,
            Some(body),
            cx,
        )
    }

    /// Tool detail body: frameless mono args + result, max 320px tall, shown
    /// only when a tool line is expanded.
    fn render_tool_body(
        &self,
        args: &str,
        result: &str,
        status: BlockStatus,
        _mcp: bool,
    ) -> AnyElement {
        let t = self.t;
        let boxed = |text: String| {
            let omitted = text.lines().count().saturating_sub(60);
            div()
                .max_h(px(320.))
                .overflow_hidden()
                .font_family(FONT_MONO_FALLBACK)
                .text_size(px(11.5))
                .text_color(t.text_4)
                .children(
                    text.lines()
                        .take(60)
                        .map(|l| div().child(l.to_string()))
                        .collect::<Vec<_>>(),
                )
                .when(omitted > 0, |d| {
                    d.child(
                        div()
                            .text_color(t.text_4)
                            .child(format!("（+{omitted} 行已省略）")),
                    )
                })
        };
        let mut body = div().flex().flex_col().gap(px(6.));
        if !args.is_empty() {
            body = body.child(boxed(args.to_string()));
        }
        if result.is_empty() {
            body = body.child(div().text_size(px(11.5)).text_color(t.text_4).child(
                if status == BlockStatus::Running {
                    "运行中…"
                } else {
                    "无输出"
                },
            ));
        } else {
            body = body.child(boxed(result.to_string()));
        }
        body.into_any_element()
    }

    /// Subagent task as a single gray text line; click opens the detail card
    /// above the composer. A running subagent carries the lone pulse dot.
    fn render_subagent_card(
        &self,
        id: &str,
        label: &str,
        status: BlockStatus,
        _model_label: &str,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let (status_color, status_text) = match status {
            BlockStatus::Running => (t.accent, "运行中"),
            BlockStatus::Done => (t.sage, "已完成"),
            BlockStatus::Error => (t.danger, "失败"),
        };
        let open_id = id.to_string();
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .py(px(2.))
            .text_size(px(12.5))
            .text_color(t.text_3)
            .cursor_pointer()
            .hover(move |s| s.text_color(t.text_2))
            .child(div().flex_none().child("子任务"))
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .truncate()
                    .text_color(t.text_2)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(status_color)
                    .child(format!("· {status_text}")),
            );
        if status == BlockStatus::Running {
            row = row.child(super::pulse_dot(
                gpui::SharedString::from(format!("subagent:{id}:dot")),
                status_color,
            ));
        }
        row.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                this.subagent_overlay = Some(open_id.clone());
                cx.notify();
            }),
        )
        .into_any_element()
    }

    /// Assistant final-answer text block. It renders as plain markdown without
    /// a wrapper frame and stays expanded by default; users can manually collapse
    /// it to a 280px preview and expand it again.
    fn render_text_block(
        &self,
        msg_id: &str,
        text: &str,
        streaming: bool,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let content = render_markdown_flat(text, &t, streaming);
        let can_collapse = text.chars().count() > COLLAPSE_CHARS && !streaming;
        if !can_collapse {
            return content;
        }
        let collapsed = self.collapsed_msgs.contains(msg_id);
        let toggle_id = msg_id.to_string();
        let toggle_label = if collapsed {
            "展开全文 ↓"
        } else {
            "收起 ↑"
        };
        div()
            .flex()
            .flex_col()
            .child(if collapsed {
                div()
                    .max_h(px(280.))
                    .overflow_hidden()
                    .child(content)
                    .into_any_element()
            } else {
                content
            })
            .child(
                div()
                    .pt(px(4.))
                    .text_size(px(11.))
                    .text_color(t.accent)
                    .cursor_pointer()
                    .child(toggle_label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            if !this.collapsed_msgs.remove(&toggle_id) {
                                this.collapsed_msgs.insert(toggle_id.clone());
                            }
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }
}

// ---- collapsed-summary helpers (frontend-local, no backend involvement) ----

/// `(icon, 中文动词标签, group_prefix, group_suffix)` for builtin tool names.
fn tool_meta(name: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    Some(match name {
        "read_file" => ("file-text", "读取文件", "读取", "个文件"),
        "list_dir" => ("folder", "浏览目录", "浏览", "个目录"),
        "grep" | "glob" | "codebase_search" | "semantic_search" => {
            ("search", "搜索代码", "搜索", "次")
        }
        "run_command" => ("terminal", "执行命令", "执行", "条命令"),
        "check_command_status" | "stop_command" => ("terminal", "管理命令", "管理命令", "次"),
        "write_file" | "create_document" => ("pencil", "写入文件", "写入", "个文件"),
        "str_replace_edit" | "apply_patch" | "edit_file" => {
            ("pencil", "编辑文件", "编辑", "个文件")
        }
        "delete_file" => ("trash-2", "删除文件", "删除", "个文件"),
        "web_search" => ("globe", "联网搜索", "联网搜索", "次"),
        "web_fetch" => ("globe", "抓取网页", "抓取", "个网页"),
        "read_skill" => ("book-open", "读取技能", "读取", "个技能"),
        "write_todos" => ("list-checks", "更新待办", "更新待办", "次"),
        _ => return None,
    })
}

/// Icon + single-call head label for a tool block.
fn tool_visual(name: &str, mcp: &Option<(String, String)>) -> (&'static str, String) {
    if let Some((server, tool)) = mcp {
        return ("plug", format!("{server} / {tool}"));
    }
    match tool_meta(name) {
        Some((icon, label, _, _)) => (icon, label.to_string()),
        None => ("terminal", name.to_string()),
    }
}

/// Grouping bucket: consecutive completed tools sharing this key aggregate
/// into one compact row.
fn tool_group_key(name: &str, mcp: &Option<(String, String)>) -> String {
    if let Some((server, tool)) = mcp {
        return format!("mcp:{server}/{tool}");
    }
    match tool_meta(name) {
        Some((_, label, _, _)) => format!("kind:{label}"),
        None => format!("name:{name}"),
    }
}

/// 「读取 5 个文件」-style phrase for an aggregated tool row.
fn group_phrase(name: &str, mcp: &Option<(String, String)>, n: usize) -> String {
    if let Some((server, tool)) = mcp {
        return format!("{server} / {tool} ×{n}");
    }
    match tool_meta(name) {
        Some((_, _, prefix, suffix)) => format!("{prefix} {n} {suffix}"),
        None => format!("{name} ×{n}"),
    }
}

// ---- summary-timeline helpers (segmentation / stats / diff estimation) ----

/// One renderable unit of the assistant timeline: a standalone milestone
/// block, or a run of consecutive plain tool calls merged into an activity
/// segment.
enum TimelineItem {
    Block(usize),
    Activity(Vec<usize>),
}

/// Milestones break activity segments: narration / reasoning / subagents and
/// todo updates each render as their own row.
fn is_milestone_block(block: &ChatBlock) -> bool {
    match block {
        ChatBlock::Text { .. } | ChatBlock::Reasoning { .. } | ChatBlock::Subagent { .. } => true,
        ChatBlock::Tool { name, .. } => name == "write_todos",
    }
}

fn build_timeline(blocks: &[ChatBlock]) -> Vec<TimelineItem> {
    let mut items: Vec<TimelineItem> = Vec::new();
    let mut segment: Vec<usize> = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if is_milestone_block(block) {
            if !segment.is_empty() {
                items.push(TimelineItem::Activity(std::mem::take(&mut segment)));
            }
            items.push(TimelineItem::Block(i));
        } else {
            segment.push(i);
        }
    }
    if !segment.is_empty() {
        items.push(TimelineItem::Activity(segment));
    }
    items
}

/// Same-kind aggregation inside an expanded activity segment: consecutive
/// completed tools sharing a group key form one run (rendered as a single
/// 「读取 5 个文件」line when >= 2).
fn group_segment_items(blocks: &[ChatBlock], indices: &[usize]) -> Vec<Vec<usize>> {
    let mut runs: Vec<Vec<usize>> = Vec::new();
    let mut i = 0usize;
    while i < indices.len() {
        let bi = indices[i];
        if let ChatBlock::Tool {
            status, name, mcp, ..
        } = &blocks[bi]
        {
            if *status != BlockStatus::Running {
                let gk = tool_group_key(name, mcp);
                let mut j = i + 1;
                while j < indices.len() {
                    match &blocks[indices[j]] {
                        ChatBlock::Tool {
                            status: s2,
                            name: n2,
                            mcp: mcp2,
                            ..
                        } if *s2 != BlockStatus::Running && tool_group_key(n2, mcp2) == gk => {
                            j += 1;
                        }
                        _ => break,
                    }
                }
                runs.push(indices[i..j].to_vec());
                i = j;
                continue;
            }
        }
        runs.push(vec![bi]);
        i += 1;
    }
    runs
}

/// Buckets feeding the cross-kind activity summary phrase.
enum ToolCategory {
    Edit,
    Explore,
    Search,
    Command,
    Other,
}

fn tool_category(name: &str, mcp: &Option<(String, String)>) -> ToolCategory {
    if mcp.is_some() {
        return ToolCategory::Other;
    }
    match name {
        "str_replace_edit" | "apply_patch" | "edit_file" | "write_file" | "create_document"
        | "delete_file" => ToolCategory::Edit,
        "read_file" | "list_dir" => ToolCategory::Explore,
        "grep" | "glob" | "codebase_search" | "semantic_search" | "web_search" => {
            ToolCategory::Search
        }
        "run_command" | "check_command_status" | "stop_command" => ToolCategory::Command,
        _ => ToolCategory::Other,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SegmentStats {
    /// Distinct files touched by edit tools.
    edits: usize,
    /// Distinct files / directories opened by explore tools.
    explores: usize,
    searches: usize,
    commands: usize,
    others: usize,
    added: usize,
    removed: usize,
    errors: usize,
}

fn segment_stats(blocks: &[ChatBlock], indices: &[usize]) -> SegmentStats {
    let mut stats = SegmentStats::default();
    let mut edit_files: std::collections::HashSet<String> = Default::default();
    let mut explore_targets: std::collections::HashSet<String> = Default::default();
    for &bi in indices {
        let ChatBlock::Tool {
            name,
            args,
            status,
            mcp,
            ..
        } = &blocks[bi]
        else {
            continue;
        };
        if *status == BlockStatus::Error {
            stats.errors += 1;
        }
        match tool_category(name, mcp) {
            ToolCategory::Edit => {
                let files = edit_target_files(name, args);
                if files.is_empty() {
                    edit_files.insert(format!("#{bi}"));
                } else {
                    edit_files.extend(files);
                }
                let (added, removed) = tool_diff_stats(name, args);
                stats.added += added;
                stats.removed += removed;
            }
            ToolCategory::Explore => {
                let target = json_arg_str(args, "path").unwrap_or_else(|| format!("#{bi}"));
                explore_targets.insert(target);
            }
            ToolCategory::Search => stats.searches += 1,
            ToolCategory::Command => stats.commands += 1,
            ToolCategory::Other => stats.others += 1,
        }
    }
    stats.edits = edit_files.len();
    stats.explores = explore_targets.len();
    stats
}

/// 「编辑 3 个文件，探索 4 个文件，3 次搜索，执行 4 条命令」.
fn segment_phrase(stats: &SegmentStats) -> String {
    let mut parts: Vec<String> = Vec::new();
    if stats.edits > 0 {
        parts.push(format!("编辑 {} 个文件", stats.edits));
    }
    if stats.explores > 0 {
        parts.push(format!("探索 {} 个文件", stats.explores));
    }
    if stats.searches > 0 {
        parts.push(format!("{} 次搜索", stats.searches));
    }
    if stats.commands > 0 {
        parts.push(format!("执行 {} 条命令", stats.commands));
    }
    if stats.others > 0 {
        parts.push(format!("{} 次其他操作", stats.others));
    }
    if parts.is_empty() {
        "工作中".to_string()
    } else {
        parts.join("，")
    }
}

fn json_arg_str(args: &str, key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Files an edit tool touches, parsed from its args (apply_patch carries them
/// in `*** Add/Update/Delete File:` headers).
fn edit_target_files(name: &str, args: &str) -> Vec<String> {
    if name == "apply_patch" {
        let patch = json_arg_str(args, "patch")
            .or_else(|| json_arg_str(args, "input"))
            .unwrap_or_default();
        return patch
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                line.strip_prefix("*** Add File:")
                    .or_else(|| line.strip_prefix("*** Update File:"))
                    .or_else(|| line.strip_prefix("*** Delete File:"))
                    .map(|p| p.trim().to_string())
            })
            .filter(|p| !p.is_empty())
            .collect();
    }
    json_arg_str(args, "path")
        .or_else(|| json_arg_str(args, "file"))
        .or_else(|| json_arg_str(args, "filePath"))
        .map(|p| vec![p])
        .unwrap_or_default()
}

/// Frontend-local (added, removed) line estimate for an edit tool call.
fn tool_diff_stats(name: &str, args: &str) -> (usize, usize) {
    match name {
        "str_replace_edit" | "edit_file" => {
            let old = json_arg_str(args, "old_string").unwrap_or_default();
            let new = json_arg_str(args, "new_string").unwrap_or_default();
            if old.is_empty() && new.is_empty() {
                return (0, 0);
            }
            diff_counts(&old, &new)
        }
        "apply_patch" => {
            let patch = json_arg_str(args, "patch")
                .or_else(|| json_arg_str(args, "input"))
                .unwrap_or_default();
            patch_diff_stats(&patch)
        }
        "write_file" | "create_document" => {
            let content = json_arg_str(args, "content")
                .or_else(|| json_arg_str(args, "contents"))
                .or_else(|| json_arg_str(args, "text"))
                .unwrap_or_default();
            (content.lines().count(), 0)
        }
        _ => (0, 0),
    }
}

fn diff_counts(old: &str, new: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in compute_line_diff(old, new) {
        match line.tag {
            DiffTag::Insert => added += 1,
            DiffTag::Delete => removed += 1,
            DiffTag::Equal => {}
        }
    }
    (added, removed)
}

/// Count `+` / `-` body lines of a Codex-style patch (headers `*** …` and the
/// `@@` context markers are skipped).
fn patch_diff_stats(patch: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in patch.lines() {
        if line.starts_with("***") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

/// Headline todo pulled out of `write_todos` args: prefer the in-progress
/// item, fall back to the first one.
fn todo_milestone_label(args: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(args).ok();
    let todos = parsed
        .as_ref()
        .and_then(|v| v.get("todos"))
        .and_then(|v| v.as_array());
    let Some(todos) = todos else {
        return "更新待办".to_string();
    };
    let content = |item: &serde_json::Value| {
        item.get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let headline = todos
        .iter()
        .find(|item| item.get("status").and_then(|v| v.as_str()) == Some("in_progress"))
        .and_then(content)
        .or_else(|| todos.iter().find_map(|item| content(item)));
    match headline {
        Some(text) => ellipsize(&text, 60),
        None => "更新待办".to_string(),
    }
}

/// Pull the most informative argument (path / command / query …) out of the
/// pretty-printed args JSON for the collapsed one-line summary.
fn arg_summary(args: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    let obj = value.as_object()?;
    const KEYS: [&str; 11] = [
        "path", "file", "filePath", "command", "query", "pattern", "url", "dir", "skill", "prompt",
        "name",
    ];
    for key in KEYS {
        if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(ellipsize(v, 48));
            }
        }
    }
    None
}

/// Collapsed summary for a tool card: key argument plus result / error head.
fn tool_summary(args: &str, result: &str, status: BlockStatus) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = arg_summary(args) {
        parts.push(a);
    }
    match status {
        BlockStatus::Running => parts.push("运行中…".to_string()),
        BlockStatus::Error => {
            let head = first_line(result);
            parts.push(if head.is_empty() {
                "失败".to_string()
            } else {
                format!("失败：{}", ellipsize(&head, 48))
            });
        }
        BlockStatus::Done => {
            let head = first_line(result);
            if !head.is_empty() {
                parts.push(ellipsize(&head, 48));
            }
        }
    }
    parts.join(" · ")
}

/// Collapsed summary for a reasoning card: a richer excerpt + char count.
fn reasoning_summary(text: &str) -> String {
    let head = compact_text(text);
    let chars = text.chars().count();
    if head.is_empty() {
        format!("{chars} 字")
    } else {
        format!("{} · {chars} 字", ellipsize(&head, 200))
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn compact_text(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// Line-based markdown flattening matching the legacy `.msg-md` chat style:
/// fenced code on sunk background, `>` quotes with a 2px left rail, bullet
/// lists, headings, simplified `|` tables; inline markers are stripped.
pub(crate) fn render_markdown_flat(
    text: &str,
    t: &moonlit_uikit::Tokens,
    streaming: bool,
) -> AnyElement {
    let t = *t;
    let base_color = if streaming { t.text_2 } else { t.text };
    let mut root = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(13.))
        .text_color(base_color);
    let mut lines = text.lines().peekable();
    let mut code_buf: Option<Vec<String>> = None;
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```") {
            match code_buf.take() {
                // closing fence: flush the code block
                Some(buf) => {
                    root = root.child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(6.))
                            .bg(t.bg_sunk)
                            .font_family(FONT_MONO_FALLBACK)
                            .text_size(px(12.))
                            .text_color(t.text_2)
                            .children(buf.into_iter().map(|l| div().child(l))),
                    );
                }
                None => code_buf = Some(Vec::new()),
            }
            continue;
        }
        if let Some(buf) = code_buf.as_mut() {
            buf.push(line.to_string());
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(h) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            root = root.child(
                div()
                    .pt(px(4.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(14.))
                    .child(strip_inline(h)),
            );
        } else if let Some(q) = trimmed.strip_prefix("> ") {
            root = root.child(
                div()
                    .pl(px(12.))
                    .border_l_2()
                    .border_color(t.line)
                    .text_color(t.text_2)
                    .child(strip_inline(q)),
            );
        } else if let Some(li) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .child(div().flex_none().text_color(t.text_3).child("•"))
                    .child(div().flex_1().min_w(px(0.)).child(strip_inline(li))),
            );
        } else if trimmed.starts_with('|') {
            // simplified table row; skip |---| separators
            let cells: Vec<String> = trimmed
                .trim_matches('|')
                .split('|')
                .map(|c| strip_inline(c.trim()))
                .collect();
            if cells.iter().all(|c| {
                c.chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
            }) {
                continue;
            }
            let mut row = div().flex().flex_row().gap(px(8.)).text_size(px(12.));
            for cell in cells {
                row = row.child(div().flex_1().min_w(px(0.)).child(cell));
            }
            root = root.child(row);
        } else {
            root = root.child(div().child(strip_inline(trimmed)));
        }
    }
    // unterminated fence: flush as code
    if let Some(buf) = code_buf {
        root = root.child(
            div()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(6.))
                .bg(t.bg_sunk)
                .font_family(FONT_MONO_FALLBACK)
                .text_size(px(12.))
                .text_color(t.text_2)
                .children(buf.into_iter().map(|l| div().child(l))),
        );
    }
    root.into_any_element()
}

/// Strip `**bold**`, `` `code` `` and `*italic*` markers (inline runs are not
/// individually styled in the GPUI port).
fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(id: &str, name: &str, args: serde_json::Value) -> ChatBlock {
        ChatBlock::Tool {
            id: id.to_string(),
            name: name.to_string(),
            args: args.to_string(),
            result: String::new(),
            status: BlockStatus::Done,
            mcp: None,
        }
    }

    #[test]
    fn timeline_splits_on_milestones() {
        let blocks = vec![
            tool("t1", "read_file", json!({ "path": "a.rs" })),
            tool("t2", "grep", json!({ "pattern": "foo" })),
            ChatBlock::Reasoning {
                text: "思考".into(),
            },
            tool("t3", "write_todos", json!({ "todos": [] })),
            tool("t4", "run_command", json!({ "command": "cargo check" })),
            ChatBlock::Text {
                text: "最终回答".into(),
            },
        ];
        let timeline = build_timeline(&blocks);
        assert_eq!(timeline.len(), 5);
        assert!(matches!(&timeline[0], TimelineItem::Activity(v) if v == &vec![0, 1]));
        assert!(matches!(timeline[1], TimelineItem::Block(2)));
        assert!(matches!(timeline[2], TimelineItem::Block(3)));
        assert!(matches!(&timeline[3], TimelineItem::Activity(v) if v == &vec![4]));
        assert!(matches!(timeline[4], TimelineItem::Block(5)));
    }

    #[test]
    fn segment_stats_counts_categories_and_diff_lines() {
        let blocks = vec![
            tool(
                "t1",
                "str_replace_edit",
                json!({ "path": "a.rs", "old_string": "a\nb", "new_string": "a\nc\nd" }),
            ),
            tool("t2", "read_file", json!({ "path": "a.rs" })),
            tool("t3", "read_file", json!({ "path": "a.rs" })),
            tool("t4", "grep", json!({ "pattern": "foo" })),
            tool("t5", "run_command", json!({ "command": "cargo test" })),
        ];
        let stats = segment_stats(&blocks, &[0, 1, 2, 3, 4]);
        assert_eq!(stats.edits, 1);
        assert_eq!(stats.explores, 1); // 同一路径去重
        assert_eq!(stats.searches, 1);
        assert_eq!(stats.commands, 1);
        assert_eq!(stats.added, 2);
        assert_eq!(stats.removed, 1);
        assert_eq!(
            segment_phrase(&stats),
            "编辑 1 个文件，探索 1 个文件，1 次搜索，执行 1 条命令"
        );
    }

    #[test]
    fn apply_patch_stats_parse_files_and_lines() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n context\n-old line\n+new line\n+extra\n*** Add File: src/b.rs\n+hello\n*** End Patch";
        let args = json!({ "patch": patch }).to_string();
        let files = edit_target_files("apply_patch", &args);
        assert_eq!(files, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
        let (added, removed) = tool_diff_stats("apply_patch", &args);
        assert_eq!((added, removed), (3, 1));
    }

    #[test]
    fn write_file_counts_content_as_added() {
        let args = json!({ "path": "a.txt", "content": "1\n2\n3" }).to_string();
        assert_eq!(tool_diff_stats("write_file", &args), (3, 0));
    }

    #[test]
    fn todo_milestone_prefers_in_progress_item() {
        let args = json!({
            "todos": [
                { "content": "已完成项", "status": "completed" },
                { "content": "进行中项", "status": "in_progress" },
                { "content": "待办项", "status": "pending" }
            ]
        })
        .to_string();
        assert_eq!(todo_milestone_label(&args), "进行中项");
        assert_eq!(todo_milestone_label("not-json"), "更新待办");
    }
}
