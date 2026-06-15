//! Center workbench mirroring the legacy `TabGroup` + `BottomPanel`: 34px tab
//! bar on a sunk background with a 2px top accent bar on the active tab,
//! Plan / Todo / Diff / Swarm / README builtin pages plus file tabs, and the
//! optional 260px bottom panel with its own 32px tab bar.

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, MouseDownEvent};
use moonlit_uikit::{DiffTag, FONT_MONO_FALLBACK, FONT_SERIF};

use super::icons::icon;
use super::{ibtn, sh1, status_dot};
use crate::app::AgentIdeApp;
use crate::{BottomPanelTab, ProposalView, TabKind};

/// Collect task/step nodes from the various plan bundle shapes the backend may
/// emit (snapshot bundle, raw `Plan` with nested stages, legacy steps arrays).
fn plan_task_elements(bundle: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    if let Some(tasks) = bundle.get("tasks").and_then(|v| v.as_array()) {
        out.extend(tasks.iter().cloned());
    }
    let plan = bundle.get("plan").unwrap_or(bundle);
    if let Some(arr) = plan
        .get("steps")
        .or_else(|| plan.get("todos"))
        .or_else(|| plan.get("nodes"))
        .and_then(|v| v.as_array())
    {
        out.extend(arr.iter().cloned());
    }
    if let Some(stages) = plan.get("stages").and_then(|v| v.as_array()) {
        for stage in stages {
            if let Some(tasks) = stage.get("tasks").and_then(|v| v.as_array()) {
                out.extend(tasks.iter().cloned());
            }
        }
    }
    out
}

/// Extract `(title, status)` step rows from a plan bundle payload.
fn plan_steps(bundle: &serde_json::Value) -> Vec<(String, String)> {
    let arr = plan_task_elements(bundle);
    if arr.is_empty() {
        return Vec::new();
    }
    arr.iter()
        .filter_map(|el| {
            let title = el
                .get("title")
                .or_else(|| el.get("name"))
                .or_else(|| el.get("content"))
                .or_else(|| el.get("objective"))
                .and_then(|v| v.as_str())?
                .to_string();
            let status = el
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending")
                .to_string();
            Some((title, status))
        })
        .collect()
}

