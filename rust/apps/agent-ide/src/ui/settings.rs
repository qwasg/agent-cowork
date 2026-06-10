//! Settings fullscreen — 1:1 replica of the legacy `SettingsModal`
//! (`apps/agent-ide/public/panels.jsx` + `.settings-* / .set-*` CSS in
//! `index.html`). Shared `set-*` control family lives at the top; the shell
//! and the eight pages are `impl AgentIdeApp` methods below.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    canvas, deferred, div, prelude::*, px, AnyElement, Context, Div, MouseButton, MouseDownEvent,
    SharedString,
};
use moonlit_uikit::{Tokens, FONT_MONO_FALLBACK, FONT_SERIF};

use super::icons::icon;
use crate::app::AgentIdeApp;

/// `.set-h1` — serif 22/600, margin-bottom 18.
pub fn set_h1(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .mb(px(18.))
        .font_family(FONT_SERIF)
        .text_size(px(22.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.text)
        .child(text.into())
}

/// `.set-section-label` — 11px text-3, margin 24/0/8, padding-left 2.
pub fn set_section_label(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .mt(px(24.))
        .mb(px(8.))
        .pl(px(2.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(t.text_3)
        .child(text.into())
}

/// `.set-card` — bg-sunk, 1px line border, radius 10.
pub fn set_card(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_sunk)
}

/// `.set-badge` — 9px mono accent capsule (BETA etc.).
pub fn set_badge(text: &'static str, t: &Tokens) -> Div {
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(3.))
        .bg(t.accent_bg)
        .text_size(px(9.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .font_family(FONT_MONO_FALLBACK)
        .text_color(t.accent)
        .child(text)
}

/// `.set-row` — `1fr auto` grid, padding 14/16, border-b unless `last`,
/// opacity 0.6 when `dim`. `title` is an arbitrary element so callers can
/// add icons/badges inline like the legacy JSX.
pub fn set_row(
    title: impl IntoElement,
    desc: Option<AnyElement>,
    control: Option<AnyElement>,
    last: bool,
    dim: bool,
    t: &Tokens,
) -> Div {
    let mut text_col = div().flex_1().min_w(px(0.)).flex().flex_col().child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(t.text)
            .child(title),
    );
    if let Some(desc) = desc {
        text_col = text_col.child(
            div()
                .mt(px(3.))
                .text_size(px(11.5))
                .line_height(px(17.))
                .text_color(t.text_2)
                .child(desc),
        );
    }
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(16.))
        .py(px(14.))
        .when(!last, |d| d.border_b_1().border_color(t.line))
        .when(dim, |d| d.opacity(0.6))
        .child(text_col);
    if let Some(control) = control {
        row = row.child(div().flex_none().child(control));
    }
    row
}

/// Convenience: plain text title/desc row.
pub fn srow(
    title: &'static str,
    desc: &'static str,
    control: Option<AnyElement>,
    last: bool,
    t: &Tokens,
) -> Div {
    set_row(
        title,
        (!desc.is_empty()).then(|| div().child(desc).into_any_element()),
        control,
        last,
        false,
        t,
    )
}

/// `.set-toggle` — 32×18 pill, sage when on, 14px white knob.
pub fn set_toggle(
    on: bool,
    t: &Tokens,
    on_click: impl Fn(&mut AgentIdeApp, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    div()
        .w(px(32.))
        .h(px(18.))
        .flex_none()
        .rounded_full()
        .bg(if on { t.sage } else { gpui::rgba(0x2a27242e) })
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .px(px(2.))
        .cursor_pointer()
        .child(
            div()
                .w(px(14.))
                .h(px(14.))
                .rounded_full()
                .bg(gpui::rgb(0xffffff))
                .shadow(vec![gpui::BoxShadow {
                    color: gpui::rgba(0x0000002e).into(),
                    offset: gpui::point(px(0.), px(1.)),
                    blur_radius: px(3.),
                    spread_radius: px(0.),
                }]),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                on_click(this, cx);
                cx.notify();
            }),
        )
}

/// `.set-select-wrap` — white pill with chevrons-up-down; opening is
/// mutually exclusive through `app.settings_menu`. The dropdown is rendered
/// `deferred(anchored())` so it escapes the card's rounded clip.
pub fn set_select(
    id: impl Into<String>,
    value: String,
    options: Vec<(String, String)>,
    t: &Tokens,
    app: &AgentIdeApp,
    on_pick: impl Fn(&mut AgentIdeApp, String, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let id: String = id.into();
    let open = app.settings_menu.as_deref() == Some(id.as_str());
    let label = options
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| value.clone());
    let t = *t;
    let on_pick = Rc::new(on_pick);
    let mut wrap = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .pl(px(10.))
        .pr(px(6.))
        .py(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(if open { t.line_strong } else { t.line })
        .bg(t.bg_panel)
        .cursor_pointer()
        .hover(move |s| s.border_color(t.line_strong))
        .text_size(px(12.))
        .text_color(t.text)
        .child(label)
        .child(icon("chevrons-up-down", 11., t.text_3))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                cx.stop_propagation();
                this.settings_menu = if this.settings_menu.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id.clone())
                };
                cx.notify();
            }),
        );
    if open {
        let mut menu = div()
            .min_w(px(170.))
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.line_strong)
            .bg(t.bg_panel)
            .shadow(super::sh_float());
        for (val, lab) in options {
            let is_active = val == value;
            let on_pick = on_pick.clone();
            menu = menu.child(
                div()
                    .h(px(26.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(if is_active { t.accent } else { t.text_2 })
                    .when(is_active, |d| d.bg(t.accent_bg))
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg_hover))
                    .child(lab)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            this.settings_menu = None;
                            on_pick(this, val.clone(), cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        wrap = wrap.child(deferred(
            gpui::anchored().snap_to_window_with_margin(px(8.)).child(
                div().occlude().mt(px(30.)).child(menu),
            ),
        ));
    }
    wrap
}

/// `.set-stepper` — 26×26 ± buttons around a mono value cell.
pub fn set_stepper(
    value: i64,
    min: i64,
    max: i64,
    t: &Tokens,
    on_set: impl Fn(&mut AgentIdeApp, i64, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let t = *t;
    let on_set = Rc::new(on_set);
    let step_btn = |name: &'static str,
                    next: i64,
                    on_set: Rc<dyn Fn(&mut AgentIdeApp, i64, &mut Context<AgentIdeApp>)>,
                    cx: &mut Context<AgentIdeApp>| {
        div()
            .w(px(26.))
            .h(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |s| s.bg(t.bg_hover))
            .child(icon(name, 11., t.text_2))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                    on_set(this, next, cx);
                    cx.notify();
                }),
            )
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_panel)
        .overflow_hidden()
        .child(step_btn("minus", (value - 1).max(min), on_set.clone(), cx))
        .child(
            div()
                .min_w(px(30.))
                .py(px(4.))
                .flex()
                .justify_center()
                .border_l_1()
                .border_r_1()
                .border_color(t.line)
                .text_size(px(12.))
                .font_family(FONT_MONO_FALLBACK)
                .child(format!("{value}")),
        )
        .child(step_btn("plus", (value + 1).min(max), on_set, cx))
}

/// `.set-slider` — draggable track. Geometry (origin x, width) is captured
/// each paint into `app.slider_geom[id]` via a canvas overlay; the global
/// mouse-move handler in `app.rs` resolves drags (`dragging = id`).
/// `hue` renders the 4px rainbow gradient variant.
pub fn set_slider(
    id: &'static str,
    value: f32,
    min: f32,
    max: f32,
    hue: bool,
    t: &Tokens,
    geom: Rc<RefCell<HashMap<&'static str, (f32, f32)>>>,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let ratio = ((value - min) / (max - min)).clamp(0., 1.);
    let t = *t;
    let mut track = div()
        .relative()
        .flex_1()
        .h(px(4.))
        .rounded_full()
        .overflow_hidden()
        .cursor_pointer();
    if hue {
        // hsl(0..360) in 6 gradient segments, like the CSS rainbow.
        let mut bands = div().absolute().inset_0().flex().flex_row();
        for i in 0..6 {
            let from = gpui::hsla((i as f32 * 60.) / 360., 0.5, 0.5, 1.0);
            let to = gpui::hsla(((i + 1) as f32 * 60. % 360.) / 360., 0.5, 0.5, 1.0);
            bands = bands.child(div().flex_1().h_full().bg(gpui::linear_gradient(
                90.,
                gpui::linear_color_stop(from, 0.),
                gpui::linear_color_stop(to, 1.),
            )));
        }
        track = track.child(bands);
    } else {
        track = track
            .bg(gpui::rgba(0x2a272426))
            .child(div().absolute().left_0().top_0().bottom_0().w(gpui::relative(ratio)).bg(t.accent));
    }
    // Record track geometry for drag math (window coords).
    let geom_for_paint = geom.clone();
    track = track.child(
        canvas(
            move |bounds, _w, _cx| {
                geom_for_paint
                    .borrow_mut()
                    .insert(id, (bounds.origin.x.into(), bounds.size.width.into()));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full(),
    );

    // 12px knob riding the track (HTML range thumb).
    let knob = div()
        .absolute()
        .top(px(-4.))
        .left(gpui::relative(ratio))
        .ml(px(-6.))
        .w(px(12.))
        .h(px(12.))
        .rounded_full()
        .bg(if hue { gpui::rgb(0xffffff) } else { t.accent })
        .border_1()
        .border_color(if hue { t.line_strong } else { t.accent });

    div()
        .relative()
        .flex_1()
        .h(px(14.))
        .flex()
        .items_center()
        .cursor_pointer()
        .child(track.child(knob))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                this.dragging = Some(id);
                let x: f32 = ev.position.x.into();
                this.apply_slider(id, x);
                cx.notify();
            }),
        )
}

