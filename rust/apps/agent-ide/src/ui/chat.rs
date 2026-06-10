//! 360px chat column mirroring the legacy `ChatColumn`: 38px head with title /
//! provider chip / mode chip / icon buttons, message stream (right-aligned
//! white cards for the user, full-width bubble-less assistant messages with a
//! 22px 「月」avatar and an `AgentActionTimeline`), and the composer host at
//! the bottom.
//!
//! Timeline display rules (cognitive-load oriented):
//! - The final answer / summary text is always rendered in full — it is the
//!   deliverable and never auto-collapses.
//! - Intermediate steps (reasoning / tools / narration) default to collapsed
//!   cards whose head carries a locally generated one-line summary.
//! - Consecutive completed same-kind tool calls aggregate into one compact
//!   row（「读取 5 个文件」）; animations only play on running steps.

use gpui::{div, prelude::*, px, AnimationExt, AnyElement, Context, MouseButton, MouseDownEvent};
use moonlit_uikit::{ToastKind, FONT_MONO_FALLBACK, FONT_SERIF};

use super::icons::icon;
use super::{chip, ibtn, sh1, status_dot};
use crate::app::AgentIdeApp;
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

    /// `SubagentOverlay`: half-cover card raised above the composer
    /// (`top: 28%`, radius 12, upward shadow).
    fn render_subagent_overlay(&self, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let Some(target) = self.subagent_overlay.clone() else {
            return div().into_any_element();
        };
        let block = self.state.chat.messages.iter().rev().find_map(|m| {
            m.blocks.iter().find_map(|b| match b {
                ChatBlock::Subagent { id, label, prompt, summary, status } if *id == target => {
                    Some((label.clone(), prompt.clone(), summary.clone(), *status))
                }
                _ => None,
            })
        });
        let Some((label, prompt, summary, status)) = block else {
            return div().into_any_element();
        };
        let (badge, rail) = match status {
            BlockStatus::Running => ("运行中", t.accent),
            BlockStatus::Done => ("已完成", t.sage),
            BlockStatus::Error => ("失败", t.danger),
        };
        div()
            .absolute()
            .top(gpui::relative(0.28))
            .bottom(px(120.))
            .left(px(8.))
            .right(px(8.))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(t.line_strong)
            .bg(t.bg_panel)
            .shadow(vec![gpui::BoxShadow {
                color: gpui::rgba(0x00000048).into(),
                offset: gpui::point(px(0.), px(-8.)),
                blur_radius: px(28.),
                spread_radius: px(0.),
            }])
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(t.accent_bg)
                            .child(icon("git-fork", 12., t.accent)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .bg(t.bg_sunk)
                            .text_size(px(10.5))
                            .text_color(rail)
                            .child(badge),
                    )
                    .child(
                        div()
                            .cursor_pointer()
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
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(12.))
                    .overflow_y_scroll()
                    .when(!prompt.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(div().text_size(px(10.)).text_color(t.text_4).font_weight(gpui::FontWeight::SEMIBOLD).child("PROMPT"))
                                .child(
                                    div()
                                        .p(px(8.))
                                        .rounded(px(6.))
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
                            .child(div().text_size(px(10.)).text_color(t.text_4).font_weight(gpui::FontWeight::SEMIBOLD).child("SUMMARY"))
                            .child(if summary.is_empty() {
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.text_4)
                                    .child(if status == BlockStatus::Running { "运行中，尚无摘要…" } else { "无摘要输出。" })
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
            .map(|s| if s.title.is_empty() { s.id.clone() } else { s.title.clone() })
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
            .child(ibtn("git-branch", 13., &t, |this, _w, cx| {
                this.fork_session(cx);
                this.toast("已基于当前会话创建分支", ToastKind::Info, cx);
            }, cx))
            .child(ibtn("history", 13., &t, |this, _w, cx| {
                this.chathead_menu = if this.chathead_menu == Some("replay") { None } else { Some("replay") };
                cx.notify();
            }, cx))
            .child(ibtn("more-horizontal", 13., &t, |this, _w, cx| {
                this.chathead_menu = if this.chathead_menu == Some("more") { None } else { Some("more") };
                cx.notify();
            }, cx))
            .when(self.chathead_menu == Some("replay"), |d| d.child(self.render_replay_menu(cx)))
            .when(self.chathead_menu == Some("more"), |d| d.child(self.render_more_menu(cx)))
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
        let mut menu = div()
            .absolute()
            .top(px(36.))
            .right(px(8.))
            .w(px(220.))
            .max_h(px(280.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.line_strong)
            .bg(t.bg_panel)
            .shadow(super::sh_float())
            .overflow_hidden();
        if users.is_empty() {
            menu = menu.child(
                div().p(px(10.)).text_size(px(11.5)).text_color(t.text_4).child("暂无可回退的消息"),
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
                    .child(div().flex_1().min_w(px(0.)).truncate().child(format!("回退到：{preview}")))
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
        div()
            .absolute()
            .top(px(36.))
            .right(px(8.))
            .w(px(180.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.line_strong)
            .bg(t.bg_panel)
            .shadow(super::sh_float())
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
                        let id = this.state.chat.active_run_id.clone()
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
            .provider
            .as_ref()
            .map(|(_, model)| model.clone())
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
            .clone()
            .or_else(|| self.provider.as_ref().map(|(_, model)| model.clone()))
            .unwrap_or_else(|| "default".to_string());
        let mode_label = match self.mode {
            crate::ComposerMode::Build => "Agent",
            crate::ComposerMode::Plan => "Plan",
            crate::ComposerMode::Debug => "Debug",
            crate::ComposerMode::Multitask => "Multitask",
            crate::ComposerMode::Ask => "Ask",
        };

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
            card = card.child(div().text_size(px(13.)).child(m.text.clone()));
        }

        // ---- bottom bar: mode chip / model chip / actions -------------------
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .pt(px(2.))
            // mode chip「∞ Agent」
            .child(
                div()
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
                    .child(icon("infinity", 11., t.text_3))
                    .child(mode_label),
            )
            // model chip
            .child(
                div()
                    .h(px(20.))
                    .px(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(11.))
                    .text_color(t.text_3)
                    .child(icon("sparkles", 11., t.text_3))
                    .child(div().max_w(px(140.)).truncate().child(model_label)),
            )
            .child(div().flex_1())
            .child(icon("paperclip", 12., t.text_4));

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
            bar = bar.child(
                div()
                    .w(px(24.))
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(t.bg_active)
                    .child(icon("arrow-up", 12., t.text_3)),
            );
        }
        card = card.child(bar);

        if !editing {
            let edit_id = m.id.clone();
            card = card
                .cursor_pointer()
                .hover(move |s| s.border_color(t.accent_ring))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, w, cx| {
                        this.start_edit_message(edit_id.clone(), w, cx);
                    }),
                );
        }

        card.into_any_element()
    }

    /// `.msg-agent`: 11px head (avatar / Agent / dot / model label / time) +
    /// the action timeline.
    fn render_agent_message(
        &self,
        m: &ChatMessage,
        is_last: bool,
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
                .when(!model_label.is_empty(), |d| d.child(model_label.to_string()))
                .child(div().flex_1())
                .when(!m.time.is_empty(), |d| d.child(m.time.clone())),
        );

        // ---- action timeline ----------------------------------------------------
        // Fallback for snapshot-hydrated messages without blocks.
        let blocks: Vec<ChatBlock> = if m.blocks.is_empty() {
            let mut v = Vec::new();
            if !m.reasoning.is_empty() {
                v.push(ChatBlock::Reasoning { text: m.reasoning.clone() });
            }
            if !m.text.is_empty() {
                v.push(ChatBlock::Text { text: m.text.clone() });
            }
            v
        } else {
            m.blocks.clone()
        };
        let last_block = blocks.len().saturating_sub(1);
        // The trailing Text block is the message's final answer / summary.
        let final_text_idx = match blocks.last() {
            Some(ChatBlock::Text { .. }) => Some(last_block),
            _ => None,
        };

        // Aggregate consecutive completed same-kind tool calls into one compact
        // row (「读取 5 个文件」); running tools stay standalone with animation.
        enum Item {
            One(usize),
            Group(Vec<usize>),
        }
        let mut items: Vec<Item> = Vec::new();
        let mut i = 0usize;
        while i < blocks.len() {
            if let ChatBlock::Tool { status, name, mcp, .. } = &blocks[i] {
                if *status != BlockStatus::Running {
                    let gk = tool_group_key(name, mcp);
                    let mut j = i + 1;
                    while j < blocks.len() {
                        match &blocks[j] {
                            ChatBlock::Tool { status: s2, name: n2, mcp: mcp2, .. }
                                if *s2 != BlockStatus::Running
                                    && tool_group_key(n2, mcp2) == gk =>
                            {
                                j += 1;
                            }
                            _ => break,
                        }
                    }
                    if j - i >= 2 {
                        items.push(Item::Group((i..j).collect()));
                        i = j;
                        continue;
                    }
                }
            }
            items.push(Item::One(i));
            i += 1;
        }

        for item in items {
            match item {
                Item::One(bi) => {
                    let block = &blocks[bi];
                    let is_trailing = bi == last_block;
                    body = body.child(match block {
                        ChatBlock::Text { text } => {
                            if Some(bi) == final_text_idx {
                                // Final answer: render in full for the live message,
                                // manual 展开/收起 toggle for older long ones.
                                self.render_text_block(
                                    &m.id,
                                    text,
                                    streaming && is_trailing,
                                    is_last,
                                    cx,
                                )
                            } else {
                                // Intermediate narration collapses to a summary card.
                                let key = format!("{}:{}", m.id, bi);
                                let expanded =
                                    *self.expanded_blocks.get(&key).unwrap_or(&false);
                                self.render_action_card(
                                    key,
                                    "message-square-text",
                                    "阶段说明".into(),
                                    None,
                                    Some(text_summary(text)),
                                    expanded,
                                    BlockStatus::Done,
                                    render_markdown_flat(text, &t, false),
                                    cx,
                                )
                            }
                        }
                        ChatBlock::Reasoning { text } => {
                            let key = format!("{}:{}", m.id, bi);
                            let live = streaming && is_trailing;
                            let expanded = *self.expanded_blocks.get(&key).unwrap_or(&live);
                            self.render_action_card(
                                key,
                                "brain",
                                "思考".into(),
                                None,
                                Some(reasoning_summary(text)),
                                expanded,
                                if live { BlockStatus::Running } else { BlockStatus::Done },
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.text_3)
                                    .child(text.clone())
                                    .into_any_element(),
                                cx,
                            )
                        }
                        ChatBlock::Tool { id, name, args, result, status, mcp } => {
                            let key = format!("tool:{id}");
                            let (icon_name, label) = tool_visual(name, mcp);
                            let expanded = *self
                                .expanded_blocks
                                .get(&key)
                                .unwrap_or(&(*status == BlockStatus::Running));
                            let inner = self.render_tool_body(args, result, *status, mcp.is_some());
                            self.render_action_card(
                                key,
                                icon_name,
                                label,
                                mcp.as_ref().map(|_| "MCP".to_string()),
                                Some(tool_summary(args, result, *status)),
                                expanded,
                                *status,
                                inner,
                                cx,
                            )
                        }
                        ChatBlock::Subagent { id, label, prompt, summary, status } => {
                            self.render_subagent_card(id, label, prompt, summary, *status, cx)
                        }
                    });
                }
                Item::Group(indices) => {
                    body = body.child(self.render_tool_group(&blocks, &indices, cx));
                }
            }
        }

        // streaming caret at the very end (legacy `.stream-caret`, 1s blink)
        if streaming {
            body = body.child(super::stream_caret("stream-caret", t.accent));
        }
        body.into_any_element()
    }

    /// `.agent-action-card`: collapsible head (13px text-2, caret rotates) +
    /// body with a 2px left rail (`border-left: 2px solid line; padding
    /// 4px 0 8px 16px; margin-left: 6px`). The collapsed head shows a gray
    /// one-line `summary` so each step stays legible without expanding; the
    /// icon only pulses while the step is actually running.
    #[allow(clippy::too_many_arguments)]
    fn render_action_card(
        &self,
        key: String,
        icon_name: &'static str,
        label: String,
        tag: Option<String>,
        summary: Option<String>,
        expanded: bool,
        status: BlockStatus,
        inner: AnyElement,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let running = status == BlockStatus::Running;
        let head_color = match status {
            BlockStatus::Error => t.danger,
            _ => t.text_2,
        };
        let summary_color = if status == BlockStatus::Error { t.danger } else { t.text_4 };
        let toggle_key = key.clone();
        let next = !expanded;
        // Animation only on the live (running) step; finished cards are static.
        let icon_el: AnyElement = if running {
            icon(icon_name, 13., t.accent)
                .with_animation(
                    gpui::SharedString::from(format!("{key}:icon")),
                    gpui::Animation::new(std::time::Duration::from_millis(1200))
                        .repeat()
                        .with_easing(gpui::pulsating_between(0.35, 1.0)),
                    |el, delta| el.opacity(delta),
                )
                .into_any_element()
        } else {
            icon(icon_name, 13., t.text_3).into_any_element()
        };
        let collapsed_summary = if expanded { None } else { summary };
        let mut card = div().flex().flex_col().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .py(px(4.))
                .px(px(2.))
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(head_color)
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon(
                    if expanded { "chevron-down" } else { "chevron-right" },
                    11.,
                    t.text_4,
                ))
                .child(icon_el)
                .child(div().flex_none().max_w(gpui::relative(0.6)).truncate().child(label))
                .child(match collapsed_summary {
                    Some(s) => div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(summary_color)
                        .child(s)
                        .into_any_element(),
                    None => div().flex_1().into_any_element(),
                })
                .when_some(tag, |d, tag| {
                    d.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(t.accent_bg)
                            .text_size(px(9.5))
                            .text_color(t.accent)
                            .child(tag),
                    )
                })
                .when(running, |d| {
                    d.child(super::pulse_dot(
                        gpui::SharedString::from(format!("{key}:dot")),
                        t.dot_running,
                    ))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.expanded_blocks.insert(toggle_key.clone(), next);
                        cx.notify();
                    }),
                ),
        );
        if expanded {
            card = card.child(
                div()
                    .ml(px(6.))
                    .pl(px(16.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .border_l_2()
                    .border_color(t.line)
                    .child(inner),
            );
        }
        card.into_any_element()
    }

    /// Compact aggregation row for consecutive completed same-kind tool calls
    /// (「读取 5 个文件」); click expands to the individual tool cards. Keys are
    /// derived from tool-call ids so expansion state survives streaming
    /// re-renders.
    fn render_tool_group(
        &self,
        blocks: &[ChatBlock],
        indices: &[usize],
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let mut first_id = String::new();
        let mut icon_name = "terminal";
        let mut phrase = String::new();
        if let Some(ChatBlock::Tool { id, name, mcp, .. }) = indices.first().map(|&bi| &blocks[bi]) {
            first_id = id.clone();
            icon_name = tool_visual(name, mcp).0;
            phrase = group_phrase(name, mcp, indices.len());
        }
        let errors = indices
            .iter()
            .filter(|&&bi| {
                matches!(&blocks[bi], ChatBlock::Tool { status: BlockStatus::Error, .. })
            })
            .count();
        let key = format!("group:{first_id}");
        let expanded = *self.expanded_blocks.get(&key).unwrap_or(&false);
        let toggle_key = key.clone();
        let next = !expanded;
        let mut wrap = div().flex().flex_col().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .py(px(4.))
                .px(px(2.))
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(t.text_2)
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(icon(
                    if expanded { "chevron-down" } else { "chevron-right" },
                    11.,
                    t.text_4,
                ))
                .child(icon(icon_name, 13., t.text_3))
                .child(div().flex_none().truncate().child(phrase))
                .when(errors > 0, |d| {
                    d.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(t.bg_sunk)
                            .text_size(px(9.5))
                            .text_color(t.danger)
                            .child(format!("{errors} 失败")),
                    )
                })
                .child(div().flex_1())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.expanded_blocks.insert(toggle_key.clone(), next);
                        cx.notify();
                    }),
                ),
        );
        if expanded {
            let mut list = div()
                .ml(px(6.))
                .pl(px(10.))
                .border_l_2()
                .border_color(t.line)
                .flex()
                .flex_col();
            for &bi in indices {
                if let ChatBlock::Tool { id, name, args, result, status, mcp } = &blocks[bi] {
                    let ikey = format!("tool:{id}");
                    let (icon_name, label) = tool_visual(name, mcp);
                    let iexp = *self.expanded_blocks.get(&ikey).unwrap_or(&false);
                    let inner = self.render_tool_body(args, result, *status, mcp.is_some());
                    list = list.child(self.render_action_card(
                        ikey,
                        icon_name,
                        label,
                        mcp.as_ref().map(|_| "MCP".to_string()),
                        Some(tool_summary(args, result, *status)),
                        iexp,
                        *status,
                        inner,
                        cx,
                    ));
                }
            }
            wrap = wrap.child(list);
        }
        wrap.into_any_element()
    }

    /// Tool card body: args + result boxes (`.tool-use-args/-result`): mono
    /// 11.5px on `--bg` with a 6px radius, max 320px tall.
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
                .px(px(10.))
                .py(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(t.line)
                .bg(t.bg)
                .max_h(px(320.))
                .overflow_hidden()
                .font_family(FONT_MONO_FALLBACK)
                .text_size(px(11.5))
                .text_color(t.text_2)
                .children(text.lines().take(60).map(|l| div().child(l.to_string())).collect::<Vec<_>>())
                .when(omitted > 0, |d| {
                    d.child(div().text_color(t.text_4).child(format!("（+{omitted} 行已省略）")))
                })
        };
        let mut body = div().flex().flex_col().gap(px(6.));
        if !args.is_empty() {
            body = body.child(boxed(args.to_string()));
        }
        if result.is_empty() {
            body = body.child(div().text_size(px(11.5)).text_color(t.text_4).child(
                if status == BlockStatus::Running { "运行中…" } else { "无输出" },
            ));
        } else {
            body = body.child(boxed(result.to_string()));
        }
        body.into_any_element()
    }

    /// `.subagent-card`: radius 8, 3px status rail, 22x22 icon, 10.5px badge,
    /// 2-line prompt preview; click opens the half-cover overlay.
    fn render_subagent_card(
        &self,
        id: &str,
        label: &str,
        prompt: &str,
        summary: &str,
        status: BlockStatus,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let (rail, badge_text) = match status {
            BlockStatus::Running => (t.accent, "运行中"),
            BlockStatus::Done => (t.sage, "已完成"),
            BlockStatus::Error => (t.danger, "失败"),
        };
        let open_id = id.to_string();
        div()
            .flex()
            .flex_row()
            .my(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(if status == BlockStatus::Running { t.accent_ring } else { t.line })
            .bg(t.bg_panel)
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |s| s.border_color(t.accent_ring))
            .child(div().w(px(3.)).flex_none().bg(rail))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .bg(t.accent_bg)
                                    .child(icon("git-fork", 12., t.accent)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_size(px(12.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(label.to_string()),
                            )
                            .child(
                                div()
                                    .px(px(8.))
                                    .py(px(3.))
                                    .rounded(px(5.))
                                    .bg(t.bg_sunk)
                                    .text_size(px(10.5))
                                    .text_color(t.text_3)
                                    .child(badge_text),
                            )
                            .child(icon("maximize-2", 11., t.text_4)),
                    )
                    .when(!prompt.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(t.text_3)
                                .line_clamp(2)
                                .child(prompt.to_string()),
                        )
                    })
                    .when(!summary.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(t.text_2)
                                .line_clamp(3)
                                .child(summary.to_string()),
                        )
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                    this.subagent_overlay = Some(open_id.clone());
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Assistant final-answer text block. The latest message's answer (and any
    /// actively streaming text) always renders in full — the final summary is
    /// the deliverable and must never be hidden. Older long answers keep a
    /// manual 展开/收起 toggle around the 140px collapse.
    fn render_text_block(
        &self,
        msg_id: &str,
        text: &str,
        streaming: bool,
        is_final: bool,
        cx: &mut Context<AgentIdeApp>,
    ) -> AnyElement {
        let t = self.t;
        let content = render_markdown_flat(text, &t, streaming);
        if is_final || streaming || text.chars().count() <= COLLAPSE_CHARS {
            return content;
        }
        let expanded = self.expanded_msgs.contains(msg_id);
        let toggle_id = msg_id.to_string();
        let toggle_label = if expanded { "收起 ↑" } else { "展开全文 ↓" };
        div()
            .flex()
            .flex_col()
            .child(if expanded {
                content
            } else {
                div().max_h(px(140.)).overflow_hidden().child(content).into_any_element()
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
                            if !this.expanded_msgs.remove(&toggle_id) {
                                this.expanded_msgs.insert(toggle_id.clone());
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

/// Pull the most informative argument (path / command / query …) out of the
/// pretty-printed args JSON for the collapsed one-line summary.
fn arg_summary(args: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    let obj = value.as_object()?;
    const KEYS: [&str; 11] =
        ["path", "file", "filePath", "command", "query", "pattern", "url", "dir", "skill", "prompt", "name"];
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

/// Collapsed summary for a reasoning card: first sentence + char count.
fn reasoning_summary(text: &str) -> String {
    let head = first_line(text);
    let chars = text.chars().count();
    if head.is_empty() {
        format!("{chars} 字")
    } else {
        format!("{} · {chars} 字", ellipsize(&head, 60))
    }
}

/// Collapsed summary for an intermediate narration text block.
fn text_summary(text: &str) -> String {
    ellipsize(&first_line(text), 72)
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
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
pub(crate) fn render_markdown_flat(text: &str, t: &moonlit_uikit::Tokens, streaming: bool) -> AnyElement {
    let t = *t;
    let base_color = if streaming { t.text_2 } else { t.text };
    let mut root = div().flex().flex_col().gap(px(4.)).text_size(px(13.)).text_color(base_color);
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
        if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
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
        } else if let Some(li) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
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
            if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())) {
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
