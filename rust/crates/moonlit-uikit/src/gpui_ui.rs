//! GPUI rendering layer for the native frontends.
//!
//! This module is only compiled with the `gpui-backend` feature. It bridges the
//! headless `Theme` tokens to GPUI colors and provides a reusable single-line
//! text input (generalized from GPUI's `input` example, with change events so
//! parent views can mirror edits into `DocCore`).

use std::ops::Range;

use gpui::{
    div, fill, hsla, point, prelude::*, px, relative, rgb, rgba, size, App, Bounds, ClipboardItem,
    Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Rgba, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window,
};
use gpui::actions;
use unicode_segmentation::UnicodeSegmentation;

use crate::{Theme, ToastKind};

/// Shared border tint used for dividers/panels across the apps.
pub const BORDER: u32 = 0xffffff20;

/// Map a [`ToastKind`] to its accent color.
pub fn toast_color(kind: ToastKind, theme: &Theme) -> Rgba {
    match kind {
        ToastKind::Info => hex(&theme.accent),
        ToastKind::Success => rgb(0x16a34a),
        ToastKind::Warning => rgb(0xd97706),
        ToastKind::Error => hex(&theme.danger),
    }
}

/// Parse a `#rrggbb` hex string into a GPUI [`Rgba`]. Falls back to magenta so
/// mistakes are visible rather than silent.
pub fn hex(color: &str) -> Rgba {
    let trimmed = color.trim_start_matches('#');
    u32::from_str_radix(trimmed, 16)
        .map(rgb)
        .unwrap_or_else(|_| rgb(0xff00ff))
}

/// Convenience accessors so views can do `theme.colors().accent`.
pub struct ThemeColors {
    pub background: Rgba,
    pub panel: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    pub danger: Rgba,
}

impl Theme {
    pub fn colors(&self) -> ThemeColors {
        ThemeColors {
            background: hex(&self.background),
            panel: hex(&self.panel),
            text: hex(&self.text),
            muted: hex(&self.muted),
            accent: hex(&self.accent),
            danger: hex(&self.danger),
        }
    }
}

/// Full design-token set ported 1:1 from the legacy React frontend
/// (`apps/agent-ide/public/styles.css`): Claude-inspired cream palette with an
/// amber accent. All values are GPUI [`Rgba`] ready for direct use in views.
#[derive(Clone, Copy)]
pub struct Tokens {
    // Surfaces
    pub bg: Rgba,
    pub bg_sunk: Rgba,
    pub bg_panel: Rgba,
    pub bg_input: Rgba,
    pub bg_hover: Rgba,
    pub bg_active: Rgba,
    pub bg_selection: Rgba,
    // Text
    pub text: Rgba,
    pub text_2: Rgba,
    pub text_3: Rgba,
    pub text_4: Rgba,
    pub text_inv: Rgba,
    // Lines
    pub line: Rgba,
    pub line_strong: Rgba,
    // Accent (amber / coral)
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub accent_bg: Rgba,
    pub accent_ring: Rgba,
    // Secondary accent
    pub sage: Rgba,
    pub sage_bg: Rgba,
    // Semantic
    pub danger: Rgba,
    pub danger_bg: Rgba,
    pub warn: Rgba,
    pub warn_bg: Rgba,
    pub info: Rgba,
    pub info_bg: Rgba,
    // Status dots
    pub dot_running: Rgba,
    pub dot_done: Rgba,
    pub dot_idle: Rgba,
    pub dot_blocked: Rgba,
    pub dot_queued: Rgba,
}

impl Tokens {
    /// Light (default) theme — `:root` in styles.css.
    pub fn claude_light() -> Self {
        Self {
            bg: rgb(0xfaf9f5),
            bg_sunk: rgb(0xf3f1e9),
            bg_panel: rgb(0xffffff),
            bg_input: rgb(0xffffff),
            bg_hover: rgba(0x2a27240a),      // rgba(42,39,36,0.04)
            bg_active: rgba(0x2a272412),     // rgba(42,39,36,0.07)
            bg_selection: rgba(0xc9644217),  // rgba(201,100,66,0.09)
            text: rgb(0x2a2724),
            text_2: rgb(0x5a564f),
            text_3: rgb(0x8a857c),
            text_4: rgb(0xb3ad9f),
            text_inv: rgb(0xfaf9f5),
            line: rgba(0x2a272417),          // rgba(42,39,36,0.09)
            line_strong: rgba(0x2a272424),   // rgba(42,39,36,0.14)
            accent: rgb(0xc96442),
            accent_soft: rgb(0xe2886a),
            accent_bg: rgba(0xc9644214),     // rgba(201,100,66,0.08)
            accent_ring: rgba(0xc9644247),   // rgba(201,100,66,0.28)
            sage: rgb(0x6a8f7a),
            sage_bg: rgba(0x6a8f7a1a),       // rgba(106,143,122,0.10)
            danger: rgb(0xb4503b),
            danger_bg: rgba(0xb4503b14),
            warn: rgb(0xb28632),
            warn_bg: rgba(0xb286321a),
            info: rgb(0x4e6b89),
            info_bg: rgba(0x4e6b8914),
            dot_running: rgb(0xc96442),
            dot_done: rgb(0x6a8f7a),
            dot_idle: rgb(0xb3ad9f),
            dot_blocked: rgb(0xb4503b),
            dot_queued: rgb(0xb28632),
        }
    }

