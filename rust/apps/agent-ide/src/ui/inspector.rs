//! 288px right inspector mirroring the legacy `InspectorPanel`: workspace file
//! tree (12px depth indentation, chevron + type icon grid rows, max-height
//! 360px), Subagents section, and current-run controls.

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, MouseDownEvent};

use super::icons::icon;
use super::sec_head;
use crate::app::AgentIdeApp;
use crate::{WorkspaceNode, WorkspaceNodeKind};

impl AgentIdeApp {
    pub(crate) fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let query = self.ws_search.read(cx).text().to_lowercase();
        let tree: Vec<_> = self
            .state
            .workbench
            .workspace_tree
            .iter()
            .filter(|n| query.is_empty() || n.name.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let run_id = self.state.chat.active_run_id.clone();
        let info_row = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .gap(px(8.))
                .text_size(px(11.))
                .child(div().w(px(44.)).flex_none().text_color(t.text_4).child(label))
                .child(div().flex_1().min_w(px(0.)).truncate().text_color(t.text_2).child(value))
        };

        div()
            .w(px(self.pane_w.2))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.bg_sunk)
            // ---- workspace tree ------------------------------------------------
            .child(
                sec_head("folder-tree", "工作区", &t).when_some(
                    self.git_branch.clone(),
                    |d, branch| {
                        d.child(
                            div()
                                .ml_auto()
                                .max_w(px(110.))
                                .px(px(7.))
                                .py(px(1.))
                                .rounded_full()
                                .bg(t.accent_bg)
                                .text_size(px(10.))
                                .text_color(t.accent)
                                .truncate()
                                .child(branch),
                        )
                    },
                ),
            )
            .child(
                div().px(px(10.)).pb(px(6.)).child(
                    div()
                        .h(px(26.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.line)
                        .bg(t.bg_panel)
                        .text_size(px(12.))
                        .child(icon("search", 11., t.text_4))
                        .child(div().flex_1().child(self.ws_search.clone())),
                ),
            )
            .child(
                div()
                    .id("ws-tree")
                    .max_h(px(360.))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .when(tree.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(14.))
                                .py(px(6.))
                                .text_size(px(12.))
                                .text_color(t.text_4)
                                .child("未打开工作区"),
                        )
                    })
                    .children(render_tree_rows(&tree, 0, self, cx)),
            )
            // ---- subagents ------------------------------------------------------
            .child(
                sec_head("git-fork", "Subagents", &t).child(
                    div()
                        .ml_auto()
                        .px(px(6.))
                        .rounded_full()
                        .bg(t.bg_active)
                        .text_size(px(10.))
                        .child("0 运行中"),
                ),
            )
            .child(
                div()
                    .px(px(14.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(t.text_4)
                    .child("暂无运行中的子代理。"),
            )
            // ---- checkpoints ----------------------------------------------------
            .child(
                sec_head("history", "检查点", &t).child(
                    div()
                        .ml_auto()
                        .px(px(7.))
                        .py(px(1.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.line)
                        .bg(t.bg_panel)
                        .text_size(px(11.))
                        .text_color(t.text_2)
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.bg_hover))
                        .child("+ 新建")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev: &MouseDownEvent, _w, cx| this.make_checkpoint(cx)),
                        ),
                ),
            )
            .child(render_checkpoints(self, cx))
            .child(div().flex_1())
            // ---- current run (legacy 任务/状态/Run/Context rows) ------------------
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .p(px(12.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.))
                            .pb(px(2.))
                            .text_size(px(10.))
                            .text_color(t.text_4)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(icon("activity", 10., t.text_4))
                            .child("当前运行"),
                    )
                    .child(info_row(
                        "任务",
                        self.state
                            .chat
                            .messages
                            .iter()
                            .rev()
                            .find(|m| m.role == crate::ChatRole::User)
                            .map(|m| m.text.clone())
                            .unwrap_or_else(|| "等待启动运行".into()),
                    ))
                    .child(info_row(
                        "状态",
                        if run_id.is_some() { "运行中".into() } else { "空闲".into() },
                    ))
                    .child(info_row("Run", run_id.clone().unwrap_or_else(|| "—".into())))
                    .child(info_row(
                        "Context",
                        format!("{:.0}%", self.metrics.context_fill_pct),
                    ))
                    // legacy controls: primary 暂停/恢复 + ghost 取消 + sync
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(5.))
                            .mt(px(4.))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(5.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(t.line)
                                    .bg(t.bg_panel)
                                    .text_size(px(12.))
                                    .text_color(if run_id.is_some() { t.text } else { t.text_4 })
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("pause", 11., if run_id.is_some() { t.text_2 } else { t.text_4 }))
                                    .child("暂停")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.run_control("pause", cx)
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(5.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(t.line)
                                    .bg(t.bg_panel)
                                    .text_size(px(12.))
                                    .text_color(if run_id.is_some() { t.text } else { t.text_4 })
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("play", 11., if run_id.is_some() { t.text_2 } else { t.text_4 }))
                                    .child("恢复")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.run_control("resume", cx)
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(24.))
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(t.line)
                                    .bg(t.bg_panel)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("scroll-text", 11., t.text_3))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.fetch_run_logs(cx)
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(24.))
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(t.line)
                                    .bg(t.bg_panel)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.danger_bg))
                                    .child(icon("square", 10., t.danger))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            this.abort_run(cx)
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(24.))
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(t.line)
                                    .bg(t.bg_panel)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(t.bg_hover))
                                    .child(icon("rotate-cw", 11., t.text_3))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                            if let Some(id) = this.state.active_session_id.clone() {
                                                this.select_session(id, cx);
                                            }
                                        }),
                                    ),
                            ),
                    ),
            )
    }
}

