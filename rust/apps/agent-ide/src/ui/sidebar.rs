//! 256px sessions sidebar mirroring the legacy `SessionsSidebar`: search box,
//! New Agent / Marketplace action rows, Pinned + HOME sections with hoverable
//! pin/delete actions and double-click rename, More(N) overflow, footer
//! workspace menu + user card popup.

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, MouseDownEvent};
use moonlit_core::models::DebugSession;
use moonlit_uikit::ToastKind;

use super::icons::icon;
use super::{float_surface, kbd, sec_head, status_dot};
use crate::app::AgentIdeApp;

impl AgentIdeApp {
    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let query = self.sidebar_search.read(cx).text().to_lowercase();
        let filtered: Vec<DebugSession> = self
            .state
            .sessions
            .iter()
            .filter(|s| {
                query.is_empty()
                    || s.title.to_lowercase().contains(&query)
                    || s.id.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let pinned: Vec<DebugSession> = filtered.iter().filter(|s| s.pinned).cloned().collect();
        let home_all: Vec<DebugSession> = filtered.into_iter().filter(|s| !s.pinned).collect();
        let count = home_all.len();
        // Legacy HOME_VISIBLE_LIMIT = 12 with a "More (N)" toggle.
        let overflow = count.saturating_sub(12);
        let home: Vec<DebugSession> = if self.show_all_home || overflow == 0 {
            home_all
        } else {
            home_all.into_iter().take(12).collect()
        };
        let show_all = self.show_all_home;

        div()
            .w(px(self.pane_w.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.bg_sunk)
            // ---- head: search + action rows --------------------------------
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .child(
                        div()
                            .h(px(28.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.line)
                            .bg(t.bg_panel)
                            .child(icon("search", 12., t.text_3))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .child(self.sidebar_search.clone()),
                            )
                            .child(kbd("/", &t)),
                    )
                    .child(self.sidebar_action(
                        "sparkles",
                        "New Agent",
                        Some("Ctrl+Shift+N"),
                        |this, cx| this.new_session(cx),
                        cx,
                    ))
                    .child(self.sidebar_action(
                        "store",
                        "Marketplace",
                        None,
                        |this, cx| {
                            this.toast("Marketplace 即将上线", ToastKind::Info, cx);
                        },
                        cx,
                    )),
            )
            // ---- body: sections ---------------------------------------------
            .child(
                div()
                    .id("sessions-body")
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .when(!self.connected, |d| {
                        // `.sessions-offline-hint`
                        d.child(
                            div()
                                .mx(px(12.))
                                .mb(px(10.))
                                .p(px(8.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(t.line)
                                .bg(t.bg_panel)
                                .text_size(px(11.))
                                .text_color(t.text_3)
                                .child("后端未连接"),
                        )
                    })
                    .when(pinned.is_empty() && home.is_empty(), |d| {
                        d.child(
                            div()
                                .p(px(14.))
                                .text_size(px(12.))
                                .text_color(t.text_4)
                                .child("暂无会话，可点击「New Agent」创建。"),
                        )
                    })
                    .when(!pinned.is_empty(), |d| {
                        d.child(sec_head("pin", "PINNED", &t))
                            .children(pinned.iter().map(|s| self.session_row(s, cx)))
                    })
                    .when(!home.is_empty(), |d| {
                        d.child(
                            sec_head("home", "HOME", &t).child(
                                div()
                                    .ml_auto()
                                    .text_size(px(10.))
                                    .text_color(t.text_4)
                                    .font_family(moonlit_uikit::FONT_MONO_FALLBACK)
                                    .child(format!("{count}")),
                            ),
                        )
                    })
                    .children(home.iter().map(|s| self.session_row(s, cx)))
                    .when(overflow > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(5.))
                                .mx(px(10.))
                                .my(px(4.))
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(6.))
                                .text_size(px(11.))
                                .text_color(t.text_3)
                                .cursor_pointer()
                                .hover(move |s| s.bg(t.bg_hover))
                                .child(icon(
                                    if show_all {
                                        "chevron-up"
                                    } else {
                                        "chevron-down"
                                    },
                                    11.,
                                    t.text_3,
                                ))
                                .child(if show_all {
                                    "收起".to_string()
                                } else {
                                    format!("More ({overflow})")
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                        this.show_all_home = !this.show_all_home;
                                        cx.notify();
                                    }),
                                ),
                        )
                    }),
            )
            // ---- foot: workspace + user card --------------------------------
            .child(self.render_sidebar_foot(cx))
    }

    /// One `.sess-row`: 36px, status dot + title/meta, hover pin/trash actions,
    /// double-click rename.
    fn session_row(&self, s: &DebugSession, cx: &mut Context<AgentIdeApp>) -> AnyElement {
        let t = self.t;
        let id = s.id.clone();
        let is_sel = self.state.active_session_id.as_deref() == Some(s.id.as_str());
        let renaming = self.renaming_session.as_deref() == Some(s.id.as_str());
        let title = if s.title.is_empty() {
            s.id.clone()
        } else {
            s.title.clone()
        };
        let dot = t.dot_for_status(s.status.as_deref().unwrap_or("idle"));
        let mode = s.mode.clone().unwrap_or_else(|| "hybrid".into());
        let status_label = match s.status.as_deref() {
            Some("completed") | Some("done") => "已完成",
            Some("running") => "运行中",
            Some("failed") => "失败",
            _ => "已同步",
        };
        let pinned = s.pinned;

        let select_id = id.clone();
        let rename_id = id.clone();
        let pin_id = id.clone();
        let del_id = id.clone();

        let middle: AnyElement = if renaming {
            div()
                .flex_1()
                .min_w(px(0.))
                .px(px(4.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .border_color(t.accent_ring)
                .bg(t.bg_panel)
                .text_size(px(12.4))
                .child(self.rename_input.clone())
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.4))
                        .text_color(t.text)
                        .truncate()
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.2))
                        .text_color(t.text_4)
                        .truncate()
                        .child(format!("{mode} · {status_label}")),
                )
                .into_any_element()
        };

        div()
            .group("sess")
            .min_h(px(36.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(6.))
            .py(px(6.))
            .border_l_2()
            .border_color(if is_sel {
                t.accent
            } else {
                gpui::rgba(0x00000000)
            })
            .when(is_sel, |d| d.bg(t.bg_active))
            .cursor_pointer()
            .hover(move |st| st.bg(t.bg_hover))
            .child(status_dot(dot))
            .child(middle)
            // hover actions: pin / trash (legacy `.sess-row .actions`)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(2.))
                    .opacity(0.)
                    .group_hover("sess", |st| st.opacity(1.))
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .hover(move |st| st.bg(t.bg_active))
                            .child(icon(if pinned { "pin-off" } else { "pin" }, 11., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                                    cx.stop_propagation();
                                    let _ = ev;
                                    this.pin_session(pin_id.clone(), !pinned, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .hover(move |st| st.bg(t.danger_bg))
                            .child(icon("trash-2", 11., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                    cx.stop_propagation();
                                    this.delete_session(del_id.clone(), cx);
                                }),
                            ),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    if ev.click_count >= 2 {
                        this.start_rename(rename_id.clone(), cx);
                    } else if !ev.modifiers.secondary() {
                        this.select_session(select_id.clone(), cx);
                    }
                }),
            )
            .into_any_element()
    }

    fn render_sidebar_foot(&self, cx: &mut Context<AgentIdeApp>) -> impl IntoElement {
        let t = self.t;
        let ws_root = self
            .store
            .as_ref()
            .map(|s| s.get_string_or(moonlit_core::store::keys::WORKSPACE_ROOT, ""))
            .filter(|p| !p.is_empty());
        let ws_name = ws_root
            .as_ref()
            .and_then(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "我的工作区".to_string());

        let mut foot = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(8.))
            .border_t_1()
            .border_color(t.line)
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .text_size(px(12.4))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg_hover))
                    .child(icon("folder", 13., t.text_3))
                    .child(div().flex_1().child("Open Workspace"))
                    .child(icon("chevron-up", 11., t.text_4))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                            this.ws_menu_open = !this.ws_menu_open;
                            this.user_menu_open = false;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg_hover))
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(t.accent)
                            .text_color(gpui::rgb(0xffffff))
                            .text_size(px(12.))
                            .child("我"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.text)
                                    .truncate()
                                    .child("我的工作区"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(t.text_4)
                                    .truncate()
                                    .child(self.auth_user.clone().unwrap_or_else(|| "本地用户".into())),
                            ),
                    )
                    .child(icon("chevron-up", 11., t.text_4))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                            this.user_menu_open = !this.user_menu_open;
                            this.ws_menu_open = false;
                            cx.notify();
                        }),
                    ),
            );

        // Popups render last so they paint above the foot rows.
        if self.ws_menu_open {
            let mut menu = float_surface(&t)
                .absolute()
                .bottom(px(70.))
                .left(px(8.))
                .right(px(8.))
                .flex()
                .flex_col()
                .p(px(4.))
                .rounded(px(10.));
            if let Some(root) = &ws_root {
                let root = root.clone();
                menu = menu.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .hover(move |s| s.bg(t.bg_selection))
                        .child(icon("folder", 13., t.accent))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(12.)).truncate().child(ws_name.clone()))
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(t.text_4)
                                        .truncate()
                                        .child(root),
                                ),
                        )
                        .child(icon("check", 11., t.accent)),
                );
                menu = menu.child(div().h(px(1.)).my(px(4.)).bg(t.line));
            }
            menu = menu
                .child(
                    super::menu_item("folder", "Open Folder…", false, &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                            this.ws_menu_open = false;
                            this.open_workspace(cx);
                        }),
                    ),
                )
                .child(disabled_item("settings-2", "Set Up Workspace", &t))
                .child(disabled_item("terminal", "Connect SSH", &t))
                .child(disabled_item("terminal", "Connect WSL", &t));
            foot = foot.child(menu);
        }

        if self.user_menu_open {
            foot = foot.child(
                float_surface(&t)
                    .absolute()
                    .bottom(px(40.))
                    .left(px(8.))
                    .w(px(180.))
                    .flex()
                    .flex_col()
                    .p(px(4.))
                    .rounded(px(8.))
                    .child(
                        super::menu_item("user-round", "个人资料", false, &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                this.user_menu_open = false;
                                this.profile_open = true;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        super::menu_item("settings-2", "设置", false, &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                this.user_menu_open = false;
                                this.settings_open = true;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }

        foot
    }

    fn sidebar_action(
        &self,
        icon_name: &'static str,
        label: &'static str,
        hint: Option<&'static str>,
        on_click: fn(&mut AgentIdeApp, &mut Context<AgentIdeApp>),
        cx: &mut Context<AgentIdeApp>,
    ) -> impl IntoElement {
        let t = self.t;
        let mut row = div()
            .h(px(28.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .rounded(px(6.))
            .text_size(px(12.4))
            .text_color(t.text_2)
            .cursor_pointer()
            .hover(move |s| s.bg(t.bg_hover))
            .child(icon(icon_name, 13., t.text_3))
            .child(div().flex_1().child(label));
        if let Some(hint) = hint {
            row = row.child(kbd(hint, &t));
        }
        row.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| on_click(this, cx)),
        )
    }
}

fn disabled_item(
    icon_name: &'static str,
    label: &'static str,
    t: &moonlit_uikit::Tokens,
) -> gpui::Div {
    let t = *t;
    div()
        .h(px(26.))
        .px(px(8.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(t.text_4)
        .child(icon(icon_name, 13., t.text_4))
        .child(div().flex_1().child(label))
}