/// `.seg-wrap` — inline segmented control, active = white card + sh-1.
pub fn seg_wrap(
    items: &[(&'static str, &'static str)],
    active: &str,
    t: &Tokens,
    on_pick: impl Fn(&mut AgentIdeApp, &'static str, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let t = *t;
    let on_pick = Rc::new(on_pick);
    let mut wrap = div()
        .flex()
        .flex_row()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_sunk);
    for (id, label) in items {
        let id = *id;
        let is_active = active == id;
        let on_pick = on_pick.clone();
        wrap = wrap.child(
            div()
                .px(px(14.))
                .py(px(4.))
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(if is_active { t.text } else { t.text_3 })
                .when(is_active, |d| d.bg(t.bg_panel).shadow(super::sh1()))
                .cursor_pointer()
                .child(*label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        on_pick(this, id, cx);
                        cx.notify();
                    }),
                ),
        );
    }
    wrap
}

/// `.set-code-preview` — the appearance page's static diff sample;
/// font size follows the「代码字号」setting.
pub fn set_code_preview(code_size: f32, t: &Tokens) -> Div {
    let line = |kind: &'static str, ln: &'static str, code: &'static str| {
        let (bg, bar) = match kind {
            "removed" => (gpui::rgba(0xb4503b14), t.danger),
            _ => (gpui::rgba(0x6a8f7a14), t.sage),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .py(px(2.))
            .bg(bg)
            .border_l_2()
            .border_color(bar)
            .child(
                div()
                    .w(px(32.))
                    .pr(px(8.))
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(t.text_4)
                    .border_r_1()
                    .border_color(t.line)
                    .child(div().w_full().flex().justify_end().child(ln)),
            )
            .child(div().pl(px(10.)).text_size(px(code_size)).text_color(t.text).child(code))
    };
    div()
        .mx(px(16.))
        .mb(px(16.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_panel)
        .overflow_hidden()
        .font_family(FONT_MONO_FALLBACK)
        .child(line("removed", "1", "return a + b;"))
        .child(line("added", "1", "const result = a + b;"))
        .child(line("added", "2", "return result;"))
}

/// `.set-input` — 200px mono text field shell around a uikit `TextInput`.
pub fn set_input_box(input: gpui::Entity<moonlit_uikit::TextInput>, width: f32, t: &Tokens) -> Div {
    div()
        .w(px(width))
        .px(px(10.))
        .py(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_panel)
        .text_size(px(12.))
        .font_family(FONT_MONO_FALLBACK)
        .child(input)
}

/// `.set-list-head` — section heading row with the accent「新建」button.
pub fn set_list_head(
    title: &'static str,
    desc: &'static str,
    t: &Tokens,
    on_add: impl Fn(&mut AgentIdeApp, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let t = *t;
    div()
        .mt(px(24.))
        .mb(px(8.))
        .flex()
        .flex_row()
        .items_end()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(title),
                )
                .when(!desc.is_empty(), |d| {
                    d.child(
                        div()
                            .mt(px(3.))
                            .max_w(px(540.))
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(t.text_3)
                            .child(desc),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .px(px(6.))
                .py(px(4.))
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(t.accent)
                .cursor_pointer()
                .hover(move |s| s.bg(t.accent_bg))
                .child(icon("plus", 11., t.accent))
                .child("新建")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                        on_add(this, cx);
                        cx.notify();
                    }),
                ),
        )
}

/// `.set-empty` — centered empty state inside a card.
pub fn set_empty(title: &'static str, desc: &'static str, t: &Tokens) -> Div {
    div()
        .py(px(28.))
        .px(px(16.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(t.text)
                .child(title),
        )
        .child(div().text_size(px(11.5)).text_color(t.text_3).child(desc))
}

/// `.ibtn`-style 26×26 icon button scoped to settings rows.
fn row_ibtn(
    name: &'static str,
    t: &Tokens,
    on_click: impl Fn(&mut AgentIdeApp, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let t = *t;
    div()
        .w(px(26.))
        .h(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(t.bg_hover))
        .child(icon(name, 13., t.text_2))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                on_click(this, cx);
                cx.notify();
            }),
        )
}

/// `btn btn--sm` — the legacy small button (also used for 登出 / 取消 / 保存).
pub fn sm_btn(
    label: impl Into<SharedString>,
    accent: bool,
    t: &Tokens,
    on_click: impl Fn(&mut AgentIdeApp, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> Div {
    let t = *t;
    let (bg, fg) = if accent { (t.accent, t.bg) } else { (t.bg_panel, t.text) };
    div()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(if accent { t.accent } else { t.line })
        .bg(bg)
        .text_size(px(12.))
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| if accent { s.bg(t.accent_soft) } else { s.bg(t.bg_hover) })
        .child(label.into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                on_click(this, cx);
                cx.notify();
            }),
        )
}

// ====================================================================
// CRUD field tables (legacy CrudSection definitions, verbatim copy)
// ====================================================================

use crate::app::{CrudField, CrudFieldKind};

static RULE_FIELDS: &[CrudField] = &[
    CrudField { key: "name", label: "名称", required: true, placeholder: "例如 始终用简体中文回答", kind: CrudFieldKind::Text },
    CrudField {
        key: "trigger",
        label: "触发方式",
        required: false,
        placeholder: "",
        kind: CrudFieldKind::Select(&[("always", "总是"), ("path", "按文件路径"), ("manual", "手动")]),
    },
    CrudField { key: "content", label: "规则内容", required: false, placeholder: "用一句话描述对 Agent 的约束…", kind: CrudFieldKind::Textarea },
];

static SKILL_FIELDS: &[CrudField] = &[
    CrudField { key: "name", label: "名称", required: true, placeholder: "例如 pdf", kind: CrudFieldKind::Text },
    CrudField { key: "desc", label: "描述", required: false, placeholder: "这个技能在什么场景下使用…", kind: CrudFieldKind::Textarea },
];

static SUBAGENT_FIELDS: &[CrudField] = &[
    CrudField { key: "name", label: "名称", required: true, placeholder: "例如 doc-reviewer", kind: CrudFieldKind::Text },
    CrudField { key: "desc", label: "职责描述", required: false, placeholder: "这个子 Agent 负责…", kind: CrudFieldKind::Textarea },
    CrudField { key: "tools", label: "可用工具", required: false, placeholder: "逗号分隔，如 read,grep,write", kind: CrudFieldKind::Text },
];

static COMMAND_FIELDS: &[CrudField] = &[
    CrudField { key: "name", label: "命令名", required: true, placeholder: "例如 /review", kind: CrudFieldKind::Text },
    CrudField { key: "prompt", label: "提示词", required: false, placeholder: "触发该命令时注入的提示…", kind: CrudFieldKind::Textarea },
];

static MCP_USER_FIELDS: &[CrudField] = &[
    CrudField { key: "name", label: "服务名称", required: true, placeholder: "例如 filesystem", kind: CrudFieldKind::Text },
    CrudField {
        key: "transport",
        label: "传输方式",
        required: false,
        placeholder: "",
        kind: CrudFieldKind::Select(&[("stdio", "stdio（本地命令）"), ("url", "URL（远程）")]),
    },
    CrudField { key: "command", label: "启动命令", required: false, placeholder: "stdio：如 npx -y @modelcontextprotocol/server-filesystem ./", kind: CrudFieldKind::Text },
    CrudField { key: "url", label: "服务地址", required: false, placeholder: "URL：如 http://127.0.0.1:9000/sse", kind: CrudFieldKind::Text },
];

static MCP_TEAM_FIELDS: &[CrudField] = &[
    CrudField { key: "name", label: "服务名称", required: true, placeholder: "例如 team-knowledge", kind: CrudFieldKind::Text },
    CrudField {
        key: "transport",
        label: "传输方式",
        required: false,
        placeholder: "",
        kind: CrudFieldKind::Select(&[("url", "URL（远程）"), ("stdio", "stdio（本地命令）")]),
    },
    CrudField { key: "command", label: "启动命令", required: false, placeholder: "stdio：如 uvx mcp-server-xxx", kind: CrudFieldKind::Text },
    CrudField { key: "url", label: "服务地址", required: false, placeholder: "URL：如 https://mcp.example.com/sse", kind: CrudFieldKind::Text },
];

/// `PLAN_TIERS` — 套餐与用量页的档位表.
static PLAN_TIERS: &[(&str, &str, &str, &str)] = &[
    ("free", "Free", "¥0/月", "基础 Agent 用量"),
    ("pro", "Pro", "¥149/月", "更高 Agent 用量上限"),
    ("pro_plus", "Pro+", "¥449/月", "3 倍用量上限 + 后台并发"),
];

/// 渠道供应商兜底表 (`CHANNEL_PROVIDER_FALLBACK`).
static PROVIDER_FALLBACK: &[(&str, &str, &str)] = &[
    ("deepseek", "DeepSeek", "https://api.deepseek.com/anthropic"),
    ("anthropic", "Anthropic", "https://api.anthropic.com"),
    ("openai", "OpenAI", "https://api.openai.com/v1"),
    ("kimi-api", "Kimi (Moonshot)", "https://api.moonshot.cn/anthropic"),
    ("kimi-coding", "Kimi Coding", "https://api.kimi.com/coding/v1"),
    ("zhipu", "智谱 GLM", "https://open.bigmodel.cn/api/paas/v4"),
    ("minimax", "MiniMax", "https://api.minimaxi.com/anthropic"),
    ("doubao", "豆包", "https://ark.cn-beijing.volces.com/api/v3"),
    ("qwen", "通义千问", "https://dashscope.aliyuncs.com/compatible-mode/v1"),
    ("google", "Google Gemini", "https://generativelanguage.googleapis.com"),
    ("custom", "自定义", ""),
];

