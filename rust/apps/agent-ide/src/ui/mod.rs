//! GPUI view layer for the Agent IDE, a 1:1 replica of the legacy React
//! frontend (`apps/agent-ide`): Claude cream palette, 36px titlebar, five
//! column body, 26px statusbar. Each submodule mirrors one legacy component.

pub mod auth;
pub mod chat;
pub mod composer;
pub mod icons;
pub mod inspector;
pub mod overlays;
pub mod settings;
pub mod sidebar;
pub mod statusbar;
pub mod titlebar;
pub mod workbench;

use gpui::{
    div, prelude::*, px, AnimationExt, BoxShadow, Context, Div, MouseButton, MouseDownEvent, Rgba,
    SharedString, Window,
};
use moonlit_uikit::Tokens;

use crate::app::AgentIdeApp;
use icons::icon;

/// `--sh-1`: 0 1px 2px rgba(42,39,36,0.04), 0 0 0 1px rgba(42,39,36,0.04)
pub fn sh1() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: gpui::rgba(0x2a27240a).into(),
            offset: gpui::point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
        },
        BoxShadow {
            color: gpui::rgba(0x2a27240a).into(),
            offset: gpui::point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
        },
    ]
}

/// Opaque floating surface: occludes siblings beneath, solid panel, border, shadow.
pub fn float_surface(t: &Tokens) -> Div {
    div()
        .occlude()
        .border_1()
        .border_color(t.line_strong)
        .bg(t.bg_float)
        .shadow(sh_float())
}

/// `--sh-float`: 0 12px 40px rgba(42,39,36,0.14), 0 0 0 1px rgba(42,39,36,0.06)
pub fn sh_float() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: gpui::rgba(0x2a272424).into(),
            offset: gpui::point(px(0.), px(12.)),
            blur_radius: px(40.),
            spread_radius: px(0.),
        },
        BoxShadow {
            color: gpui::rgba(0x2a27240f).into(),
            offset: gpui::point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
        },
    ]
}

/// A 6x6 status dot (`.status-dot`).
pub fn status_dot(color: Rgba) -> Div {
    div()
        .w(px(6.))
        .h(px(6.))
        .rounded_full()
        .bg(color)
        .flex_none()
}

/// Running status dot with the legacy 1.6s pulse.
pub fn pulse_dot(id: impl Into<gpui::ElementId>, color: Rgba) -> impl IntoElement {
    status_dot(color).with_animation(
        id,
        gpui::Animation::new(std::time::Duration::from_millis(1600))
            .repeat()
            .with_easing(gpui::pulsating_between(0.35, 1.0)),
        |dot, delta| dot.opacity(delta),
    )
}

/// Streaming caret: 2px amber bar blinking at ~1s (legacy `.stream-caret`).
pub fn stream_caret(id: impl Into<gpui::ElementId>, color: Rgba) -> impl IntoElement {
    div()
        .w(px(2.))
        .h(px(14.))
        .bg(color)
        .flex_none()
        .with_animation(
            id,
            gpui::Animation::new(std::time::Duration::from_millis(1000))
                .repeat()
                .with_easing(gpui::pulsating_between(0.1, 1.0)),
            |caret, delta| caret.opacity(if delta > 0.5 { 1.0 } else { 0.15 }),
        )
}

/// `.kbd` — 16px tall mono key hint.
pub fn kbd(label: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .h(px(16.))
        .px(px(4.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(t.line)
        .bg(t.bg_sunk)
        .text_size(px(10.))
        .text_color(t.text_3)
        .font_family(moonlit_uikit::FONT_MONO_FALLBACK)
        .child(label.into())
}

/// `.sec-head` — 10px uppercase section header used by the sidebar/inspector.
pub fn sec_head(icon_name: &'static str, label: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .px(px(12.))
        .pt(px(10.))
        .pb(px(4.))
        .text_size(px(10.))
        .text_color(t.text_4)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(icon(icon_name, 10., t.text_4))
        .child(label.into())
}

/// `.ibtn` — 26x26 icon button.
pub fn ibtn(
    icon_name: &'static str,
    size: f32,
    t: &Tokens,
    on_click: impl Fn(&mut AgentIdeApp, &mut Window, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> impl IntoElement {
    let hover_bg = t.bg_hover;
    div()
        .w(px(26.))
        .h(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(icon(icon_name, size, t.text_2))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, window, cx| on_click(this, window, cx)),
        )
}

/// `.btn` — 28px text button. `primary` renders the ink-on-cream variant.
pub fn btn(
    label: impl Into<SharedString>,
    primary: bool,
    t: &Tokens,
    on_click: impl Fn(&mut AgentIdeApp, &mut Window, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> impl IntoElement {
    let (bg, fg, hover_bg) = if primary {
        (t.text, t.text_inv, t.text_2)
    } else {
        (t.bg_panel, t.text, t.bg_sunk)
    };
    div()
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.line)
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(label.into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, window, cx| on_click(this, window, cx)),
        )
}

/// `.chip` — 22px capsule.
pub fn chip(label: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .h(px(22.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded_full()
        .border_1()
        .border_color(t.line)
        .bg(t.bg_sunk)
        .text_size(px(11.))
        .text_color(t.text_2)
        .child(label.into())
}

/// `.cmb-menu-item` / `.menu-item`: 26px dropdown row with icon and label.
pub fn menu_item(icon_name: &'static str, label: &'static str, active: bool, t: &Tokens) -> Div {
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
        .text_color(if active { t.accent } else { t.text_2 })
        .when(active, |d| {
            d.font_weight(gpui::FontWeight::SEMIBOLD).bg(t.accent_bg)
        })
        .cursor_pointer()
        .hover(move |s| s.bg(t.bg_selection).text_color(t.accent))
        .child(icon(
            icon_name,
            13.,
            if active { t.accent } else { t.text_3 },
        ))
        .child(div().flex_1().child(label))
}

/// One of the three 9px pane divider tracks with the centered 11x24 collapse
/// pill (`.pane-divider`); dragging the track resizes the adjacent pane.
pub fn pane_divider(
    kind: &'static str,
    collapsed: bool,
    t: &Tokens,
    on_toggle: impl Fn(&mut AgentIdeApp, &mut Window, &mut Context<AgentIdeApp>) + 'static,
    cx: &mut Context<AgentIdeApp>,
) -> impl IntoElement {
    let hover_bg = t.bg_hover;
    div()
        .w(px(9.))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.bg_sunk)
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .hover(move |s| s.bg(hover_bg))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                this.dragging = Some(kind);
                cx.notify();
            }),
        )
        .child(
            div()
                .w(px(11.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(t.line)
                .bg(t.bg_panel)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .child(icon(
                    if collapsed {
                        "chevron-right"
                    } else {
                        "chevron-left"
                    },
                    9.,
                    t.text_3,
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.dragging = None;
                        on_toggle(this, window, cx)
                    }),
                ),
        )
}