/// Like [`plan_steps`] but also keeps each node's id when present, so the Tree
/// view can offer per-node rerun.
fn plan_steps_full(bundle: &serde_json::Value) -> Vec<(String, String, Option<String>)> {
    let arr = plan_task_elements(bundle);
    if arr.is_empty() {
        return Vec::new();
    }
    arr.iter()
        .filter_map(|el| {
            let title = el
                .get("title")
                .or_else(|| el.get("name"))
                .or_else(|| el.get("content"))
                .or_else(|| el.get("objective"))
                .and_then(|v| v.as_str())?
                .to_string();
            let status = el
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending")
                .to_string();
            let id = el
                .get("id")
                .or_else(|| el.get("nodeId"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Some((title, status, id))
        })
        .collect()
}

/// Legacy `makePages` icon mapping.
fn tab_icon(id: &str) -> &'static str {
    if id.starts_with("doc:") {
        let lower = id.to_ascii_lowercase();
        if lower.ends_with(".pptx") {
            return "file-text";
        }
        if lower.ends_with(".docx") {
            return "file-text";
        }
        if lower.ends_with(".pdf") {
            return "file";
        }
    }
    match id {
        "plan" => "list-tree",
        "todo" => "list-checks",
        "diff" => "git-compare",
        "swarm" => "network",
        "readme" => "file-text",
        "docforge" => "file-text",
        _ => "file",
    }
}

impl AgentIdeApp {
    pub(crate) fn render_main(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let mut main = div()
            .flex_1()
            .min_w(px(420.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(self.render_tabbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(self.render_tab_content(cx)),
            );
        if self.state.workbench.bottom_panel.open {
            main = main.child(self.render_bottom_panel(cx));
        }
        main
    }

    /// `.tabs` — 34px, sunk background; only OPEN tabs are shown (legacy
    /// default is an empty bar with just the split/more actions on the right).
    /// The active tab has a 2px accent top bar and merges into the content
    /// background; every tab carries an icon and a close `x`.
    fn render_tabbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let active = self.active_tab.clone();
        let open_tabs: Vec<(String, String, bool)> = self
            .state
            .workbench
            .tabs
            .iter()
            .map(|tab| {
                let dirty = if let Some(rel) = tab.id.strip_prefix("doc:") {
                    self.document_editors
                        .get(rel)
                        .map(|e| e.read(cx).is_dirty())
                        .unwrap_or(false)
                } else {
                    tab.dirty
                };
                (tab.id.clone(), tab.title.clone(), dirty)
            })
            .collect();

        let mut bar = div()
            .h(px(34.))
            .flex_none()
            .flex()
            .flex_row()
            .items_end()
            .bg(t.bg_sunk)
            .border_b_1()
            .border_color(t.line);

        for (id, title, dirty) in open_tabs {
            let is_active = active == id;
            let icon_name = tab_icon(&id);
            let open_id = id.clone();
            let close_id = id.clone();
            bar = bar.child(
                div()
                    .h(px(34.))
                    .pl(px(12.))
                    .pr(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .border_t_2()
                    .border_color(if is_active {
                        t.accent
                    } else {
                        gpui::rgba(0x00000000)
                    })
                    .when(is_active, |d| d.bg(t.bg))
                    .text_size(px(12.))
                    .text_color(if is_active { t.text } else { t.text_3 })
                    .cursor_pointer()
                    .child(icon(
                        icon_name,
                        12.,
                        if is_active { t.text_2 } else { t.text_4 },
                    ))
                    .child(div().max_w(px(140.)).truncate().child(title))
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .w(px(7.))
                                .h(px(7.))
                                .flex_none()
                                .rounded_full()
                                .bg(t.accent),
                        )
                    })
                    .child(
                        div()
                            .w(px(16.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(move |s| s.bg(t.bg_hover))
                            .child(icon("x", 10., t.text_3))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                    this.close_tab(close_id.clone(), cx)
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.active_tab = open_id.clone();
                            cx.notify();
                        }),
                    ),
            );
        }
        // `.tabs-actions`: split + more
        bar.child(div().flex_1().min_w(px(40.))).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .pr(px(4.))
                .child(ibtn(
                    "columns-2",
                    12.,
                    &t,
                    |this, _w, cx| {
                        this.toast("拆分视图开发中", moonlit_uikit::ToastKind::Info, cx);
                    },
                    cx,
                ))
                .child(ibtn(
                    "more-horizontal",
                    12.,
                    &t,
                    |this, _w, cx| {
                        this.palette_open = true;
                        cx.notify();
                    },
                    cx,
                )),
        )
    }

    fn render_tab_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.active_tab.clone();
        match tab.as_str() {
            "plan" => self.render_plan_page(cx).into_any_element(),
            "todo" => self.render_todo_page(cx).into_any_element(),
            "diff" => self.render_diff_page(cx).into_any_element(),
            "swarm" => self.render_swarm_page().into_any_element(),
            "readme" => self.render_readme_page(cx).into_any_element(),
            "docforge" => self.docforge.clone().into_any_element(),
            other if other.starts_with("doc:") => {
                let path = other.trim_start_matches("doc:").to_string();
                if let Some(editor) = self.document_editors.get(&path) {
                    editor.clone().into_any_element()
                } else if let Some(text) = self.document_previews.get(&path) {
                    self.render_document_text(&path, text).into_any_element()
                } else {
                    self.render_document_loading(&path).into_any_element()
                }
            }
            other if other.starts_with("file:") => {
                self.render_file_page(other.to_string()).into_any_element()
            }
            _ => self.render_empty_page().into_any_element(),
        }
    }

    /// Legacy `EmptyPage`: centered 420px card with serif h1, description and
    /// three capsule tips.
    fn render_empty_page(&self) -> impl IntoElement {
        let t = self.t;
        let tip = |label: &'static str| {
            div()
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(t.line)
                .bg(t.bg)
                .text_size(px(11.))
                .text_color(t.text_3)
                .child(label)
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(32.))
            .child(
                div()
                    .w(px(420.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(28.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(t.line)
                    // color-mix(panel 94%, accent-bg): warm-tinted white
                    .bg(gpui::rgb(0xfdf9f6))
                    .shadow(sh1())
                    .child(
                        div()
                            .text_size(px(28.))
                            .font_family(moonlit_uikit::FONT_SERIF)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("个人工作区"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.text_2)
                            .child("从左侧选择会话，或打开工作区后开始编辑文件。"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(8.))
                            .mt(px(6.))
                            .child(tip("新建会话"))
                            .child(tip("打开工作区"))
                            .child(tip("创建文件")),
                    ),
            )
    }

    // ---- Plan ----------------------------------------------------------------

    fn render_plan_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let view = self.plan_view;
        let plan = self.state.workbench.plan_bundle.clone();
        let steps = plan.as_ref().map(plan_steps).unwrap_or_default();
        let steps_full = plan.as_ref().map(plan_steps_full).unwrap_or_default();
        let has_plan = plan.is_some();

        // `.plan-viewtabs` capsule (Tree | DAG | Timeline | Diff History)
        let viewtab = |label: &'static str, id: &'static str, cx: &mut Context<AgentIdeApp>| {
            let is_active = view == id;
            div()
                .px(px(12.))
                .py(px(4.))
                .rounded_full()
                .text_size(px(12.))
                .when(is_active, |d| {
                    d.bg(t.bg_panel).shadow(sh1()).text_color(t.text)
                })
                .when(!is_active, |d| d.text_color(t.text_3))
                .cursor_pointer()
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        this.plan_view = id;
                        cx.notify();
                    }),
                )
        };

        // meta row: 状态 / 进度 / Tokens / Todos / Subagents
        let done = steps
            .iter()
            .filter(|(_, s)| matches!(s.as_str(), "completed" | "done"))
            .count();
        let meta_chip = |label: String| {
            div()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded_full()
                .border_1()
                .border_color(t.line)
                .bg(t.bg_panel)
                .text_size(px(11.))
                .text_color(t.text_2)
                .child(label)
        };
        let running = self.state.chat.active_run_id.is_some();

        let body: AnyElement = if steps.is_empty() {
            div()
                .pt(px(48.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(24.))
                        .font_family(FONT_SERIF)
                        .child("尚无计划"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text_3)
                        .child("在对话中以 Plan 模式发送任务后，计划将在此展示。"),
                )
                .into_any_element()
        } else {
            match view {
                "dag" => self.render_plan_dag(&steps),
                "timeline" => self.render_plan_timeline(&steps),
                "diff" => self.render_plan_diff_history(),
                _ => self.render_plan_tree(&steps_full, cx),
            }
        };

        div()
            .id("plan-page")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(20.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_family(FONT_SERIF)
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Plan"),
                    )
                    .child(div().flex_1())
                    .child(ibtn(
                        "rotate-cw",
                        12.,
                        &t,
                        |this, _w, cx| {
                            if let Some(id) = this.state.active_session_id.clone() {
                                this.select_session(id, cx);
                            }
                        },
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .p(px(2.))
                            .rounded_full()
                            .bg(t.bg_sunk)
                            .child(viewtab("Tree", "tree", cx))
                            .child(viewtab("DAG", "dag", cx))
                            .child(viewtab("Timeline", "timeline", cx))
                            .child(viewtab("Diff History", "diff", cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(meta_chip(format!(
                        "状态: {}",
                        if running { "运行中" } else { "空闲" }
                    )))
                    .child(meta_chip(format!("进度: {done}/{}", steps.len())))
                    .child(meta_chip(format!("Tokens: {}", self.metrics.total_tokens)))
                    .child(meta_chip(format!("Todos: {}", self.state.todos.len())))
                    .child(meta_chip(format!("Subagents: {}", self.metrics.subagents))),
            )
            .when(has_plan, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(super::btn(
                            "确认计划",
                            false,
                            &t,
                            |this, _w, cx| this.confirm_active_plan(cx),
                            cx,
                        ))
                        .child(super::btn(
                            "执行计划",
                            true,
                            &t,
                            |this, _w, cx| this.execute_active_plan(cx),
                            cx,
                        ))
                        .child(super::btn(
                            "重新规划",
                            false,
                            &t,
                            |this, _w, cx| this.replan_active_plan(cx),
                            cx,
                        )),
                )
            })
            .child(body)
    }

    /// Tree view: status dot + title rows with badges, plus a per-node rerun
    /// action when a run is active and the node carries an id.
    fn render_plan_tree(
        &self,
        steps: &[(String, String, Option<String>)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let can_rerun = self.state.chat.active_run_id.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(steps.iter().map(|(title, status, id)| {
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .child(status_dot(t.dot_for_status(status)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(13.))
                            .child(title.clone()),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(1.))
                            .rounded_full()
                            .bg(t.bg_sunk)
                            .text_size(px(10.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(t.text_3)
                            .child(status.clone()),
                    );
                if can_rerun {
                    if let Some(node_id) = id.clone() {
                        row = row.child(ibtn(
                            "rotate-cw",
                            11.,
                            &t,
                            move |this, _w, cx| this.rerun_plan_node(node_id.clone(), cx),
                            cx,
                        ));
                    }
                }
                row
            }))
            .into_any_element()
    }

    /// DAG view (simplified): node chips chained with arrows.
    fn render_plan_dag(&self, steps: &[(String, String)]) -> AnyElement {
        let t = self.t;
        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(6.));
        for (i, (title, status)) in steps.iter().enumerate() {
            if i > 0 {
                row = row.child(div().text_color(t.text_4).child("→"));
            }
            let accent = matches!(status.as_str(), "running" | "in_progress");
            row = row.child(
                div()
                    .max_w(px(200.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if accent { t.accent_ring } else { t.line })
                    .bg(t.bg_panel)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .child(status_dot(t.dot_for_status(status)))
                    .child(div().truncate().text_size(px(12.)).child(title.clone())),
            );
        }
        row.into_any_element()
    }

    /// Timeline view: label + horizontal progress bars.
    fn render_plan_timeline(&self, steps: &[(String, String)]) -> AnyElement {
        let t = self.t;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(steps.iter().map(|(title, status)| {
                let fill = match status.as_str() {
                    "completed" | "done" => 1.0,
                    "running" | "in_progress" => 0.5,
                    "failed" => 1.0,
                    _ => 0.08,
                };
                let color = t.dot_for_status(status);
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(180.))
                            .flex_none()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(t.text_2)
                            .child(title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(8.))
                            .rounded_full()
                            .bg(t.bg_active)
                            .child(
                                div()
                                    .w(gpui::relative(fill))
                                    .h_full()
                                    .rounded_full()
                                    .bg(color),
                            ),
                    )
            }))
            .into_any_element()
    }

    /// Diff History view: `.dh-row` proposals with a 3px tone bar.
    fn render_plan_diff_history(&self) -> AnyElement {
        let t = self.t;
        let proposals = self.state.workbench.proposals.clone();
        if proposals.is_empty() {
            return div()
                .text_size(px(12.))
                .text_color(t.text_4)
                .child("暂无变更历史。")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(proposals.iter().map(|p| {
                let plus = p.diff.iter().filter(|l| l.tag == DiffTag::Insert).count();
                let minus = p.diff.iter().filter(|l| l.tag == DiffTag::Delete).count();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .overflow_hidden()
                    .child(div().w(px(3.)).h(px(36.)).flex_none().bg(t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px(px(10.))
                            .truncate()
                            .text_size(px(12.5))
                            .child(p.path.clone()),
                    )
                    .child(
                        div()
                            .pr(px(10.))
                            .flex()
                            .flex_row()
                            .gap(px(6.))
                            .font_family(FONT_MONO_FALLBACK)
                            .text_size(px(11.))
                            .child(div().text_color(t.sage).child(format!("+{plus}")))
                            .child(div().text_color(t.danger).child(format!("-{minus}"))),
                    )
            }))
            .into_any_element()
    }

    // ---- Todo ----------------------------------------------------------------

    /// Legacy `TodoBoardPage`: four-column kanban (Backlog / Running / Review
    /// / Done).
    fn render_todo_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let todos = self.state.todos.clone();
        let can_rerun = self.state.chat.active_run_id.is_some();
        let bucket = |statuses: &[&str]| -> Vec<_> {
            todos
                .iter()
                .filter(|td| statuses.contains(&td.status.as_str()))
                .cloned()
                .collect()
        };
        let backlog = bucket(&["pending", "queued", "", "created", "todo"]);
        let running = bucket(&["running", "in_progress"]);
        let review = bucket(&["review", "blocked", "failed"]);
        let done_items = bucket(&["completed", "done"]);

        let column = |label: &'static str,
                      items: Vec<moonlit_core::models::TodoItem>,
                      cx: &mut Context<Self>| {
            let mut col = div()
                .flex_1()
                .min_w(px(180.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .rounded(px(10.))
                .bg(t.bg_sunk)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .pb(px(2.))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(t.text_3)
                        .child(label)
                        .child(
                            div()
                                .px(px(5.))
                                .rounded_full()
                                .bg(t.bg_active)
                                .font_family(FONT_MONO_FALLBACK)
                                .text_size(px(10.))
                                .child(format!("{}", items.len())),
                        ),
                );
            for td in items {
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .p(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.line)
                        .bg(t.bg_panel)
                        .shadow(sh1())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.))
                                .child(status_dot(t.dot_for_status(&td.status)))
                                .child(div().flex_1().min_w(px(0.)).text_size(px(12.5)).child(
                                    if td.title.is_empty() {
                                        td.id.clone()
                                    } else {
                                        td.title.clone()
                                    },
                                )),
                        )
                        .when_some(
                            td.description.clone().filter(|d| !d.is_empty()),
                            |d, desc| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(t.text_3)
                                        .line_clamp(2)
                                        .child(desc),
                                )
                            },
                        )
                        .child({
                            let td_id = td.id.clone();
                            let mut foot = div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .w(px(16.))
                                        .h(px(16.))
                                        .rounded_full()
                                        .bg(t.accent_bg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(9.))
                                        .text_color(t.accent)
                                        .child("月"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(t.text_4)
                                        .child(td.status.clone()),
                                )
                                .child(div().flex_1());
                            if can_rerun {
                                foot = foot.child(ibtn(
                                    "rotate-cw",
                                    11.,
                                    &t,
                                    move |this, _w, cx| this.rerun_todo_action(td_id.clone(), cx),
                                    cx,
                                ));
                            }
                            foot
                        }),
                );
            }
            col
        };

        div()
            .id("todo-page")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(20.))
            .overflow_y_scroll()
            .child(
                div()
                    .text_size(px(24.))
                    .font_family(FONT_SERIF)
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Todo"),
            )
            .when(todos.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text_4)
                        .child("暂无待办。"),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(10.))
                    .child(column("Backlog", backlog, cx))
                    .child(column("Running", running, cx))
                    .child(column("Review", review, cx))
                    .child(column("Done", done_items, cx)),
            )
    }

    // ---- Diff ----------------------------------------------------------------

    /// Legacy `DiffPage`: side-by-side panes, `1/n` proposal navigation,
    /// 拒绝 / 应用修改 buttons wired to the proposal APIs.
    fn render_diff_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let proposals = self.state.workbench.proposals.clone();
        let total = proposals.len();
        let idx = self.diff_index.min(total.saturating_sub(1));

        let mut page = div()
            .id("diff-page")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(20.))
            .overflow_y_scroll();

        if total == 0 {
            return page
                .child(
                    div()
                        .text_size(px(24.))
                        .font_family(FONT_SERIF)
                        .font_weight(gpui::FontWeight::BOLD)
                        .child("Diff"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text_4)
                        .child("暂无变更提案。"),
                );
        }
        let p: &ProposalView = &proposals[idx];

        // header: title + nav + actions
        page = page.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(24.))
                        .font_family(FONT_SERIF)
                        .font_weight(gpui::FontWeight::BOLD)
                        .child("Diff"),
                )
                .child(div().flex_1())
                .child(ibtn(
                    "chevron-left",
                    12.,
                    &t,
                    |this, _w, cx| {
                        if this.diff_index > 0 {
                            this.diff_index -= 1;
                            cx.notify();
                        }
                    },
                    cx,
                ))
                .child(
                    div()
                        .font_family(FONT_MONO_FALLBACK)
                        .text_size(px(11.))
                        .text_color(t.text_3)
                        .child(format!("{}/{}", idx + 1, total)),
                )
                .child(ibtn(
                    "chevron-right",
                    12.,
                    &t,
                    |this, _w, cx| {
                        let total = this.state.workbench.proposals.len();
                        if this.diff_index + 1 < total {
                            this.diff_index += 1;
                            cx.notify();
                        }
                    },
                    cx,
                ))
                .child(
                    div()
                        .h(px(26.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.line)
                        .bg(t.bg_panel)
                        .text_size(px(12.))
                        .text_color(t.danger)
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.danger_bg))
                        .child("拒绝")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                let i = this.diff_index;
                                this.discard_proposal_at(i, cx);
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
                        .child("应用修改")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                let i = this.diff_index;
                                this.apply_proposal_at(i, cx);
                            }),
                        ),
                ),
        );

        // path bar
        page = page.child(
            div()
                .px(px(12.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(t.bg_sunk)
                .border_1()
                .border_color(t.line)
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(p.path.clone())
                .when_some(p.summary.clone().filter(|s| !s.is_empty()), |d, s| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_3)
                            .font_weight(gpui::FontWeight::NORMAL)
                            .child(s),
                    )
                }),
        );

        // side-by-side panes: left = original (Equal+Delete), right = proposed (Equal+Insert)
        let pane = |lines: Vec<(String, bool)>, removed: bool| {
            let mut col = div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .rounded(px(8.))
                .border_1()
                .border_color(t.line)
                .bg(t.bg_panel)
                .overflow_hidden()
                .font_family(FONT_MONO_FALLBACK)
                .text_size(px(11.5));
            for (text, marked) in lines.into_iter().take(400) {
                let (bg, fg) = if marked {
                    if removed {
                        (t.danger_bg, t.danger)
                    } else {
                        (t.sage_bg, t.sage)
                    }
                } else {
                    (gpui::rgba(0x00000000), t.text_3)
                };
                col = col.child(div().px(px(10.)).bg(bg).text_color(fg).child(text));
            }
            col
        };
        let left: Vec<(String, bool)> = p
            .diff
            .iter()
            .filter(|l| l.tag != DiffTag::Insert)
            .map(|l| {
                (
                    l.text.trim_end_matches('\n').to_string(),
                    l.tag == DiffTag::Delete,
                )
            })
            .collect();
        let right: Vec<(String, bool)> = p
            .diff
            .iter()
            .filter(|l| l.tag != DiffTag::Delete)
            .map(|l| {
                (
                    l.text.trim_end_matches('\n').to_string(),
                    l.tag == DiffTag::Insert,
                )
            })
            .collect();
        page.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.))
                .child(pane(left, true))
                .child(pane(right, false)),
        )
    }

    // ---- Swarm -----------------------------------------------------------------

    /// Legacy `SwarmPage`: 24px dot-grid canvas, 200px node cards (main node
    /// with an accent ring) and sampled bezier edges.
    fn render_swarm_page(&self) -> impl IntoElement {
        let t = self.t;
        let swarm = self.state.workbench.swarm.clone();
        // children: best-effort names from the swarm payload
        let children: Vec<(String, String)> = swarm
            .as_ref()
            .and_then(|v| {
                v.get("subagents")
                    .or_else(|| v.get("nodes"))
                    .or_else(|| v.get("agents"))
                    .and_then(|a| a.as_array())
            })
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        let name = n
                            .get("label")
                            .or_else(|| n.get("name"))
                            .or_else(|| n.get("id"))
                            .and_then(|v| v.as_str())?
                            .to_string();
                        let status = n
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("pending")
                            .to_string();
                        Some((name, status))
                    })
                    .take(6)
                    .collect()
            })
            .unwrap_or_default();
        let child_count = children.len();

        let node_card = |title: String, status: String, main: bool| {
            div()
                .w(px(200.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(10.))
                .rounded(px(10.))
                .border_1()
                .border_color(if main { t.accent } else { t.line })
                .bg(t.bg_panel)
                .when(main, |d| {
                    d.shadow(vec![gpui::BoxShadow {
                        color: t.accent_ring.into(),
                        offset: gpui::point(px(0.), px(0.)),
                        blur_radius: px(10.),
                        spread_radius: px(1.),
                    }])
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .child(status_dot(t.dot_for_status(&status)))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(title),
                        ),
                )
                .child(div().text_size(px(10.5)).text_color(t.text_4).child(status))
        };

        // dot-grid + edges painted behind the nodes
        let dot_color = t.line_strong;
        let edge_color = t.accent_ring;
        let grid = gpui::canvas(
            |_bounds, _window, _cx| {},
            move |bounds, _state, window, _cx| {
                let step = px(24.);
                let mut y = bounds.top();
                while y < bounds.bottom() {
                    let mut x = bounds.left();
                    while x < bounds.right() {
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(gpui::point(x, y), gpui::size(px(1.5), px(1.5))),
                            dot_color,
                        ));
                        x += step;
                    }
                    y += step;
                }
                // sampled bezier edges from the main node to each child
                let start = gpui::point(bounds.left() + px(240.), bounds.top() + px(80.));
                for i in 0..child_count {
                    let end = gpui::point(
                        bounds.left() + px(340.),
                        bounds.top() + px(40. + i as f32 * 84.),
                    );
                    let ctrl = gpui::point((start.x + end.x) / 2., start.y);
                    for s in 0..=24 {
                        let u = s as f32 / 24.;
                        let inv = 1. - u;
                        let x = inv * inv * start.x + 2. * inv * u * ctrl.x + u * u * end.x;
                        let y = inv * inv * start.y + 2. * inv * u * ctrl.y + u * u * end.y;
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(gpui::point(x, y), gpui::size(px(2.), px(2.))),
                            edge_color,
                        ));
                    }
                }
            },
        )
        .absolute()
        .inset_0();

        let mut stage = div()
            .relative()
            .flex_1()
            .min_h(px(360.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.line)
            .bg(t.bg_sunk)
            .overflow_hidden()
            .child(grid)
            .child(div().absolute().left(px(40.)).top(px(56.)).child(node_card(
                "主 Agent".into(),
                if self.state.chat.active_run_id.is_some() {
                    "running".into()
                } else {
                    "idle".into()
                },
                true,
            )));
        if children.is_empty() {
            stage = stage.child(
                div()
                    .absolute()
                    .left(px(340.))
                    .top(px(72.))
                    .text_size(px(12.))
                    .text_color(t.text_4)
                    .child("暂无子代理(swarm)活动。"),
            );
        }
        for (i, (name, status)) in children.into_iter().enumerate() {
            stage = stage.child(
                div()
                    .absolute()
                    .left(px(340.))
                    .top(px(24. + i as f32 * 84.))
                    .child(node_card(name, status, false)),
            );
        }

        div()
            .id("swarm-page")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(20.))
            .overflow_y_scroll()
            .child(
                div()
                    .text_size(px(24.))
                    .font_family(FONT_SERIF)
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Swarm"),
            )
            .child(stage)
    }

    // ---- README ----------------------------------------------------------------

    /// Legacy `ReadmePage`: hero bar (mono path + refresh + 在编辑器中打开),
    /// markdown body capped at 760px, content from the REST gateway.
    fn render_readme_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let readme = self.readme.clone();
        div()
            .id("readme-page")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            // `.md-readme-hero`
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(20.))
                    .py(px(8.))
                    .bg(t.bg_sunk)
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .font_family(FONT_MONO_FALLBACK)
                            .text_size(px(11.5))
                            .text_color(t.text_3)
                            .child("README.md"),
                    )
                    .child(div().flex_1())
                    .child(ibtn(
                        "rotate-cw",
                        12.,
                        &t,
                        |this, _w, cx| this.refresh_readme(cx),
                        cx,
                    ))
                    .child(
                        div()
                            .h(px(24.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.line)
                            .bg(t.bg_panel)
                            .text_size(px(11.5))
                            .text_color(t.text_2)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child("在编辑器中打开")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.open_file("README.md".to_string(), cx)
                                }),
                            ),
                    ),
            )
            .child(match readme {
                None => div()
                    .p(px(24.))
                    .text_size(px(12.))
                    .text_color(t.text_4)
                    .child("未找到 README.md。")
                    .into_any_element(),
                Some(text) => div()
                    .w_full()
                    .max_w(px(760.))
                    .pt(px(24.))
                    .px(px(32.))
                    .pb(px(48.))
                    .text_size(px(14.))
                    .child(crate::ui::chat::render_markdown_flat(&text, &t, false))
                    .into_any_element(),
            })
    }

    // ---- File tab ----------------------------------------------------------------

    fn render_document_loading(&self, path: &str) -> impl IntoElement {
        let t = self.t;
        div()
            .id("doc-loading")
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.))
            .text_color(t.text_3)
            .child(format!("正在加载 {path}…"))
    }

    /// Read-only extracted-text view for non-editable documents (PDF).
    fn render_document_text(&self, _path: &str, text: &str) -> impl IntoElement {
        let t = self.t;
        let owned = text.to_string();
        div()
            .id("doc-text")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(6.))
                    .bg(t.bg_sunk)
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(11.5))
                    .text_color(t.text_3)
                    .child("只读预览（PDF 抽取文本，不可编辑保存）"),
            )
            .child(
                div()
                    .id("doc-text-body")
                    .flex_1()
                    .min_h(px(0.))
                    .p(px(12.))
                    .overflow_y_scroll()
                    .font_family(FONT_MONO_FALLBACK)
                    .text_size(px(12.5))
                    .children(owned.lines().take(4000).enumerate().map(|(i, line)| {
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(12.))
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .text_color(t.text_4)
                                    .child(format!("{}", i + 1)),
                            )
                            .child(div().text_color(t.text_2).child(line.to_string()))
                    })),
            )
    }

    fn render_file_page(&self, id: String) -> impl IntoElement {
        let t = self.t;
        let content = self
            .state
            .workbench
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .and_then(|tab| match &tab.kind {
                TabKind::File(buf) => Some(buf.text().to_string()),
                _ => None,
            })
            .unwrap_or_default();
        div()
            .id("file-page")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .p(px(12.))
            .overflow_y_scroll()
            .font_family(FONT_MONO_FALLBACK)
            .text_size(px(12.5))
            .children(content.lines().take(4000).enumerate().map(|(i, line)| {
                div()
                    .flex()
                    .flex_row()
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_color(t.text_4)
                            .child(format!("{}", i + 1)),
                    )
                    .child(div().text_color(t.text_2).child(line.to_string()))
            }))
    }

    // ---- Bottom panel ----------------------------------------------------------

    /// `.bottom-panel`: 260px tall, 32px tab bar (icon + label + mono count,
    /// top-accent active indicator), trash/maximize/close ibtns on the right,
    /// mono 12px content. Terminal shows the legacy `workspace ❯` prompt.
    fn render_bottom_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let active = self.state.workbench.bottom_panel.active;
        let log_count = self.logs.len();
        let event_count = self.events.len();
        let term_count = self.term_history.len();
        let tabs: &[(BottomPanelTab, &str, &str, Option<usize>)] = &[
            (
                BottomPanelTab::Problems,
                "circle-alert",
                "Problems",
                Some(0),
            ),
            (BottomPanelTab::Output, "logs", "Output", Some(log_count)),
            (
                BottomPanelTab::Terminal,
                "terminal",
                "Terminal",
                Some(term_count),
            ),
            (
                BottomPanelTab::Logs,
                "scroll-text",
                "Agent Logs",
                Some(event_count),
            ),
            (BottomPanelTab::Metrics, "activity", "Metrics", None),
        ];

        let mut bar = div()
            .h(px(32.))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .bg(t.bg_sunk)
            .border_b_1()
            .border_color(t.line);
        for (tab, icon_name, label, count) in tabs {
            let tab = *tab;
            let is_active = active == tab;
            let mut node = div()
                .h(px(32.))
                .px(px(12.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .border_t_2()
                .border_color(if is_active {
                    t.accent
                } else {
                    gpui::rgba(0x00000000)
                })
                .when(is_active, |d| d.bg(t.bg))
                .text_size(px(12.))
                .text_color(if is_active { t.text } else { t.text_3 })
                .cursor_pointer()
                .child(icon(
                    icon_name,
                    11.,
                    if is_active { t.text_2 } else { t.text_4 },
                ))
                .child(*label);
            if let Some(count) = count {
                node = node.child(
                    div()
                        .text_size(px(10.))
                        .font_family(FONT_MONO_FALLBACK)
                        .text_color(t.text_4)
                        .child(format!("{count}")),
                );
            }
            bar = bar.child(node.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| this.set_bottom_tab(tab, cx)),
            ));
        }
        bar = bar
            .child(div().flex_1())
            .child(ibtn(
                "trash-2",
                12.,
                &t,
                |this, _w, cx| {
                    this.logs.clear();
                    this.toast("已清空", moonlit_uikit::ToastKind::Info, cx);
                },
                cx,
            ))
            .child(ibtn("maximize-2", 12., &t, |_this, _w, _cx| {}, cx))
            .child(ibtn(
                "x",
                12.,
                &t,
                |this, _w, cx| this.toggle_bottom(cx),
                cx,
            ));

        let body: AnyElement = match active {
            BottomPanelTab::Terminal => {
                let history = self.term_history.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .when(history.is_empty(), |d| {
                        d.child(div().text_color(t.text_4).child("暂无终端历史。"))
                    })
                    .children(history.into_iter().map(|l| {
                        if let Some(cmd) = l.strip_prefix("workspace ❯ ") {
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(4.))
                                .child(div().text_color(t.accent).child("workspace ❯"))
                                .child(div().text_color(t.text_2).child(cmd.to_string()))
                                .into_any_element()
                        } else {
                            div().text_color(t.text_4).child(l).into_any_element()
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .child(div().text_color(t.accent).child("workspace ❯"))
                            .child(div().flex_1().child(self.term_input.clone())),
                    )
                    .into_any_element()
            }
            BottomPanelTab::Logs => {
                // Agent Logs: collapsible event tree (type + seq → payload).
                let events: Vec<_> = self.events.iter().rev().take(120).cloned().collect();
                let mut col = div().flex().flex_col();
                if events.is_empty() {
                    col = col.child(div().text_color(t.text_4).child("（暂无事件）"));
                }
                for evt in events {
                    let key = format!("log:{}", evt.seq.unwrap_or(0));
                    let expanded = *self.expanded_blocks.get(&key).unwrap_or(&false);
                    let toggle = key.clone();
                    let next = !expanded;
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .py(px(1.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child(icon(
                                if expanded {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                9.,
                                t.text_4,
                            ))
                            .child(
                                div()
                                    .text_color(t.text_4)
                                    .child(format!("#{}", evt.seq.unwrap_or(0))),
                            )
                            .child(div().text_color(t.text_2).child(evt.event_type.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                    this.expanded_blocks.insert(toggle.clone(), next);
                                    cx.notify();
                                }),
                            ),
                    );
                    if expanded {
                        let payload =
                            serde_json::to_string_pretty(&evt.payload).unwrap_or_default();
                        col = col.child(
                            div()
                                .ml(px(18.))
                                .my(px(2.))
                                .p(px(6.))
                                .rounded(px(4.))
                                .bg(t.bg_sunk)
                                .flex()
                                .flex_col()
                                .text_color(t.text_3)
                                .children(
                                    payload.lines().take(40).map(|l| div().child(l.to_string())),
                                ),
                        );
                    }
                }
                col.into_any_element()
            }
            BottomPanelTab::Output => {
                let lines: Vec<String> = self.logs.iter().rev().take(200).cloned().collect();
                div()
                    .flex()
                    .flex_col()
                    .when(lines.is_empty(), |d| {
                        d.child(div().text_color(t.text_4).child("（无输出）"))
                    })
                    .children(lines.into_iter().map(|l| div().child(l)))
                    .into_any_element()
            }
            BottomPanelTab::Metrics => {
                // 4-column metric cards incl. Context fill %.
                let card = |label: &'static str, value: String| {
                    div()
                        .flex_1()
                        .min_w(px(120.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.line)
                        .bg(t.bg_panel)
                        .child(div().text_size(px(10.)).text_color(t.text_4).child(label))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_family(FONT_MONO_FALLBACK)
                                .text_color(t.text)
                                .child(value),
                        )
                };
                let m = &self.metrics;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.))
                            .child(card("Total tokens", format!("{}", m.total_tokens)))
                            .child(card("Tool calls", format!("{}", m.tool_calls)))
                            .child(card("Files touched", format!("{}", m.files_touched)))
                            .child(card("Avg latency", format!("{:.0}ms", m.avg_latency_ms))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.))
                            .child(card("Subagents", format!("{}", m.subagents)))
                            .child(card("Context fill", format!("{:.0}%", m.context_fill_pct)))
                            .child(card("Sessions", format!("{}", self.state.sessions.len())))
                            .child(card(
                                "Messages",
                                format!("{}", self.state.chat.messages.len()),
                            )),
                    )
                    .into_any_element()
            }
            BottomPanelTab::Problems => div()
                .text_color(t.text_4)
                .child("暂无问题。")
                .into_any_element(),
        };

        div()
            .h(px(self.bottom_h))
            .flex_none()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.line)
            // `BottomResizer`: 4px ns-resize strip
            .child(
                div()
                    .h(px(4.))
                    .w_full()
                    .flex_none()
                    .cursor(gpui::CursorStyle::ResizeUpDown)
                    .hover(move |s| s.bg(t.accent_ring))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                            this.dragging = Some("bottom");
                            cx.notify();
                        }),
                    ),
            )
            .child(bar)
            .child(
                div()
                    .id("bottom-body")
                    .flex_1()
                    .min_h(px(0.))
                    .p(px(10.))
                    .overflow_y_scroll()
                    .font_family(FONT_MONO_FALLBACK)
                    .text_size(px(12.))
                    .text_color(t.text_2)
                    .child(body),
            )
    }
}