/// 导航分组 (`NAV`): (page, 中文标签, lucide icon).
static NAV_GROUPS: &[&[(crate::SettingsPage, &str, &str)]] = &[
    &[
        (crate::SettingsPage::General, "通用", "settings"),
        (crate::SettingsPage::Appearance, "外观", "palette"),
    ],
    &[
        (crate::SettingsPage::Plan, "套餐与用量", "credit-card"),
        (crate::SettingsPage::Agents, "Agent", "sparkles"),
        (crate::SettingsPage::Tab, "自动补全", "corner-down-right"),
        (crate::SettingsPage::Models, "模型", "boxes"),
    ],
    &[
        (crate::SettingsPage::Rules, "规则 · 技能 · 子 Agent", "book-open"),
        (crate::SettingsPage::Tools, "工具与 MCP", "puzzle"),
    ],
];

// ====================================================================
// Shell + pages
// ====================================================================

impl AgentIdeApp {
    /// `.settings-fullscreen` — fixed inset, `240px | 1fr` grid over `--bg`.
    pub(crate) fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_settings_data();
        let t = self.t;
        let mut root = div()
            .absolute()
            .inset_0()
            .flex()
            .flex_row()
            .bg(t.bg)
            .text_color(t.text)
            // Click anywhere closes the open select dropdown.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    if this.settings_menu.is_some() {
                        this.settings_menu = None;
                        cx.notify();
                    }
                }),
            )
            .child(self.settings_nav(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(
                        div()
                            .id("settings-content")
                            .size_full()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(720.))
                                    .pt(px(56.))
                                    .px(px(32.))
                                    .pb(px(80.))
                                    .flex()
                                    .flex_col()
                                    .child(self.render_settings_body(cx)),
                            ),
                    )
                    // `.settings-close` — 28×28 at top 16 / right 20.
                    .child(
                        div()
                            .absolute()
                            .top(px(16.))
                            .right(px(20.))
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.line)
                            .bg(t.bg_panel)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child(icon("x", 14., t.text_2))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.settings_open = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        if self.crud_modal.is_some() {
            root = root.child(self.render_crud_modal(cx));
        }
        root
    }

    /// `.settings-nav` — 240px column: search, 返回, three rule-separated
    /// groups, spacer, user foot.
    fn settings_nav(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let page = self.state.settings.page;
        let search = self.settings_input("nav:search", "搜索设置…", cx);
        let user = self.auth_profile.clone();
        let display_name = user
            .as_ref()
            .and_then(|u| u.get("displayName").and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "本地工作区".into());
        let avatar = user
            .as_ref()
            .and_then(|u| u.get("avatar").and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| display_name.chars().take(1).collect());
        let plan_label = user
            .as_ref()
            .and_then(|u| u.get("plan"))
            .and_then(|p| p.get("label"))
            .and_then(|v| v.as_str())
            .map(|l| format!("{l} 套餐"))
            .unwrap_or_else(|| "个人配置".into());

        let mut nav = div()
            .id("settings-nav")
            .w(px(240.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .pt(px(10.))
            .pb(px(10.))
            .px(px(8.))
            .bg(t.bg_sunk)
            .border_r_1()
            .border_color(t.line)
            .overflow_y_scroll()
            // search box
            .child(
                div()
                    .mb(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .child(icon("search", 12., t.text_3))
                    .child(div().flex_1().text_size(px(12.)).child(search)),
            )
            // 返回
            .child(
                div()
                    .my(px(4.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(t.text_2)
                    .cursor_pointer()
                    .hover(move |s| s.bg(t.bg_hover))
                    .child(icon("arrow-left", 13., t.text_2))
                    .child("返回")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                            this.settings_open = false;
                            cx.notify();
                        }),
                    ),
            );
        for (gi, group) in NAV_GROUPS.iter().enumerate() {
            let mut g = div()
                .flex()
                .flex_col()
                .py(px(4.))
                .when(gi > 0, |d| d.mt(px(4.)).border_t_1().border_color(t.line));
            for (target, label, icon_name) in group.iter() {
                let target = *target;
                let is_active = page == target;
                g = g.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .text_size(px(13.))
                        .text_color(if is_active { t.text } else { t.text_2 })
                        .when(is_active, |d| {
                            d.bg(t.bg_panel)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .shadow(super::sh1())
                        })
                        .cursor_pointer()
                        .hover(move |s| s.bg(t.bg_hover).text_color(t.text))
                        .child(icon(icon_name, 13., if is_active { t.accent } else { t.text_3 }))
                        .child(*label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                this.set_settings_page(target, cx);
                            }),
                        ),
                );
            }
            nav = nav.child(g);
        }
        nav.child(div().flex_1())
            // `.settings-nav-foot`
            .child(
                div()
                    .mt(px(8.))
                    .pt(px(10.))
                    .px(px(8.))
                    .pb(px(0.))
                    .border_t_1()
                    .border_color(t.line)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(t.text)
                            .text_color(t.bg)
                            .font_family(FONT_SERIF)
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(if avatar.is_empty() { "月".to_string() } else { avatar }),
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
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(t.text)
                                    .truncate()
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(t.text_3)
                                    .font_family(FONT_MONO_FALLBACK)
                                    .child(plan_label),
                            ),
                    ),
            )
    }

    fn render_settings_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.state.settings.page {
            crate::SettingsPage::General => self.set_page_general(cx).into_any_element(),
            crate::SettingsPage::Appearance => self.set_page_appearance(cx).into_any_element(),
            crate::SettingsPage::Plan => self.set_page_plan(cx).into_any_element(),
            crate::SettingsPage::Agents => self.set_page_agents(cx).into_any_element(),
            crate::SettingsPage::Tab => self.set_page_tab(cx).into_any_element(),
            crate::SettingsPage::Models => self.set_page_models(cx).into_any_element(),
            crate::SettingsPage::Rules => self.set_page_rules(cx).into_any_element(),
            crate::SettingsPage::Tools => self.set_page_tools(cx).into_any_element(),
        }
    }

    // ---- 通用 -------------------------------------------------------------------

    fn set_page_general(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let acct_open = self.acct_open;
        let acct_saving = self.acct_saving;
        let name_input = self.settings_input("acct:name", "月夜开发者", cx);
        let ws_input = self.settings_input("acct:ws", "default_workspace", cx);
        let avatar_input = self.settings_input("acct:avatar", "月", cx);
        let user = self.auth_profile.clone();

        let (acct_title, acct_desc, email) = match &user {
            Some(u) => (
                u.get("displayName")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("月夜账户")
                    .to_string(),
                format!(
                    "{} · {} 套餐",
                    u.get("email").and_then(|v| v.as_str()).unwrap_or("—"),
                    u.get("plan")
                        .and_then(|p| p.get("label"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Free")
                ),
                u.get("email").and_then(|v| v.as_str()).unwrap_or("—").to_string(),
            ),
            None => (
                "月夜账户".to_string(),
                "管理账户、计费与许可证".to_string(),
                "—".to_string(),
            ),
        };

        // 「编辑 / 收起」expander button.
        let edit_btn = div()
            .h(px(26.))
            .px(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .rounded(px(6.))
            .border_1()
            .border_color(t.line)
            .bg(t.bg_panel)
            .text_size(px(12.))
            .cursor_pointer()
            .hover(move |s| s.bg(t.bg_hover))
            .child(if acct_open { "收起" } else { "编辑" })
            .child(icon(if acct_open { "chevron-up" } else { "pencil" }, 11., t.text_2))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.acct_open = !this.acct_open;
                    if this.acct_open {
                        let u = this.auth_profile.clone().unwrap_or_default();
                        let get = |k: &str| {
                            u.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
                        };
                        for (key, val) in [
                            ("acct:name", get("displayName")),
                            ("acct:ws", get("workspace")),
                            ("acct:avatar", get("avatar")),
                        ] {
                            if let Some(input) = this.settings_inputs.get(key) {
                                input.update(cx, |i, cx| i.set_text(val, cx));
                            }
                        }
                    }
                    cx.notify();
                }),
            );

        let mut acct_card = set_card(&t).child(set_row(
            acct_title,
            Some(div().child(acct_desc).into_any_element()),
            Some(edit_btn.into_any_element()),
            !acct_open,
            false,
            &t,
        ));
        if acct_open {
            acct_card = acct_card
                .child(srow(
                    "显示名",
                    "在头像与个人资料中展示",
                    Some(set_input_box(name_input, 200., &t).into_any_element()),
                    false,
                    &t,
                ))
                .child(srow(
                    "工作区",
                    "当前账户的工作区标识",
                    Some(set_input_box(ws_input, 200., &t).into_any_element()),
                    false,
                    &t,
                ))
                .child(srow(
                    "头像字符",
                    "头像中显示的单个字符",
                    Some(set_input_box(avatar_input, 200., &t).into_any_element()),
                    false,
                    &t,
                ))
                .child(srow(
                    "邮箱",
                    "账户登录邮箱（不可修改）",
                    Some(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_3)
                            .font_family(FONT_MONO_FALLBACK)
                            .child(email)
                            .into_any_element(),
                    ),
                    true,
                    &t,
                ))
                .child(
                    div()
                        .px(px(14.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(t.line)
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.))
                        .child(sm_btn("取消", false, &t, |this, _cx| this.acct_open = false, cx))
                        .child(sm_btn(
                            if acct_saving { "保存中…" } else { "保存" },
                            true,
                            &t,
                            |this, cx| {
                                if !this.acct_saving {
                                    this.save_account(cx);
                                }
                            },
                            cx,
                        )),
                );
        }

        div()
            .flex()
            .flex_col()
            .child(set_h1("通用", &t))
            .child(acct_card)
            .child(set_section_label("通知", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "系统通知",
                        "Agent 完成或需要审批时弹出系统级通知",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:sysNotif", true), &t, |this, _| {
                                this.s_flip("moonlit:s:sysNotif", true)
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "警告通知",
                        "工具失败、token 超限等以应用内 toast 提示",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:warnNotif", false), &t, |this, _| {
                                this.s_flip("moonlit:s:warnNotif", false)
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "系统托盘图标",
                        "在系统托盘显示工作区图标",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:tray", true), &t, |this, _| {
                                this.s_flip("moonlit:s:tray", true)
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "完成提示音",
                        "Agent 完成响应时播放提示音",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:doneSound", false), &t, |this, _| {
                                this.s_flip("moonlit:s:doneSound", false)
                            }, cx)
                            .into_any_element(),
                        ),
                        true,
                        &t,
                    )),
            )
            .child(set_section_label("隐私", &t))
            .child(
                set_card(&t).child(set_row(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.))
                        .child(icon("lock", 11., t.text))
                        .child("隐私模式"),
                    Some(
                        div()
                            .child("代码不会用于模型训练。代码可能被临时存储以提供 Background Agent 等功能。")
                            .into_any_element(),
                    ),
                    Some(
                        set_select(
                            "privacy",
                            self.s_str("moonlit:s:privacy", "privacy"),
                            vec![
                                ("privacy".into(), "隐私模式".into()),
                                ("shared".into(), "标准模式".into()),
                            ],
                            &t,
                            self,
                            |this, v, _| this.s_set_str("moonlit:s:privacy", &v),
                            cx,
                        )
                        .into_any_element(),
                    ),
                    true,
                    false,
                    &t,
                )),
            )
            .child(
                div()
                    .mt(px(24.))
                    .flex()
                    .flex_row()
                    .child(sm_btn("登出", false, &t, |this, cx| this.logout(cx), cx)),
            )
    }

    // ---- 外观 -------------------------------------------------------------------

    fn set_page_appearance(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let hue = self.s_int("moonlit:s:hue", 24);
        let intensity = self.s_int("moonlit:s:intensity", 0);
        let ui_size = self.s_int("moonlit:s:uiSize", 13);
        let code_size = self.s_int("moonlit:s:codeSize", 12);
        let theme = self.s_str("moonlit:settings:theme", "auto");
        let ui_family = self.settings_pinput("moonlit:s:uiFamily", "System default", cx);
        let code_family = self.settings_pinput("moonlit:s:codeFamily", "System monospace", cx);
        let geom = self.slider_geom.clone();

        let hue_control = div()
            .w(px(240.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .child(set_slider("slider-hue", hue as f32, 0., 360., true, &t, geom.clone(), cx))
            .child(
                div()
                    .w(px(14.))
                    .h(px(14.))
                    .flex_none()
                    .rounded_full()
                    .border_1()
                    .border_color(t.line)
                    .bg(gpui::hsla(hue as f32 / 360., 0.5, 0.5, 1.0)),
            );
        let intensity_control = div()
            .w(px(200.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(set_slider(
                "slider-intensity",
                intensity as f32,
                0.,
                100.,
                false,
                &t,
                geom,
                cx,
            ))
            .child(
                div()
                    .w(px(36.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .text_size(px(11.))
                    .text_color(t.text_3)
                    .font_family(FONT_MONO_FALLBACK)
                    .child(format!("{intensity}%")),
            );

        div()
            .flex()
            .flex_col()
            .child(set_h1("外观", &t))
            .child(set_card(&t).child(srow(
                "主题",
                "在浅色 / 深色 / 高对比度之间选择",
                Some(
                    set_select(
                        "theme",
                        theme,
                        vec![
                            ("auto".into(), "跟随系统".into()),
                            ("light".into(), "浅色".into()),
                            ("dark".into(), "深色".into()),
                        ],
                        &t,
                        self,
                        |this, v, cx| this.set_theme_choice(&v, cx),
                        cx,
                    )
                    .into_any_element(),
                ),
                true,
                &t,
            )))
            .child(set_section_label("颜色", &t))
            .child(
                set_card(&t)
                    .child(srow("色相", "调整界面着色", Some(hue_control.into_any_element()), false, &t))
                    .child(srow("强度", "控制着色应用强度", Some(intensity_control.into_any_element()), false, &t))
                    .child(srow(
                        "降低透明度",
                        "用不透明背景替换半透明效果",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:reduceTransp", false), &t, |this, _| {
                                this.s_flip("moonlit:s:reduceTransp", false)
                            }, cx)
                            .into_any_element(),
                        ),
                        true,
                        &t,
                    )),
            )
            .child(set_section_label("字体", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "界面字号",
                        "UI 文字大小（标题、菜单、面板）",
                        Some(
                            set_stepper(ui_size, 11, 18, &t, |this, v, _| {
                                this.s_set_int("moonlit:s:uiSize", v)
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "代码字号",
                        "代码编辑器与 diff 视图字号",
                        Some(
                            set_stepper(code_size, 10, 20, &t, |this, v, _| {
                                this.s_set_int("moonlit:s:codeSize", v)
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "界面字体",
                        "覆盖默认 UI 字体",
                        Some(set_input_box(ui_family, 200., &t).into_any_element()),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "代码字体",
                        "覆盖代码编辑器字体",
                        Some(set_input_box(code_family, 200., &t).into_any_element()),
                        true,
                        &t,
                    ))
                    .child(set_code_preview(code_size as f32, &t)),
            )
            .child(set_section_label("隐私", &t))
            .child(set_card(&t).child(srow(
                "隐藏邮箱地址",
                "在 UI 中部分遮蔽邮箱",
                Some(
                    set_toggle(self.s_bool("moonlit:s:hideEmail", false), &t, |this, _| {
                        this.s_flip("moonlit:s:hideEmail", false)
                    }, cx)
                    .into_any_element(),
                ),
                true,
                &t,
            )))
    }

    // ---- 套餐与用量 ----------------------------------------------------------------

    fn set_page_plan(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let user = self.auth_profile.clone().unwrap_or_default();
        let plan = user.get("plan").cloned().unwrap_or_default();
        let tier = plan.get("tier").and_then(|v| v.as_str()).unwrap_or("free").to_string();
        let label = plan.get("label").and_then(|v| v.as_str()).unwrap_or("Free").to_string();
        let price = plan.get("priceLabel").and_then(|v| v.as_str()).unwrap_or("¥0/月").to_string();
        let renews = plan
            .get("renewsAt")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                let date = s.split('T').next()?;
                let mut parts = date.split('-');
                let _year = parts.next()?;
                let month: u32 = parts.next()?.parse().ok()?;
                let day: u32 = parts.next()?.parse().ok()?;
                Some(format!("{month} 月 {day} 日刷新"))
            })
            .unwrap_or_else(|| "套餐周期未设置".to_string());
        let cap = match user.get("monthlyCapRmb") {
            Some(serde_json::Value::String(s)) if !s.is_empty() => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => "unlimited".to_string(),
        };
        // Legacy `findIndex` semantics: unknown tier (e.g. debug) → -1 → upgrade = tiers[0].
        let upgrade = match PLAN_TIERS.iter().position(|(id, ..)| *id == tier) {
            Some(idx) => PLAN_TIERS.get(idx + 1).copied(),
            None => PLAN_TIERS.first().copied(),
        };
        let connected = self.connected;
        let m = self.metrics.clone();
        let plan_progress = m
            .plan_progress
            .as_ref()
            .map(|p| {
                format!(
                    "{}/{}",
                    p.get("completed").and_then(|v| v.as_u64()).unwrap_or(0),
                    p.get("total").and_then(|v| v.as_u64()).unwrap_or(0)
                )
            })
            .unwrap_or_else(|| "—".to_string());

        let mono_val = |v: String, strong: bool| {
            div()
                .text_size(px(if strong { 13. } else { 12. }))
                .text_color(if strong { t.text } else { t.text_3 })
                .font_family(FONT_MONO_FALLBACK)
                .child(v)
                .into_any_element()
        };

        // 左卡: 当前套餐
        let mut current_card = set_card(&t)
            .p(px(16.))
            .child(
                div()
                    .mb(px(8.))
                    .pl(px(2.))
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(t.text_3)
                    .child("当前套餐"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_family(FONT_SERIF)
                            .text_size(px(22.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_3)
                            .font_family(FONT_MONO_FALLBACK)
                            .child(price),
                    ),
            )
            .child(div().mt(px(8.)).mb(px(12.)).text_size(px(12.)).text_color(t.text_3).child(renews));
        current_card = if tier != "free" {
            current_card.child(div().flex().flex_row().child(sm_btn(
                "降级为 Free",
                false,
                &t,
                |this, cx| this.save_profile_patch(serde_json::json!({"plan": "free"}), "套餐已更新", false, cx),
                cx,
            )))
        } else {
            current_card.child(div().text_size(px(11.)).text_color(t.text_4).child("基础档位"))
        };

        // 右卡: 可升级 / 已是最高档
        let upgrade_card = match upgrade {
            Some((up_tier, up_label, up_price, up_blurb)) => {
                let up_tier: &'static str = up_tier;
                let info_btn = div()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.info)
                    .bg(t.info)
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xffffff))
                    .cursor_pointer()
                    .child(format!("升级到 {up_label}"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.save_profile_patch(
                                serde_json::json!({ "plan": up_tier }),
                                "套餐已更新",
                                false,
                                cx,
                            );
                        }),
                    );
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .p(px(16.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(gpui::rgba(0x4e6b8933))
                    .bg(gpui::rgba(0x4e6b890d))
                    .child(
                        div()
                            .mb(px(8.))
                            .pl(px(2.))
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(t.info)
                            .child("可升级"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family(FONT_SERIF)
                                    .text_size(px(22.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(up_label),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.text_3)
                                    .font_family(FONT_MONO_FALLBACK)
                                    .child(up_price),
                            ),
                    )
                    .child(div().mt(px(8.)).mb(px(12.)).text_size(px(12.)).text_color(t.text_3).child(up_blurb))
                    .child(div().flex().flex_row().child(info_btn))
            }
            None => set_card(&t)
                .flex_1()
                .p(px(16.))
                .child(
                    div()
                        .mb(px(8.))
                        .pl(px(2.))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(t.text_3)
                        .child("已是最高档"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text_3)
                        .child("当前已是 Pro+，享受最高用量与后台并发。"),
                ),
        };

        div()
            .flex()
            .flex_col()
            .child(set_h1("套餐与用量", &t))
            .child(
                div()
                    .mb(px(16.))
                    .flex()
                    .flex_row()
                    .gap(px(12.))
                    .child(current_card.flex_1())
                    .child(upgrade_card),
            )
            .child(set_section_label("本周期用量", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "累计 tokens",
                        "来自后端实时指标",
                        Some(mono_val(
                            if connected { format!("{}", m.total_tokens) } else { "—".into() },
                            true,
                        )),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "工具调用",
                        "本会话已执行的工具次数",
                        Some(mono_val(
                            if connected { format!("{}", m.tool_calls) } else { "—".into() },
                            true,
                        )),
                        false,
                        &t,
                    ))
                    .child(srow("计划进度", "已完成 / 总步骤", Some(mono_val(plan_progress, false)), true, &t)),
            )
            .child(set_section_label("按需用量", &t))
            .child(set_card(&t).child(srow(
                "月度上限",
                "设置硬性额度，或保持无上限",
                Some(
                    set_select(
                        "cap",
                        cap,
                        vec![
                            ("unlimited".into(), "无上限".into()),
                            ("100".into(), "¥100".into()),
                            ("300".into(), "¥300".into()),
                            ("500".into(), "¥500".into()),
                        ],
                        &t,
                        self,
                        |this, v, cx| {
                            this.save_profile_patch(
                                serde_json::json!({ "monthlyCapRmb": v }),
                                "月度上限已更新",
                                false,
                                cx,
                            )
                        },
                        cx,
                    )
                    .into_any_element(),
                ),
                true,
                &t,
            )))
    }

    // ---- Agent --------------------------------------------------------------------

    fn set_page_agents(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let transitions = self.settings_pinput("moonlit:s:transitions", "例如 agent->plan", cx);
        let toggle =
            |this: &mut Self, key: &'static str, default: bool, cx: &mut Context<Self>| -> AnyElement {
                set_toggle(this.s_bool(key, default), &t, move |app, _| app.s_flip(key, default), cx)
                    .into_any_element()
            };

        div()
            .flex()
            .flex_col()
            .child(set_h1("Agent", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "Ctrl + Enter 发送",
                        "启用后，Ctrl+Enter 发送，Enter 换行",
                        Some(
                            set_toggle(self.state.settings.submit_with_ctrl_enter, &t, |this, _| {
                                this.state.settings.submit_with_ctrl_enter =
                                    !this.state.settings.submit_with_ctrl_enter;
                                this.s_set_bool(
                                    "moonlit:s:submitCtrl",
                                    this.state.settings.submit_with_ctrl_enter,
                                );
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "排队消息",
                        "Agent 运行中再次发送时的默认行为",
                        Some(
                            set_select(
                                "queueMode",
                                self.s_str("moonlit:s:queueMode", "after"),
                                vec![
                                    ("after".into(), "在当前消息后发送".into()),
                                    ("interrupt".into(), "立即打断".into()),
                                    ("discard".into(), "丢弃新消息".into()),
                                ],
                                &t,
                                self,
                                |this, v, _| this.s_set_str("moonlit:s:queueMode", &v),
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "用量摘要",
                        "底部对话区是否显示本次会话用量",
                        Some(
                            set_select(
                                "usage",
                                self.s_str("moonlit:s:usage", "auto"),
                                vec![
                                    ("auto".into(), "自动".into()),
                                    ("always".into(), "始终显示".into()),
                                    ("never".into(), "不显示".into()),
                                ],
                                &t,
                                self,
                                |this, v, _| this.s_set_str("moonlit:s:usage", &v),
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "Agent 自动补全",
                        "输入指令时给上下文建议",
                        Some(toggle(self, "moonlit:s:autocomplete", true, cx)),
                        true,
                        &t,
                    )),
            )
            .child(set_section_label("上下文", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "联网搜索工具",
                        "本地偏好开关；真实能力请使用会话状态栏里的联网搜索开关",
                        Some(toggle(self, "moonlit:s:webSearch", true, cx)),
                        false,
                        &t,
                    ))
                    .child(set_row(
                        "自动接受联网搜索",
                        Some(div().child("开启 Run Everything 时跳过搜索类工具的审批").into_any_element()),
                        Some(toggle(self, "moonlit:s:autoAcceptSearch", true, cx)),
                        false,
                        true, // `.dim`
                        &t,
                    ))
                    .child(srow(
                        "网页抓取工具",
                        "本地偏好开关；真实能力由后端工具配置决定",
                        Some(toggle(self, "moonlit:s:webFetch", true, cx)),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "层级 Cursor Ignore",
                        "对所有子目录套用 .cursorignore 文件，更改后需重启",
                        Some(toggle(self, "moonlit:s:hierIgnore", false, cx)),
                        false,
                        &t,
                    ))
                    .child(set_row(
                        "跳过软链接",
                        Some(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.))
                                .child(icon("alert-triangle", 10., t.warn))
                                .child("仅当仓库存在大量软链接时启用，更改后需重启")
                                .into_any_element(),
                        ),
                        Some(toggle(self, "moonlit:s:skipSymlinks", false, cx)),
                        true,
                        false,
                        &t,
                    )),
            )
            .child(set_section_label("自动运行", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "执行权限模式（后端）",
                        "同步到 Agent Debug 后端的当前会话工具权限",
                        Some(
                            set_select(
                                "permMode",
                                self.permission_mode.clone(),
                                vec![
                                    ("auto".into(), "自动（默认）".into()),
                                    ("plan".into(), "计划模式（只读优先）".into()),
                                    ("bypass".into(), "绕过（全部放行）".into()),
                                ],
                                &t,
                                self,
                                |this, v, cx| this.set_permission_mode_backend(v, cx),
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "自动运行模式",
                        "Agent 执行命令、MCP 工具与文件写入的策略",
                        Some(
                            set_select(
                                "autoRun",
                                self.s_str("moonlit:s:autoRun", "everything"),
                                vec![
                                    ("everything".into(), "执行全部（无沙箱）".into()),
                                    ("low-risk".into(), "仅低风险".into()),
                                    ("ask".into(), "每次询问".into()),
                                ],
                                &t,
                                self,
                                |this, v, _| this.s_set_str("moonlit:s:autoRun", &v),
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "自动批准的模式切换",
                        "哪些模式切换可不询问直接通过",
                        Some(set_input_box(transitions, 200., &t).into_any_element()),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "浏览器保护",
                        "禁止 Agent 自动运行 Browser 类工具",
                        Some(toggle(self, "moonlit:s:browserProt", false, cx)),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "MCP 工具保护",
                        "禁止 Agent 自动运行 MCP 工具",
                        Some(toggle(self, "moonlit:s:mcpProt", false, cx)),
                        true,
                        &t,
                    )),
            )
            .child(set_section_label("内联编辑与终端", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "旧版终端工具",
                        "不支持的 shell 配置下使用旧版终端实现",
                        Some(toggle(self, "moonlit:s:legacyTerm", false, cx)),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "自动解析链接",
                        "粘贴到 Quick Edit (⌘K) 时自动识别链接",
                        Some(toggle(self, "moonlit:s:parseLinks", false, cx)),
                        true,
                        &t,
                    )),
            )
    }

    // ---- 自动补全 (Tab) -------------------------------------------------------------

    fn set_page_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let ignored = self.settings_pinput("moonlit:s:ignored", "例如 *.md, **/generated/**", cx);
        let toggle =
            |this: &mut Self, key: &'static str, default: bool, cx: &mut Context<Self>| -> AnyElement {
                set_toggle(this.s_bool(key, default), &t, move |app, _| app.s_flip(key, default), cx)
                    .into_any_element()
            };

        div()
            .flex()
            .flex_col()
            .child(set_h1("自动补全", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "月夜 Tab",
                        "基于近期编辑的上下文感知多行建议",
                        Some(toggle(self, "moonlit:s:tab", true, cx)),
                        false,
                        &t,
                    ))
                    .child(set_row(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .child("部分接受")
                            .child(icon("info", 10., t.text_3)),
                        Some(div().child("通过 Ctrl+→ 接受建议的下一个词").into_any_element()),
                        Some(toggle(self, "moonlit:s:partial", false, cx)),
                        false,
                        false,
                        &t,
                    ))
                    .child(srow(
                        "注释中的建议",
                        "在注释区域也允许 Tab 触发",
                        Some(toggle(self, "moonlit:s:comments", true, cx)),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "纯空白建议",
                        "允许仅修改换行与缩进的建议",
                        Some(toggle(self, "moonlit:s:whitespace", false, cx)),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "自动 Import",
                        "为 TypeScript 自动引入所需模块",
                        Some(toggle(self, "moonlit:s:imports", true, cx)),
                        false,
                        &t,
                    ))
                    .child(set_row(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .child("Python 自动 Import")
                            .child(set_badge("BETA", &t)),
                        Some(div().child("为 Python 启用自动 Import，仍处于 Beta 阶段").into_any_element()),
                        Some(toggle(self, "moonlit:s:pyImports", false, cx)),
                        false,
                        false,
                        &t,
                    ))
                    .child(srow(
                        "忽略的文件",
                        "Glob 模式：匹配的文件不会被建议",
                        Some(set_input_box(ignored, 200., &t).into_any_element()),
                        true,
                        &t,
                    )),
            )
    }

    // ---- 模型 (渠道 + Tavily) -------------------------------------------------------

    fn set_page_models(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        div()
            .flex()
            .flex_col()
            .child(set_h1("模型", &t))
            .child(self.channels_section(cx))
            .child(self.tavily_section(cx))
    }

    /// 「模型配置（渠道）」: head + channel rows + the optional accent form.
    fn channels_section(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let loaded = self.channels_loaded;
        let channels = self.channels.clone();
        let has_draft = self.channel_draft.is_some();

        let mini_chip = |text: String, color: gpui::Rgba| {
            div()
                .h(px(18.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(t.line)
                .bg(t.bg_panel)
                .text_size(px(10.))
                .text_color(color)
                .child(text)
        };

        // 头部: 标题 + 「添加渠道」accent button.
        let add_btn = div()
            .h(px(26.))
            .px(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.))
            .rounded(px(6.))
            .border_1()
            .border_color(t.accent)
            .bg(t.accent)
            .text_size(px(12.))
            .text_color(t.bg)
            .cursor_pointer()
            .when(has_draft, |d| d.opacity(0.5))
            .child(icon("plus", 12., t.bg))
            .child("添加渠道")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| this.new_channel(cx)),
            );

        let mut card = set_card(&t);
        if !loaded {
            card = card.child(
                div()
                    .p(px(24.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(t.text_3)
                    .child("加载中…"),
            );
        } else if channels.is_empty() {
            card = card.child(
                div()
                    .p(px(24.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(t.text_3)
                    .child("暂无渠道。点击「添加渠道」配置供应商与 API Key，模型将出现在编辑器的模型选择器中。"),
            );
        }
        let count = channels.len();
        for (i, ch) in channels.into_iter().enumerate() {
            let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let provider_label = ch
                .get("providerLabel")
                .or_else(|| ch.get("provider"))
                .and_then(|v| v.as_str())
                .unwrap_or("custom")
                .to_string();
            let key_set = ch.get("apiKeySet").and_then(|v| v.as_bool()).unwrap_or(false);
            let enabled = ch.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let models: Vec<String> = ch
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let models_desc = if models.is_empty() {
                "无模型".to_string()
            } else {
                format!("{} 个模型 · {}", models.len(), models.join(", "))
            };
            let id = ch.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ch_for_toggle = ch.clone();
            let ch_for_edit = ch.clone();
            let id_for_delete = id.clone();
            let title = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(name)
                .child(mini_chip(provider_label, t.text_2))
                .child(mini_chip(
                    if key_set { "已配置 Key".into() } else { "未配置 Key".into() },
                    if key_set { t.accent } else { t.text_3 },
                ));
            let controls = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(set_toggle(enabled, &t, move |this, cx| {
                    this.toggle_channel_enabled(ch_for_toggle.clone(), cx)
                }, cx))
                .child(row_ibtn("pencil", &t, move |this, cx| {
                    this.edit_channel(ch_for_edit.clone(), cx)
                }, cx))
                .child(row_ibtn("trash-2", &t, move |this, cx| {
                    this.remove_channel(id_for_delete.clone(), cx)
                }, cx));
            card = card.child(set_row(
                title,
                Some(
                    div()
                        .font_family(FONT_MONO_FALLBACK)
                        .text_size(px(10.))
                        .child(models_desc)
                        .into_any_element(),
                ),
                Some(controls.into_any_element()),
                i == count - 1,
                false,
                &t,
            ));
        }

        let mut section = div()
            .mt(px(16.))
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(8.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("模型配置（渠道）"),
                    )
                    .child(add_btn),
            )
            .child(card);
        if has_draft {
            section = section.child(self.channel_form(cx));
        }
        section
    }

    /// `ChannelForm` — accent-bordered editor card.
    fn channel_form(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let name_input = self.settings_input("ch:name", "例如 DeepSeek 主力", cx);
        let base_input = self.settings_input("ch:base", "https://...", cx);
        let key_input = self.settings_input("ch:key", "sk-...", cx);
        let draft = self.channel_draft.as_ref().expect("channel draft open");
        let provider = draft.provider.clone();
        let is_edit = draft.id.is_some();
        let api_key_set = draft.api_key_set;
        let enabled = draft.enabled;
        let fetching = draft.fetching_models;
        let saving = self.channel_saving;
        let model_enabled = draft.model_enabled.clone();

        // Provider options: backend list, else fallback table.
        let provider_options: Vec<(String, String)> = if self.provider_types.is_empty() {
            PROVIDER_FALLBACK.iter().map(|(p, l, _)| (p.to_string(), l.to_string())).collect()
        } else {
            self.provider_types
                .iter()
                .filter_map(|p| {
                    let id = p.get("provider")?.as_str()?.to_string();
                    let label = p
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&id)
                        .to_string();
                    Some((id, label))
                })
                .collect()
        };
        let provider_defaults: Vec<(String, String)> = if self.provider_types.is_empty() {
            PROVIDER_FALLBACK.iter().map(|(p, _, u)| (p.to_string(), u.to_string())).collect()
        } else {
            self.provider_types
                .iter()
                .filter_map(|p| {
                    Some((
                        p.get("provider")?.as_str()?.to_string(),
                        p.get("defaultBaseUrl").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    ))
                })
                .collect()
        };
        let base_placeholder = provider_defaults
            .iter()
            .find(|(p, _)| *p == provider)
            .map(|(_, u)| u.clone())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "https://...".to_string());

        // 模型列表 rows.
        let mut model_rows = div().flex().flex_col();
        if model_enabled.is_empty() {
            model_rows = model_rows.child(
                div()
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(t.text_3)
                    .child("暂无模型，点击「添加模型」录入模型 ID（如 deepseek-chat）。"),
            );
        }
        for (i, m_enabled) in model_enabled.iter().enumerate() {
            let m_enabled = *m_enabled;
            let id_input = self.settings_input(&format!("ch:m{i}:id"), "模型 ID（如 deepseek-chat）", cx);
            let mname_input = self.settings_input(&format!("ch:m{i}:name"), "显示名（可选）", cx);
            model_rows = model_rows.child(
                div()
                    .mb(px(6.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .child(set_input_box(id_input, 0., &t).flex_1())
                    .child(set_input_box(mname_input, 0., &t).flex_1())
                    .child(set_toggle(m_enabled, &t, move |this, _| {
                        if let Some(draft) = &mut this.channel_draft {
                            if let Some(flag) = draft.model_enabled.get_mut(i) {
                                *flag = !*flag;
                            }
                        }
                    }, cx))
                    .child(row_ibtn("trash-2", &t, move |this, cx| {
                        this.remove_model_row(i, cx)
                    }, cx)),
            );
        }

        set_card(&t)
            .mt(px(8.))
            .border_color(t.accent)
            .child(srow(
                "渠道名称",
                "自定义显示名称",
                Some(set_input_box(name_input, 200., &t).into_any_element()),
                false,
                &t,
            ))
            .child(srow(
                "供应商",
                "切换后自动填充默认接口地址",
                Some(
                    set_select(
                        "ch:provider",
                        provider,
                        provider_options,
                        &t,
                        self,
                        move |this, v, cx| {
                            // 切换供应商时，若 baseUrl 为空或等于旧默认值则自动填充。
                            let old_default = this
                                .channel_draft
                                .as_ref()
                                .map(|d| d.provider.clone())
                                .and_then(|p| {
                                    provider_defaults.iter().find(|(id, _)| *id == p).map(|(_, u)| u.clone())
                                })
                                .unwrap_or_default();
                            let new_default = provider_defaults
                                .iter()
                                .find(|(id, _)| *id == v)
                                .map(|(_, u)| u.clone())
                                .unwrap_or_default();
                            if let Some(draft) = &mut this.channel_draft {
                                draft.provider = v.clone();
                            }
                            if let Some(input) = this.settings_inputs.get("ch:base") {
                                let current = input.read(cx).text().to_string();
                                if current.is_empty() || current == old_default {
                                    input.update(cx, |i, cx| i.set_text(new_default, cx));
                                }
                            }
                        },
                        cx,
                    )
                    .into_any_element(),
                ),
                false,
                &t,
            ))
            .child(srow(
                "Base URL",
                "留空则使用该供应商默认地址",
                Some(
                    set_input_box(
                        {
                            base_input.update(cx, |i, cx| i.set_placeholder(base_placeholder, cx));
                            base_input
                        },
                        200.,
                        &t,
                    )
                    .into_any_element(),
                ),
                false,
                &t,
            ))
            .child(set_row(
                "API Key",
                Some(
                    div()
                        .child(if is_edit {
                            "留空表示不修改已保存的 Key"
                        } else {
                            "用于该渠道下所有模型的鉴权"
                        })
                        .into_any_element(),
                ),
                Some(
                    set_input_box(
                        {
                            key_input.update(cx, |i, cx| {
                                i.set_placeholder(
                                    if api_key_set { "已配置（输入新 Key 以覆盖）" } else { "sk-..." },
                                    cx,
                                )
                            });
                            key_input
                        },
                        200.,
                        &t,
                    )
                    .into_any_element(),
                ),
                false,
                false,
                &t,
            ))
            .child(srow(
                "启用渠道",
                "关闭后该渠道的模型不会用于 Agent",
                Some(
                    set_toggle(enabled, &t, |this, _| {
                        if let Some(draft) = &mut this.channel_draft {
                            draft.enabled = !draft.enabled;
                        }
                    }, cx)
                    .into_any_element(),
                ),
                model_enabled.is_empty(),
                &t,
            ))
            // 模型列表 block.
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb(px(8.))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(px(12.)).text_color(t.text_2).child("模型列表"))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.))
                                    .child({
                                        let label =
                                            if fetching { "获取中…" } else { "从供应商获取" };
                                        div()
                                            .h(px(26.))
                                            .px(px(10.))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(5.))
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(t.line)
                                            .bg(t.bg_panel)
                                            .text_size(px(12.))
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(t.bg_hover))
                                            .child(icon("download", 12., t.text_2))
                                            .child(label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                                    this.fetch_channel_models(cx)
                                                }),
                                            )
                                    })
                                    .child(
                                        div()
                                            .h(px(26.))
                                            .px(px(10.))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(5.))
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(t.line)
                                            .bg(t.bg_panel)
                                            .text_size(px(12.))
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(t.bg_hover))
                                            .child(icon("plus", 12., t.text_2))
                                            .child("添加模型")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                                    this.add_model_row(cx)
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    .child(model_rows),
            )
            // footer 取消 / 保存渠道.
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(t.line)
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.))
                    .child(sm_btn("取消", false, &t, |this, _| this.channel_draft = None, cx))
                    .child(sm_btn(
                        if saving { "保存中…" } else { "保存渠道" },
                        true,
                        &t,
                        |this, cx| {
                            if !this.channel_saving {
                                this.save_channel(cx);
                            }
                        },
                        cx,
                    )),
            )
    }

    /// 「Tavily 搜索配置」 section.
    fn tavily_section(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let draft = self.tavily.clone();
        let saving = self.tavily_saving;
        let key_input = self.settings_input("tavily:key", "tvly-...", cx);

        let mut card = set_card(&t);
        match &draft {
            None => {
                card = card.child(
                    div()
                        .p(px(24.))
                        .flex()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(t.text_3)
                        .child("加载中…"),
                );
            }
            Some(d) => {
                key_input.update(cx, |i, cx| {
                    i.set_placeholder(
                        if d.api_key_set { "已配置（输入新 Key 以覆盖）" } else { "tvly-..." },
                        cx,
                    )
                });
                let opts = |values: &[&str]| -> Vec<(String, String)> {
                    values.iter().map(|v| (v.to_string(), v.to_string())).collect()
                };
                card = card
                    .child(srow(
                        "启用 Tavily",
                        "开启后，联网搜索优先使用这里保存的 Tavily 配置",
                        Some(
                            set_toggle(d.enabled, &t, |this, _| {
                                if let Some(d) = &mut this.tavily {
                                    d.enabled = !d.enabled;
                                }
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "Provider",
                        "当前版本固定使用 Tavily",
                        Some(
                            div()
                                .text_size(px(12.))
                                .text_color(t.text_2)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child("Tavily")
                                .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "默认搜索主题",
                        "general 适合通用检索，news 更偏最新新闻",
                        Some(
                            set_select(
                                "tavily:topic",
                                d.topic.clone(),
                                opts(&["general", "news"]),
                                &t,
                                self,
                                |this, v, _| {
                                    if let Some(d) = &mut this.tavily {
                                        d.topic = v;
                                    }
                                },
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "默认搜索深度",
                        "advanced 结果更强但成本更高",
                        Some(
                            set_select(
                                "tavily:depth",
                                d.search_depth.clone(),
                                opts(&["basic", "advanced"]),
                                &t,
                                self,
                                |this, v, _| {
                                    if let Some(d) = &mut this.tavily {
                                        d.search_depth = v;
                                    }
                                },
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "默认时间范围",
                        "仅在工具未显式传 freshness 时生效",
                        Some(
                            set_select(
                                "tavily:range",
                                d.time_range.clone(),
                                vec![
                                    ("".into(), "不限".into()),
                                    ("day".into(), "day".into()),
                                    ("week".into(), "week".into()),
                                    ("month".into(), "month".into()),
                                    ("year".into(), "year".into()),
                                ],
                                &t,
                                self,
                                |this, v, _| {
                                    if let Some(d) = &mut this.tavily {
                                        d.time_range = v;
                                    }
                                },
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "默认抽取深度",
                        "用于网页正文抽取，advanced 更适合复杂页面",
                        Some(
                            set_select(
                                "tavily:extract",
                                d.extract_depth.clone(),
                                opts(&["basic", "advanced"]),
                                &t,
                                self,
                                |this, v, _| {
                                    if let Some(d) = &mut this.tavily {
                                        d.extract_depth = v;
                                    }
                                },
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(set_row(
                        "API Key",
                        Some(
                            div()
                                .child(if d.api_key_set {
                                    "已配置（输入新 Key 以覆盖，留空表示不修改）"
                                } else {
                                    "请输入 Tavily API Key"
                                })
                                .into_any_element(),
                        ),
                        Some(set_input_box(key_input, 200., &t).into_any_element()),
                        true,
                        false,
                        &t,
                    ));
            }
        }

        let loaded = draft.is_some();
        div()
            .mt(px(16.))
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(8.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Tavily 搜索配置"),
                    ),
            )
            .child(card)
            .child(
                div().mt(px(8.)).flex().flex_row().justify_end().child(sm_btn(
                    if saving { "保存中…" } else { "保存 Tavily 配置" },
                    true,
                    &t,
                    move |this, cx| {
                        if loaded && !this.tavily_saving {
                            this.save_tavily(cx);
                        }
                    },
                    cx,
                )),
            )
    }

    // ---- 规则 · 技能 · 子 Agent ------------------------------------------------------

    fn set_page_rules(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let scope = self.s_str("moonlit:s:rulesScope", "user");

        fn rule_summary(it: &serde_json::Value) -> String {
            let trigger = match it.get("trigger").and_then(|v| v.as_str()) {
                Some("path") => "按路径",
                Some("manual") => "手动",
                _ => "总是",
            };
            let content = it
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("（无内容）");
            format!("{trigger} · {content}")
        }
        fn skill_summary(it: &serde_json::Value) -> String {
            it.get("desc")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("（无描述）")
                .to_string()
        }
        fn subagent_summary(it: &serde_json::Value) -> String {
            let desc = it
                .get("desc")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("（无描述）");
            match it.get("tools").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
                Some(tools) => format!("{desc} · 工具：{tools}"),
                None => desc.to_string(),
            }
        }
        fn command_summary(it: &serde_json::Value) -> String {
            it.get("prompt")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("（无提示词）")
                .to_string()
        }

        div()
            .flex()
            .flex_col()
            .child(set_h1("规则 · 技能 · 子 Agent", &t))
            .child(
                div()
                    .mt(px(-4.))
                    .mb(px(16.))
                    .text_size(px(13.))
                    .text_color(t.text_3)
                    .child("为 Agent 提供领域知识与可复用工作流"),
            )
            .child(
                div().mb(px(14.)).flex().flex_row().child(seg_wrap(
                    &[("user", "我的"), ("team", "团队")],
                    &scope,
                    &t,
                    |this, id, _| this.s_set_str("moonlit:s:rulesScope", id),
                    cx,
                )),
            )
            .child(set_card(&t).child(srow(
                "包含第三方插件、技能与配置",
                "自动从其他工具导入 agent 配置",
                Some(
                    set_toggle(self.s_bool("moonlit:s:thirdParty", true), &t, |this, _| {
                        this.s_flip("moonlit:s:thirdParty", true)
                    }, cx)
                    .into_any_element(),
                ),
                true,
                &t,
            )))
            .child(self.crud_section(
                format!("moonlit:rules:{scope}:rules"),
                "规则",
                "用规则约束 Agent 行为，可按总是 / 文件路径 / 手动 触发",
                "新建规则",
                RULE_FIELDS,
                rule_summary,
                cx,
            ))
            .child(self.skills_section(cx))
            .child(self.crud_section(
                format!("moonlit:rules:{scope}:skills"),
                "自定义技能",
                "本地备忘的技能条目（仅记录，不影响 Agent；Agent 实际读取上方已发现的 SKILL.md）",
                "新建技能",
                SKILL_FIELDS,
                skill_summary,
                cx,
            ))
            .child(self.crud_section(
                format!("moonlit:rules:{scope}:subagents"),
                "子 Agent",
                "为复杂任务创建专门 Agent，可由主 Agent 并发调用",
                "新建子 Agent",
                SUBAGENT_FIELDS,
                subagent_summary,
                cx,
            ))
            .child(self.crud_section(
                format!("moonlit:rules:{scope}:commands"),
                "命令",
                "在对话中以 / 前缀触发的可复用工作流",
                "新建命令",
                COMMAND_FIELDS,
                command_summary,
                cx,
            ))
    }

    /// `CrudSection` — list head + card of rows + empty state, backed by a
    /// ConfigStore JSON array under `storage_key`.
    fn crud_section(
        &mut self,
        storage_key: String,
        title: &'static str,
        desc: &'static str,
        add_label: &'static str,
        fields: &'static [CrudField],
        summary: fn(&serde_json::Value) -> String,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.t;
        let items = self.crud_items(&storage_key);
        let name_key = fields[0].key;

        let key_for_add = storage_key.clone();
        let head = set_list_head(title, desc, &t, move |this, cx| {
            this.open_crud(key_for_add.clone(), title, add_label, fields, None, cx)
        }, cx);

        let mut card = set_card(&t);
        if items.is_empty() {
            let key_for_empty = storage_key.clone();
            card = card.child(
                div()
                    .py(px(28.))
                    .px(px(16.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(t.text)
                            .child(format!("暂无{title}")),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.text_3)
                            .child(format!("点击「{add_label}」创建第一条。")),
                    )
                    .child(div().mt(px(6.)).child(sm_btn(add_label, false, &t, move |this, cx| {
                        this.open_crud(key_for_empty.clone(), title, add_label, fields, None, cx)
                    }, cx))),
            );
        }
        let count = items.len();
        for (i, item) in items.into_iter().enumerate() {
            let name = item
                .get(name_key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("（未命名）")
                .to_string();
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let key_for_edit = storage_key.clone();
            let key_for_remove = storage_key.clone();
            let item_for_edit = item.clone();
            let id_for_remove = id.clone();
            let controls = div()
                .flex()
                .flex_row()
                .gap(px(6.))
                .child(row_ibtn("pencil", &t, move |this, cx| {
                    this.open_crud(
                        key_for_edit.clone(),
                        title,
                        add_label,
                        fields,
                        Some(&item_for_edit),
                        cx,
                    )
                }, cx))
                .child(row_ibtn("trash-2", &t, move |this, cx| {
                    this.remove_crud_item(&key_for_remove, &id_for_remove, cx)
                }, cx));
            card = card.child(set_row(
                name,
                Some(div().child(summary(&item)).into_any_element()),
                Some(controls.into_any_element()),
                i == count - 1,
                false,
                &t,
            ));
        }
        div().flex().flex_col().child(head).child(card)
    }

    /// `DiscoveredSkillsSection` — read-only list from the backend skills API.
    fn skills_section(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let loading = self.skills.is_none() || self.skills_loading;
        let skills = self.skills.clone().unwrap_or_default();
        let open_name = self.skill_open.clone();
        let previews = self.skill_previews.clone();

        let head = div()
            .mt(px(16.))
            .mb(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("已发现技能"),
            )
            .child(row_ibtn("refresh-cw", &t, |this, _| this.load_skills(), cx));

        let mut card = set_card(&t);
        if loading {
            card = card.child(
                div()
                    .p(px(24.))
                    .flex()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(t.text_3)
                    .child("加载中…"),
            );
        } else if skills.is_empty() {
            card = card.child(set_empty(
                "未发现技能",
                "在工作区 .cursor/skills/<名称>/SKILL.md 放置技能即可被发现。",
                &t,
            ));
        }
        let count = skills.len();
        for (i, item) in skills.into_iter().enumerate() {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let scope_label = match item.get("scope").and_then(|v| v.as_str()) {
                Some("workspace") => "工作区".to_string(),
                Some("user") => "用户".to_string(),
                other => other.unwrap_or("—").to_string(),
            };
            let summary = item
                .get("summary")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("（无描述）")
                .to_string();
            let is_open = open_name.as_deref() == Some(name.as_str());
            let name_for_toggle = name.clone();
            let mut row = div()
                .px(px(16.))
                .py(px(14.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .when(i != count - 1, |d| d.border_b_1().border_color(t.line))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(13.))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(name.clone())
                                        .child(set_badge_str(scope_label, &t)),
                                )
                                .child(
                                    div()
                                        .mt(px(3.))
                                        .text_size(px(11.5))
                                        .line_height(px(16.))
                                        .text_color(t.text_3)
                                        .child(summary),
                                ),
                        )
                        .child(sm_btn(
                            if is_open { "收起" } else { "预览" },
                            false,
                            &t,
                            move |this, cx| this.toggle_skill(name_for_toggle.clone(), cx),
                            cx,
                        )),
                );
            if is_open {
                let content = previews
                    .get(&name)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "加载中…".to_string());
                row = row.child(
                    div()
                        .id(gpui::ElementId::Name(format!("skill-preview-{name}").into()))
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p(px(12.))
                        .rounded(px(8.))
                        .bg(t.bg_sunk)
                        .font_family(FONT_MONO_FALLBACK)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(t.text_2)
                        .child(content),
                );
            }
            card = card.child(row);
        }

        div()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .mb(px(8.))
                    .text_size(px(12.))
                    .text_color(t.text_3)
                    .child("来自工作区与用户目录的 SKILL.md，Agent 可通过 read_skill 工具按需读取"),
            )
            .child(card)
    }

    // ---- 工具与 MCP -----------------------------------------------------------------

    fn set_page_tools(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let section = self.s_str("moonlit:s:toolsSection", "home");

        fn mcp_summary(it: &serde_json::Value) -> String {
            let transport = it.get("transport").and_then(|v| v.as_str()).unwrap_or("stdio");
            if transport == "url" {
                format!(
                    "URL · {}",
                    it.get("url")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("（未填地址）")
                )
            } else {
                format!(
                    "stdio · {}",
                    it.get("command")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("（未填命令）")
                )
            }
        }

        div()
            .flex()
            .flex_col()
            .child(set_h1("工具与 MCP", &t))
            .child(
                div().mb(px(14.)).flex().flex_row().child(seg_wrap(
                    &[("home", "本地"), ("cloud", "云端")],
                    &section,
                    &t,
                    |this, id, _| this.s_set_str("moonlit:s:toolsSection", id),
                    cx,
                )),
            )
            .child(set_section_label("执行权限", &t))
            .child(set_card(&t).child(srow(
                "自动执行低风险工具",
                "沙箱内可写 allowlist 范围内的命令自动通过；其余仍需手动批准。",
                Some(
                    set_toggle(self.state.settings.auto_approve, &t, |this, _| {
                        this.state.settings.auto_approve = !this.state.settings.auto_approve;
                        this.s_set_bool("moonlit:autoApprove", this.state.settings.auto_approve);
                    }, cx)
                    .into_any_element(),
                ),
                true,
                &t,
            )))
            .child(set_section_label("浏览器", &t))
            .child(
                set_card(&t)
                    .child(srow(
                        "浏览器自动化",
                        "已连接到 Browser Tab",
                        Some(
                            set_select(
                                "browserAuto",
                                self.s_str("moonlit:s:browserAuto", "tab"),
                                vec![
                                    ("tab".into(), "Browser Tab".into()),
                                    ("external".into(), "外部 Chrome".into()),
                                    ("off".into(), "关闭".into()),
                                ],
                                &t,
                                self,
                                |this, v, _| this.s_set_str("moonlit:s:browserAuto", &v),
                                cx,
                            )
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "在 Browser Tab 显示 Localhost 链接",
                        "自动在 Browser Tab 中打开 localhost 链接",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:showLocalhost", true), &t, |this, _| {
                                this.s_flip("moonlit:s:showLocalhost", true)
                            }, cx)
                            .into_any_element(),
                        ),
                        false,
                        &t,
                    ))
                    .child(srow(
                        "在 Browser Tab 打开 Web 链接",
                        "自动在 Browser Tab 打开 http/https 链接",
                        Some(
                            set_toggle(self.s_bool("moonlit:s:openWeb", false), &t, |this, _| {
                                this.s_flip("moonlit:s:openWeb", false)
                            }, cx)
                            .into_any_element(),
                        ),
                        true,
                        &t,
                    )),
            )
            .child(
                div()
                    .mt(px(24.))
                    .mb(px(4.))
                    .text_size(px(11.))
                    .text_color(t.text_3)
                    .child("本地 MCP 服务"),
            )
            .child(
                div()
                    .mb(px(8.))
                    .text_size(px(11.))
                    .text_color(t.text_3)
                    .child("在当前工作区可用的 MCP 服务"),
            )
            .child(set_card(&t).child(set_empty(
                "暂无本地 MCP 摘要",
                "接入真实 MCP 服务后，这里会展示可用连接与状态。",
                &t,
            )))
            .child(self.crud_section(
                "moonlit:mcp:user".to_string(),
                "用户 MCP 服务",
                "为当前用户添加自定义 MCP 工具（本地保存）",
                "添加自定义 MCP",
                MCP_USER_FIELDS,
                mcp_summary,
                cx,
            ))
            .child(self.crud_section(
                "moonlit:mcp:team".to_string(),
                "团队 MCP 服务",
                "团队共享的 MCP（本地原型保存，非真正控制台同步）",
                "配置团队 MCP",
                MCP_TEAM_FIELDS,
                mcp_summary,
                cx,
            ))
    }

    // ---- CRUD modal -----------------------------------------------------------------

    /// `CrudModal` — 460px centered card above a dimmed overlay.
    fn render_crud_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t;
        let Some(state) = &self.crud_modal else {
            return div();
        };
        let title: SharedString = if state.editing_id.is_some() {
            format!("编辑{}", state.title).into()
        } else {
            state.add_label.into()
        };
        let fields = state.fields;
        let selects = state.selects.clone();

        let mut body = div().p(px(14.)).flex().flex_col().gap(px(12.));
        for f in fields {
            let mut label_row = div()
                .flex()
                .flex_row()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(t.text_3)
                .child(f.label);
            if f.required {
                label_row = label_row.child(div().text_color(t.danger).child(" *"));
            }
            let control: AnyElement = match &f.kind {
                CrudFieldKind::Select(options) => {
                    let key = f.key;
                    let value = selects
                        .get(key)
                        .cloned()
                        .or_else(|| options.first().map(|(v, _)| v.to_string()))
                        .unwrap_or_default();
                    set_select(
                        &format!("crud:{key}"),
                        value,
                        options.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(),
                        &t,
                        self,
                        move |this, v, _| {
                            if let Some(state) = &mut this.crud_modal {
                                state.selects.insert(key, v);
                            }
                        },
                        cx,
                    )
                    .into_any_element()
                }
                kind => {
                    let input = self.settings_input(&format!("crud:{}", f.key), f.placeholder, cx);
                    let mut box_ = set_input_box(input, 0., &t).w_full();
                    if matches!(kind, CrudFieldKind::Textarea) {
                        box_ = box_.min_h(px(76.)).items_start();
                    }
                    box_.into_any_element()
                }
            };
            body = body.child(div().flex().flex_col().gap(px(5.)).child(label_row).child(control));
        }

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000066))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                    this.crud_modal = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(460.))
                    .max_h(px(560.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .shadow(super::sh_float())
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _ev: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    // `.modal-head`
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(12.))
                            .border_b_1()
                            .border_color(t.line)
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(row_ibtn("x", &t, |this, _| this.crud_modal = None, cx)),
                    )
                    .child(div().id("crud-modal-body").flex_1().overflow_y_scroll().child(body))
                    // footer 取消 / 保存
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(t.line)
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .child(sm_btn("取消", false, &t, |this, _| this.crud_modal = None, cx))
                            .child(sm_btn("保存", true, &t, |this, cx| this.save_crud(cx), cx)),
                    ),
            )
    }
}

/// `.set-badge` for runtime strings (scope chips on discovered skills).
fn set_badge_str(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(t.accent_soft)
        .bg(t.accent_bg)
        .text_size(px(9.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.accent)
        .child(text.into())
}

