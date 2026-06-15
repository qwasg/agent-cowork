//! Legacy `LoginScreen`: centered 360px card on the cream canvas with the
//! 40px 「月」 brand mark, email/password fields, accent submit and the
//! 跳过登录（调试） debug bypass.

use gpui::{div, prelude::*, px, Context, MouseButton, MouseDownEvent};
use moonlit_uikit::FONT_SERIF;

use super::sh_float;
use crate::app::AgentIdeApp;

impl AgentIdeApp {
    pub(crate) fn render_login(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        let field = |label: &'static str, input: gpui::Entity<moonlit_uikit::TextInput>| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(11.)).text_color(t.text_3).child(label))
                .child(
                    div()
                        .h(px(34.))
                        .flex()
                        .items_center()
                        .px(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t.line)
                        .bg(t.bg_input)
                        .text_size(px(13.))
                        .child(input),
                )
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(moonlit_uikit::FONT_SANS)
            .child(
                // `.auth-card`: 360px, radius 14
                div()
                    .w(px(360.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .pt(px(22.))
                    .px(px(22.))
                    .pb(px(18.))
                    .rounded(px(14.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.bg_panel)
                    .shadow(sh_float())
                    // brand row
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
                                    .rounded(px(10.))
                                    .bg(t.text)
                                    .text_color(t.text_inv)
                                    .text_size(px(20.))
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
                                            .text_color(t.text_3)
                                            .child("登录以继续"),
                                    ),
                            ),
                    )
                    .child(field("邮箱", self.auth_email.clone()))
                    .child(field("密码", self.auth_password.clone()))
                    .when_some(self.login_error.clone(), |d, err| {
                        d.child(
                            div()
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(px(6.))
                                .bg(t.danger_bg)
                                .text_size(px(11.5))
                                .text_color(t.danger)
                                .child(err),
                        )
                    })
                    // submit
                    .child(
                        div()
                            .h(px(36.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .bg(t.accent)
                            .text_color(gpui::rgb(0xffffff))
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.accent_soft))
                            .child(if self.login_busy {
                                "登录中…"
                            } else {
                                "登录"
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| this.do_login(cx)),
                            ),
                    )
                    // debug skip
                    .child(
                        div()
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(t.line)
                            .bg(t.bg_panel)
                            .text_size(px(13.))
                            .text_color(t.text_2)
                            .cursor_pointer()
                            .hover(move |s| s.bg(t.bg_hover))
                            .child("跳过登录（调试）")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                                    this.skip_login(cx)
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(t.text_4)
                            .text_center()
                            .child("本地原型 · 账户数据由后端账户系统管理"),
                    ),
            )
    }
}