    /// Dark theme — `html.theme-dark` in styles.css.
    pub fn claude_dark() -> Self {
        Self {
            bg: rgb(0x1c1b18),
            bg_sunk: rgb(0x181715),
            bg_panel: rgb(0x232220),
            bg_input: rgb(0x232220),
            bg_hover: rgba(0xffffff0d),      // rgba(255,255,255,0.05)
            bg_active: rgba(0xffffff14),     // rgba(255,255,255,0.08)
            bg_selection: rgba(0xe2886a29),  // rgba(226,136,106,0.16)
            text: rgb(0xece8df),
            text_2: rgb(0xb8b3a7),
            text_3: rgb(0x8a857c),
            text_4: rgb(0x5a564f),
            text_inv: rgb(0x1c1b18),
            line: rgba(0xffffff12),          // rgba(255,255,255,0.07)
            line_strong: rgba(0xffffff21),   // rgba(255,255,255,0.13)
            accent: rgb(0xe2886a),
            accent_soft: rgb(0xf0a586),
            accent_bg: rgba(0xe2886a24),     // rgba(226,136,106,0.14)
            accent_ring: rgba(0xe2886a6b),   // rgba(226,136,106,0.42)
            sage: rgb(0x8eb39d),
            sage_bg: rgba(0x8eb39d21),
            danger: rgb(0xd97a63),
            danger_bg: rgba(0xd97a6324),
            warn: rgb(0xd4ab57),
            warn_bg: rgba(0xd4ab5721),
            info: rgb(0x7da0c4),
            info_bg: rgba(0x7da0c421),
            dot_running: rgb(0xe2886a),
            dot_done: rgb(0x8eb39d),
            dot_idle: rgb(0x5a564f),
            dot_blocked: rgb(0xd97a63),
            dot_queued: rgb(0xd4ab57),
        }
    }

    /// Status-dot color for a session/run status string.
    pub fn dot_for_status(&self, status: &str) -> Rgba {
        match status {
            "running" | "in_progress" | "streaming" => self.dot_running,
            "completed" | "done" | "success" => self.dot_done,
            "failed" | "error" | "blocked" => self.dot_blocked,
            "pending" | "queued" => self.dot_queued,
            _ => self.dot_idle,
        }
    }
}

/// Font stacks from the legacy frontend (with Windows fallbacks).
pub const FONT_SANS: &str = "Inter";
pub const FONT_SANS_FALLBACK: &str = "Segoe UI";
pub const FONT_SERIF: &str = "Noto Serif SC";
pub const FONT_MONO: &str = "JetBrains Mono";
pub const FONT_MONO_FALLBACK: &str = "Consolas";

actions!(
    moonlit_text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Enter,
        CtrlEnter,
    ]
);

/// Register default Windows-friendly key bindings for [`TextInput`]. Call once
/// during application setup.
pub fn register_text_input_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("enter", Enter, Some("TextInput")),
        KeyBinding::new("ctrl-enter", CtrlEnter, Some("TextInput")),
    ]);
}

/// Events emitted by [`TextInput`] so a parent view can mirror the content.
#[derive(Clone, Debug)]
pub enum TextInputEvent {
    Changed(String),
    /// Plain Enter.
    Submit(String),
    /// Ctrl+Enter (used when "submit with Ctrl+Enter" is enabled).
    SubmitCtrl(String),
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    cursor_color: Rgba,
    selection_color: Rgba,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, initial: impl Into<SharedString>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: initial.into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            cursor_color: rgb(0x7c3aed),
            selection_color: rgba(0x7c3aed40),
        }
    }

    /// Override caret + selection colors (e.g. to the Claude amber accent).
    pub fn set_accent(&mut self, cursor: Rgba, selection: Rgba) {
        self.cursor_color = cursor;
        self.selection_color = selection;
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let len = self.content.len();
        self.selected_range = len..len;
        self.marked_range = None;
        cx.notify();
    }

    pub fn focus_handle_clone(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn emit_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed(self.content.to_string()));
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submit(self.content.to_string()));
    }

    fn ctrl_enter(&mut self, _: &CtrlEnter, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::SubmitCtrl(self.content.to_string()));
    }

    /// Update the placeholder (e.g. when the composer mode changes).
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
        self.emit_changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
        self.emit_changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + last_layout.x_for_index(range.start), bounds.top()),
            point(bounds.left() + last_layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let cursor_color = input.cursor_color;
        let selection_color = input.selection_color;
        let style = window.text_style();
        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0.6, 0.5))
        } else {
            (content, style.color)
        };
        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun { len: marked_range.start, ..run.clone() },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun { len: display_text.len() - marked_range.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);
        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    cursor_color,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(selected_range.start), bounds.top()),
                        point(bounds.left() + line.x_for_index(selected_range.end), bounds.bottom()),
                    ),
                    selection_color,
                )),
                None,
            )
        };
        PrepaintState { line: Some(line), cursor, selection }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx).unwrap();
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }
        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::ctrl_enter))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .size_full()
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
