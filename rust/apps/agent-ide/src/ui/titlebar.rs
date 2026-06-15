//! 36px titlebar: 「月」logo + menubar, centered command-palette search,
//! right-side status dot / bell / share icon buttons. Mirrors `TitleBar` in
//! the legacy `components.jsx`.

use gpui::{
    anchored, deferred, div, prelude::*, px, Context, MouseButton, MouseDownEvent, Window,
    WindowControlArea,
};
use moonlit_uikit::{ToastKind, FONT_SERIF};

use super::icons::icon;
use super::{float_surface, ibtn, kbd, status_dot};
use crate::app::AgentIdeApp;

impl AgentIdeApp {
    pub(crate) fn render_titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        div()
            .occlude()
            .h(px(36.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .px(px(10.))
            .gap(px(8.))
            .bg(t.bg_sunk)
            .border_b_1()
            .border_color(t.line)
            // Left: logo (drag) + menubar
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
                            .bg(t.text)
                            .text_color(t.text_inv)
                            .text_size(px(14.))
                            .font_family(FONT_SERIF)
                            .window_control_area(WindowControlArea::Drag)
                            .child("月"),
                    )
                    .child(self.render_menubar(cx)),
            )
            // Center: drag gutters + capsule search opening the command palette
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(px(520.))
                            .h(px(24.))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(10.))
                            .rounded_full()
                            .border_1()
                            .border_color(t.line)
                            .bg(t.bg_panel)
                            .cursor_pointer()
                            .hover(move |s| s.border_color(t.line_strong))
                            .child(icon("search", 12., t.text_3))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(t.text_3)
                                    .child("搜索会话、文件、命令…"),
                            )
                            .child(kbd("⌘K", &t))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.menubar_open = None;
                                    this.open_palette(window, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    ),
            )
            // Right: API status dot + bell + share
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        div()
                            .w(px(26.))
                            .h(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(status_dot(if self.connected {
                                t.dot_done
                            } else {
                                t.dot_blocked
                            })),
                    )
                    .child(ibtn(
                        "bell",
                        13.,
                        &t,
                        |this, _w, cx| {
                            this.menubar_open = None;
                            this.notifs_open = !this.notifs_open;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(ibtn(
                        "share-2",
                        13.,
                        &t,
                        |this, _w, cx| {
                            this.menubar_open = None;
                            this.toast("分享功能开发中", ToastKind::Info, cx);
                        },
                        cx,
                    )),
            )
    }

    fn render_menubar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .on_mouse_down_out(cx.listener(|this, _ev: &MouseDownEvent, _window, cx| {
                if this.menubar_open.take().is_some() {
                    cx.notify();
                }
            }))
            .child(self.render_menubar_trigger("file", "File", cx))
            .child(self.render_menubar_trigger("edit", "Edit", cx))
            .child(self.render_menubar_trigger("view", "View", cx))
            .child(self.render_menubar_trigger("help", "Help", cx))
    }

    fn render_menubar_trigger(
        &mut self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.t;
        let is_open = self.menubar_open == Some(id);
        let mut wrap = div().relative().child(
            div()
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(t.text_2)
                .when(is_open, |d| d.bg(t.bg_active))
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_hover))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        cx.stop_propagation();
                        this.menubar_open = if this.menubar_open == Some(id) {
                            None
                        } else {
                            Some(id)
                        };
                        cx.notify();
                    }),
                ),
        );
        if is_open {
            wrap = wrap.child(deferred(
                anchored()
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .occlude()
                            .mt(px(30.))
                            .child(self.render_menubar_dropdown(id, cx)),
                    ),
            ));
        }
        wrap
    }

    /// `.menubar-dropdown`: min-width 240, radius 8, rows with shortcut hints.
    fn render_menubar_dropdown(
        &self,
        which: &'static str,
        cx: &mut Context<AgentIdeApp>,
    ) -> impl IntoElement {
        let t = self.t;
        let row = |icon_name: &'static str,
                   label: &'static str,
                   shortcut: Option<&'static str>,
                   action: fn(&mut AgentIdeApp, &mut Window, &mut Context<AgentIdeApp>),
                   cx: &mut Context<AgentIdeApp>| {
            let mut item = div()
                .h(px(26.))
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(t.text_2)
                .cursor_pointer()
                .hover(move |s| s.bg(t.bg_selection).text_color(t.accent))
                .child(icon(icon_name, 13., t.text_3))
                .child(div().flex_1().child(label));
            if let Some(sc) = shortcut {
                item = item.child(kbd(sc, &t));
            }
            item.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.menubar_open = None;
                    action(this, window, cx);
                }),
            )
        };
        let sep = || div().h(px(1.)).my(px(4.)).bg(t.line);

        let mut menu = float_surface(&t)
            .min_w(px(240.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.));
        match which {
            "file" => {
                menu = menu
                    .child(row(
                        "sparkles",
                        "新建会话",
                        Some("Ctrl+Shift+N"),
                        |this, _w, cx| this.new_session(cx),
                        cx,
                    ))
                    .child(row(
                        "folder",
                        "打开文件夹…",
                        Some("Ctrl+O"),
                        |this, _w, cx| this.open_workspace(cx),
                        cx,
                    ))
                    .child(sep())
                    .child(row("x", "退出", None, |_this, _w, cx| cx.quit(), cx));
            }
            "edit" => {
                menu = menu
                    .child(row(
                        "copy",
                        "复制",
                        Some("Ctrl+C"),
                        |_this, window, cx| {
                            window.dispatch_action(Box::new(moonlit_uikit::gpui_ui::Copy), cx);
                        },
                        cx,
                    ))
                    .child(row(
                        "split",
                        "剪切",
                        Some("Ctrl+X"),
                        |_this, window, cx| {
                            window.dispatch_action(Box::new(moonlit_uikit::gpui_ui::Cut), cx);
                        },
                        cx,
                    ))
                    .child(row(
                        "file",
                        "粘贴",
                        Some("Ctrl+V"),
                        |_this, window, cx| {
                            window.dispatch_action(Box::new(moonlit_uikit::gpui_ui::Paste), cx);
                        },
                        cx,
                    ))
                    .child(sep())
                    .child(row(
                        "check",
                        "全选",
                        Some("Ctrl+A"),
                        |_this, window, cx| {
                            window.dispatch_action(Box::new(moonlit_uikit::gpui_ui::SelectAll), cx);
                        },
                        cx,
                    ));
            }
            "view" => {
                menu = menu
                    .child(row(
                        "list-tree",
                        "切换会话栏",
                        None,
                        |this, _w, cx| {
                            this.sessions_collapsed = !this.sessions_collapsed;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(row(
                        "message-square-text",
                        "切换对话栏",
                        None,
                        |this, _w, cx| {
                            this.chat_collapsed = !this.chat_collapsed;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(row(
                        "folder-tree",
                        "切换 Inspector",
                        None,
                        |this, _w, cx| {
                            this.inspector_collapsed = !this.inspector_collapsed;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(row(
                        "panel-bottom",
                        "切换底部面板",
                        Some("Ctrl+J"),
                        |this, _w, cx| this.toggle_bottom(cx),
                        cx,
                    ))
                    .child(sep())
                    .child(row(
                        "sparkles",
                        "切换主题（浅色/深色）",
                        None,
                        |this, _w, cx| this.toggle_theme(cx),
                        cx,
                    ))
                    .child(row(
                        "search",
                        "命令面板",
                        Some("Ctrl+K"),
                        |this, window, cx| this.open_palette(window, cx),
                        cx,
                    ))
                    .child(sep())
                    .child(row(
                        "file-text",
                        "文档编辑器 (DocForge)",
                        None,
                        |this, _w, cx| {
                            this.state.workbench.open_builtin(crate::BuiltinTab::DocForge);
                            this.active_tab = "docforge".into();
                            this.menubar_open = None;
                            cx.notify();
                        },
                        cx,
                    ));
            }
            _ => {
                menu = menu
                    .child(row(
                        "message-square-text",
                        "键盘快捷键",
                        None,
                        |this, _w, cx| {
                            this.shortcuts_open = true;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(row(
                        "book-open",
                        "文档",
                        None,
                        |this, _w, cx| {
                            this.toast("文档即将上线", ToastKind::Info, cx);
                        },
                        cx,
                    ))
                    .child(sep())
                    .child(row(
                        "user-round",
                        "关于月夜",
                        None,
                        |this, _w, cx| {
                            this.about_open = true;
                            cx.notify();
                        },
                        cx,
                    ));
            }
        }
        menu
    }
}