/// Checkpoint list: label/time rows each with a rewind button.
fn render_checkpoints(app: &AgentIdeApp, cx: &mut Context<AgentIdeApp>) -> AnyElement {
    let t = app.t;
    if app.checkpoints.is_empty() {
        return div()
            .px(px(14.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(t.text_4)
            .child("暂无检查点。")
            .into_any_element();
    }
    let mut list = div().px(px(10.)).flex().flex_col().gap(px(4.));
    for cp in app.checkpoints.iter().take(20) {
        let id = cp
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let label = cp
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("检查点")
            .to_string();
        let meta = cp
            .get("createdAt")
            .and_then(|v| v.as_str())
            .map(|s| s.get(11..16).unwrap_or(s).to_string())
            .unwrap_or_default();
        let cp_id = id.clone();
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(t.line)
                .bg(t.bg_panel)
                .child(div().flex_1().min_w(px(0.)).truncate().text_size(px(12.)).child(label))
                .child(div().text_size(px(10.)).text_color(t.text_4).child(meta))
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
                        .child(icon("rotate-ccw", 11., t.text_3))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                this.rewind_to_checkpoint(cp_id.clone(), cx)
                            }),
                        ),
                ),
        );
    }
    list.into_any_element()
}

/// `.ws-tree-row`: padding 3px, depth*12px indentation, dirs lazy-expand,
/// git status badges (`.ws-status--m/u/a/d`).
fn render_tree_rows(
    nodes: &[WorkspaceNode],
    depth: usize,
    app: &AgentIdeApp,
    cx: &mut Context<AgentIdeApp>,
) -> Vec<AnyElement> {
    let t = app.t;
    let mut out = Vec::new();
    for node in nodes {
        let indent = px(12.0 + depth as f32 * 12.0);
        let badge = node.git.as_deref().map(|code| {
            let (label, fg, bg) = match code {
                "M" | "MM" | " M" => ("M", t.warn, t.warn_bg),
                "A" | "AM" => ("A", t.sage, t.sage_bg),
                "D" => ("D", t.danger, t.danger_bg),
                "??" | "U" => ("U", t.text_3, t.bg_sunk),
                _ => ("·", t.text_4, t.bg_sunk),
            };
            div()
                .px(px(4.))
                .rounded(px(3.))
                .bg(bg)
                .text_size(px(9.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(fg)
                .child(label)
        });
        match node.kind {
            WorkspaceNodeKind::Directory => {
                let expanded = app.expanded_dirs.contains(&node.path);
                let toggle_path = node.path.clone();
                out.push(
                    div()
                        .pl(indent)
                        .pr(px(14.))
                        .py(px(3.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.))
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(t.text_2)
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.bg_hover))
                        .child(icon(
                            if expanded { "chevron-down" } else { "chevron-right" },
                            10.,
                            t.text_4,
                        ))
                        .child(icon("folder", 12., t.text_3))
                        .child(div().flex_1().min_w(px(0.)).truncate().child(node.name.clone()))
                        .when_some(badge, |d, b| d.child(b))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                this.toggle_dir(toggle_path.clone(), cx)
                            }),
                        )
                        .into_any_element(),
                );
                if expanded {
                    if node.children.is_empty() {
                        out.push(
                            div()
                                .pl(px(12.0 + (depth + 1) as f32 * 12.0))
                                .py(px(2.))
                                .text_size(px(11.))
                                .text_color(t.text_4)
                                .child("加载中…")
                                .into_any_element(),
                        );
                    } else {
                        out.extend(render_tree_rows(&node.children, depth + 1, app, cx));
                    }
                }
            }
            WorkspaceNodeKind::File => {
                let path = node.path.clone();
                let hidden = node.name.starts_with('.');
                out.push(
                    div()
                        .pl(indent)
                        .pr(px(14.))
                        .py(px(3.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.))
                        .text_size(px(12.5))
                        .text_color(if hidden { t.text_3 } else { t.text })
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.bg_hover))
                        .child(div().w(px(10.)).flex_none())
                        .child(icon("file", 12., t.text_4))
                        .child(div().flex_1().min_w(px(0.)).truncate().child(node.name.clone()))
                        .when_some(badge, |d, b| d.child(b))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                this.open_file(path.clone(), cx)
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
    }
    out
}
