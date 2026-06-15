//! GPUI desktop shell for DocForge.
//!
//! Renders the Word rich-text editor and the PPT native canvas on top of the
//! `DocCore`-backed editor states, wires `DocCore::observe` to repaint, and
//! drives compile/export.

use std::sync::Arc;

use futures_util::StreamExt;
use gpui::{
    div, prelude::*, px, rgb, rgba, App, Application, Bounds, Context, Entity, FocusHandle,
    Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    SharedString, Window, WindowBounds, WindowOptions,
};
use moonlit_doccore::{DocIR, ExportFormat, StyleInput, WordBlockType, WordMark};
use moonlit_sync::SyncClient;
use moonlit_uikit::{register_text_input_keybindings, TextInput, Theme};

use crate::{DocForgeMode, DocForgeState};

/// Inches -> pixels for the PPT canvas.
const PPT_SCALE: f32 = 80.0;

/// Tokio runtime handle, set by `main` before the GPUI loop starts, used to
/// spawn the embedded-sync websocket clients.
pub static RUNTIME: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();

/// Run the DocForge GPUI application on the current (main) thread.
pub fn run() {
    Application::new().run(|cx: &mut App| {
        register_text_input_keybindings(cx);
        let bounds = Bounds::centered(None, gpui::size(px(1280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(DocForgeApp::new),
        )
        .unwrap();
        cx.activate(true);

        // Auto-quit support for headless smoke verification.
        if std::env::var("MOONLIT_SMOKE").is_ok() {
            cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}

pub struct DocForgeApp {
    focus_handle: FocusHandle,
    state: DocForgeState,
    theme: Theme,
    composer: Entity<TextInput>,
    status: SharedString,
    drag_last: Option<Point<Pixels>>,
    /// When opened from the Agent IDE workspace tree, lock mode and show filename.
    workspace_binding: Option<WorkspaceBinding>,
    /// Set on any user-initiated edit; cleared on load/save. Used by the host
    /// IDE to render a dirty indicator and prompt to save.
    dirty: bool,
}

#[derive(Debug, Clone)]
pub struct WorkspaceBinding {
    pub path: String,
    pub mode: DocForgeMode,
    /// Opened from Agent IDE workspace: read-only document view, no chrome.
    pub preview_only: bool,
}

impl DocForgeApp {
    /// Construct the collaborative DocForge view (built-in workbench tab and the
    /// standalone window root). Connects both documents to the shared sync rooms
    /// so browser Yjs peers collaborate in real time.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::build(cx, true)
    }

    /// Construct an isolated DocForge view for a single workspace file. Does NOT
    /// connect to the shared sync rooms, so multiple open documents never share
    /// or clobber each other's CRDT state. Persistence is via REST save instead.
    pub fn new_standalone(cx: &mut Context<Self>) -> Self {
        Self::build(cx, false)
    }

    fn build(cx: &mut Context<Self>, connect_sync: bool) -> Self {
        let state = DocForgeState::default();
        let composer = cx.new(|cx| TextInput::new(cx, "", "在此输入文本，然后点击插入/替换…"));

        // Bridge DocCore mutations (including remote sync) to a repaint.
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<()>();
        let tx_word = tx.clone();
        state.word.core.observe(move |_evt| {
            let _ = tx_word.unbounded_send(());
        });
        let tx_ppt = tx.clone();
        state.ppt.core.observe(move |_evt| {
            let _ = tx_ppt.unbounded_send(());
        });
        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Connect each document to the embedded sync server so browser Yjs peers
        // sharing the room collaborate in real time. Skipped for per-file
        // standalone editors to keep documents isolated.
        if connect_sync {
            if let Some(handle) = RUNTIME.get() {
                let connections = [
                    (
                        "ws://127.0.0.1:1234/docforge-word",
                        state.word.core.doc().clone(),
                        tx.clone(),
                    ),
                    (
                        "ws://127.0.0.1:1234/docforge-ppt",
                        state.ppt.core.doc().clone(),
                        tx.clone(),
                    ),
                ];
                for (url, doc, repaint) in connections {
                    let handle = handle.clone();
                    handle.spawn(async move {
                        match SyncClient::connect(url.to_string(), doc, move || {
                            let _ = repaint.unbounded_send(());
                        })
                        .await
                        {
                            Ok(client) => {
                                // Keep the connection alive for the process lifetime.
                                std::mem::forget(client);
                            }
                            Err(err) => tracing::warn!("sync connect {url} failed: {err}"),
                        }
                    });
                }
            }
        }

        Self {
            focus_handle: cx.focus_handle(),
            state,
            theme: Theme::moonlit_dark(),
            composer,
            status: "就绪".into(),
            drag_last: None,
            workspace_binding: None,
            dirty: false,
        }
    }

    /// Whether there are unsaved edits since the last load/save.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag (call after a successful save).
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn mark_dirty(&mut self) {
        if self
            .workspace_binding
            .as_ref()
            .is_some_and(|b| b.preview_only)
        {
            return;
        }
        self.dirty = true;
    }

    /// Load IR from a workspace file: preview-only view (no editor chrome).
    pub fn load_workspace_ir(&mut self, path: String, ir: DocIR, cx: &mut Context<Self>) {
        let mode = match &ir {
            DocIR::Word { .. } => DocForgeMode::Word,
            DocIR::Ppt { .. } => DocForgeMode::Ppt,
        };
        self.workspace_binding = Some(WorkspaceBinding {
            path: path.clone(),
            mode,
            preview_only: true,
        });
        self.apply_ir(ir, cx);
        self.dirty = false;
        self.set_status(format!("已打开 {path}"));
    }

    pub fn read_ir(&self) -> DocIR {
        match self.state.mode {
            DocForgeMode::Word => self.state.word.core.read_document(),
            DocForgeMode::Ppt => self.state.ppt.core.read_document(),
        }
    }

    pub fn workspace_path(&self) -> Option<&str> {
        self.workspace_binding.as_ref().map(|b| b.path.as_str())
    }

    fn apply_ir(&mut self, ir: DocIR, cx: &mut Context<Self>) {
        let tmp = moonlit_doccore::DocCore::from_ir(
            ir,
            std::sync::Arc::new(moonlit_core::DefaultIdFactory::new()),
            None,
        );
        let update = tmp.encode_state_as_update();
        let doc_type = tmp.doc_type();
        let result = match doc_type {
            moonlit_doccore::DocType::Word => {
                self.state.mode = DocForgeMode::Word;
                self.state.word.core.apply_update(&update)
            }
            moonlit_doccore::DocType::Ppt => {
                self.state.mode = DocForgeMode::Ppt;
                if let DocIR::Ppt { slides } = tmp.read_document() {
                    self.state.ppt.selected_slide_id = slides.first().map(|s| s.id.clone());
                    self.state.ppt.selected_element_id = slides
                        .first()
                        .and_then(|s| s.elements.first())
                        .map(|e| e.id.clone());
                }
                self.state.ppt.core.apply_update(&update)
            }
        };
        if let Err(err) = result {
            self.set_status(format!("加载失败: {err}"));
        }
        cx.notify();
    }

    fn composer_text(&self, cx: &App) -> String {
        self.composer.read(cx).text().to_string()
    }

    fn clear_composer(&self, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.set_text("", cx));
    }

    fn set_status(&mut self, msg: impl Into<SharedString>) {
        self.status = msg.into();
    }

    // ---- Word actions -------------------------------------------------------

    fn add_paragraph(&mut self, cx: &mut Context<Self>) {
        let text = self.composer_text(cx);
        let after = self.state.word.selected_block_id.clone();
        if let Err(err) = self.state.word.insert_paragraph(after.as_deref(), &text) {
            self.set_status(format!("插入失败: {err}"));
        } else {
            self.clear_composer(cx);
            self.mark_dirty();
            self.set_status("已插入段落");
        }
        cx.notify();
    }

    fn add_heading(&mut self, level: u8, cx: &mut Context<Self>) {
        let text = self.composer_text(cx);
        let after = self.state.word.selected_block_id.clone();
        if let Err(err) = self
            .state
            .word
            .insert_heading(after.as_deref(), level, &text)
        {
            self.set_status(format!("插入失败: {err}"));
        } else {
            self.clear_composer(cx);
            self.mark_dirty();
            self.set_status(format!("已插入 H{level}"));
        }
        cx.notify();
    }

    fn replace_selected(&mut self, cx: &mut Context<Self>) {
        let text = self.composer_text(cx);
        if let Err(err) = self.state.word.replace_selected_text(&text) {
            self.set_status(format!("替换失败: {err}"));
        } else {
            self.mark_dirty();
            self.set_status("已替换文本");
        }
        cx.notify();
    }

    fn apply_mark(&mut self, mark: WordMark, cx: &mut Context<Self>) {
        let style = match mark {
            WordMark::Bold => StyleInput {
                bold: Some(true),
                ..Default::default()
            },
            WordMark::Italic => StyleInput {
                italic: Some(true),
                ..Default::default()
            },
            WordMark::Underline => StyleInput {
                underline: Some(true),
                ..Default::default()
            },
            WordMark::Strike => StyleInput {
                strike: Some(true),
                ..Default::default()
            },
            WordMark::Code => StyleInput {
                code: Some(true),
                ..Default::default()
            },
        };
        if let Err(err) = self.state.word.apply_toolbar_style(style) {
            self.set_status(format!("样式失败: {err}"));
        } else {
            self.mark_dirty();
        }
        cx.notify();
    }

    fn select_block(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.word.selected_block_id = Some(id);
        cx.notify();
    }

    // ---- PPT actions --------------------------------------------------------

    fn add_slide(&mut self, layout: &str, cx: &mut Context<Self>) {
        let index = match self.state.ppt.core.read_document() {
            DocIR::Ppt { slides } => slides.len(),
            _ => 0,
        };
        if let Err(err) = self.state.ppt.add_slide(index, layout) {
            self.set_status(format!("添加幻灯片失败: {err}"));
        } else {
            self.mark_dirty();
            self.set_status(format!("已添加 {layout} 幻灯片"));
        }
        cx.notify();
    }

    fn select_slide(&mut self, slide_id: String, cx: &mut Context<Self>) {
        let first_el = match self.state.ppt.core.read_document() {
            DocIR::Ppt { slides } => slides
                .iter()
                .find(|s| s.id == slide_id)
                .and_then(|s| s.elements.first())
                .map(|e| e.id.clone()),
            _ => None,
        };
        self.state.ppt.select(slide_id, first_el);
        cx.notify();
    }

    fn select_element(
        &mut self,
        slide_id: String,
        el_id: String,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.state.ppt.select(slide_id, Some(el_id));
        self.drag_last = Some(pos);
        // Mirror element text into the composer for inline editing.
        if let Some(text) = self.selected_element_text(cx) {
            self.composer.update(cx, |c, cx| c.set_text(text, cx));
        }
        cx.notify();
    }

    fn selected_element_text(&self, _cx: &App) -> Option<String> {
        let (slide_id, el_id) = (
            self.state.ppt.selected_slide_id.as_ref()?,
            self.state.ppt.selected_element_id.as_ref()?,
        );
        match self.state.ppt.core.read_document() {
            DocIR::Ppt { slides } => slides
                .iter()
                .find(|s| &s.id == slide_id)?
                .elements
                .iter()
                .find(|e| &e.id == el_id)?
                .props
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        }
    }

    fn apply_element_text(&mut self, cx: &mut Context<Self>) {
        let text = self.composer_text(cx);
        if let Err(err) = self.state.ppt.edit_selected_text(&text) {
            self.set_status(format!("编辑元素失败: {err}"));
        } else {
            self.mark_dirty();
            self.set_status("已更新元素文本");
        }
        cx.notify();
    }

    fn on_canvas_move(
        &mut self,
        ev: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(last) = self.drag_last else { return };
        let dx_px: f32 = (ev.position.x - last.x).into();
        let dy_px: f32 = (ev.position.y - last.y).into();
        let dx = dx_px as f64 / PPT_SCALE as f64;
        let dy = dy_px as f64 / PPT_SCALE as f64;
        if dx.abs() < f64::EPSILON && dy.abs() < f64::EPSILON {
            return;
        }
        if self.state.ppt.move_selected(dx, dy).is_ok() {
            self.drag_last = Some(ev.position);
            self.mark_dirty();
            cx.notify();
        }
    }

    fn on_canvas_up(&mut self, _ev: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.drag_last = None;
    }

    // ---- Export -------------------------------------------------------------

    fn export(&mut self, cx: &mut Context<Self>) {
        let (ir, format, default_name) = match self.state.mode {
            DocForgeMode::Word => (
                self.state.word.core.read_document(),
                ExportFormat::Docx,
                "document.docx",
            ),
            DocForgeMode::Ppt => (
                self.state.ppt.core.read_document(),
                ExportFormat::Pptx,
                "presentation.pptx",
            ),
        };
        let picked = rfd::FileDialog::new()
            .set_file_name(default_name)
            .save_file();
        let Some(path) = picked else {
            self.set_status("已取消导出");
            cx.notify();
            return;
        };
        match crate::ExportService::export(&ir, format, &path) {
            Ok(res) => {
                let _ = open::that(&res.path);
                self.set_status(format!(
                    "已导出 {} ({} 字节)",
                    res.path.display(),
                    res.bytes
                ));
            }
            Err(err) => self.set_status(format!("导出失败: {err}")),
        }
        cx.notify();
    }

    fn export_png(&mut self, cx: &mut Context<Self>) {
        let ir = match self.state.mode {
            DocForgeMode::Word => self.state.word.core.read_document(),
            DocForgeMode::Ppt => self.state.ppt.core.read_document(),
        };
        let dir = std::env::temp_dir().join("docforge-preview");
        match crate::ExportService::preview_raster(ir, dir) {
            Ok(res) => {
                let _ = open::that(&res.artifact_path);
                self.set_status(format!(
                    "L2 渲染({}): {}",
                    res.renderer,
                    res.artifact_path.display()
                ));
            }
            Err(err) => self.set_status(format!("渲染失败: {err}")),
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (ir, name) = match self.state.mode {
            DocForgeMode::Word => (self.state.word.core.read_document(), "document.json"),
            DocForgeMode::Ppt => (self.state.ppt.core.read_document(), "presentation.json"),
        };
        let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() else {
            self.set_status("已取消保存");
            cx.notify();
            return;
        };
        match crate::ExportService::export(&ir, ExportFormat::Json, &path) {
            Ok(res) => self.set_status(format!("已保存 {}", res.path.display())),
            Err(err) => self.set_status(format!("保存失败: {err}")),
        }
        cx.notify();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DocForge JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(err) => {
                self.set_status(format!("读取失败: {err}"));
                cx.notify();
                return;
            }
        };
        let ir: DocIR = match serde_json::from_str(&text) {
            Ok(ir) => ir,
            Err(err) => {
                self.set_status(format!("解析失败: {err}"));
                cx.notify();
                return;
            }
        };
        self.workspace_binding = None;
        self.apply_ir(ir, cx);
        self.set_status(format!("已打开 {}", path.display()));
        cx.notify();
    }

    fn switch_mode(&mut self, mode: DocForgeMode, cx: &mut Context<Self>) {
        self.state.mode = mode;
        cx.notify();
    }
}

impl Focusable for DocForgeApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn button(
    label: impl Into<SharedString>,
    active: bool,
    theme: &Theme,
    on_click: impl Fn(&mut DocForgeApp, &mut Window, &mut Context<DocForgeApp>) + 'static,
    cx: &mut Context<DocForgeApp>,
) -> impl IntoElement {
    let c = theme.colors();
    let (bg, fg) = if active {
        (c.accent, rgb(0xffffff))
    } else {
        (c.panel, c.text)
    };
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(rgba(0xffffff20))
        .cursor_pointer()
        .hover(|s| s.bg(c.accent))
        .child(label.into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev: &MouseDownEvent, window, cx| on_click(this, window, cx)),
        )
}

impl Render for DocForgeApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .workspace_binding
            .as_ref()
            .is_some_and(|b| b.preview_only)
        {
            div()
                .track_focus(&self.focus_handle(cx))
                .size_full()
                .bg(rgb(0xf3f4f6))
                .child(self.render_preview_only(cx))
        } else {
            let c = self.theme.colors();
            div()
                .track_focus(&self.focus_handle(cx))
                .flex()
                .flex_col()
                .size_full()
                .bg(c.background)
                .text_color(c.text)
                .font_family("Segoe UI")
                .text_size(px(14.0))
                .child(self.render_topbar(cx))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_main(cx)),
                )
                .child(self.render_composer(cx))
                .child(self.render_statusbar())
        }
    }
}

impl DocForgeApp {
    fn render_topbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.theme.colors();
        let mode = self.state.mode;
        let embedded = self.workspace_binding.is_some();
        let title: SharedString = self
            .workspace_binding
            .as_ref()
            .map(|b| {
                std::path::Path::new(&b.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("DocForge")
                    .to_string()
            })
            .unwrap_or_else(|| "DocForge".to_string())
            .into();
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(c.panel)
            .border_b_1()
            .border_color(rgba(0xffffff20))
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(16.0))
                    .child(title),
            );
        if !embedded {
            bar = bar
                .child(div().w(px(16.0)))
                .child(button(
                    "Word",
                    mode == DocForgeMode::Word,
                    &self.theme,
                    |this, _w, cx| this.switch_mode(DocForgeMode::Word, cx),
                    cx,
                ))
                .child(button(
                    "PPT",
                    mode == DocForgeMode::Ppt,
                    &self.theme,
                    |this, _w, cx| this.switch_mode(DocForgeMode::Ppt, cx),
                    cx,
                ))
                .child(div().flex_1())
                .child(button(
                    "打开",
                    false,
                    &self.theme,
                    |this, _w, cx| this.open(cx),
                    cx,
                ))
                .child(button(
                    "保存",
                    false,
                    &self.theme,
                    |this, _w, cx| this.save(cx),
                    cx,
                ))
                .child(button(
                    "导出 PNG",
                    false,
                    &self.theme,
                    |this, _w, cx| this.export_png(cx),
                    cx,
                ))
                .child(button(
                    "导出",
                    false,
                    &self.theme,
                    |this, _w, cx| this.export(cx),
                    cx,
                ));
        } else {
            bar = bar.child(div().flex_1());
        }
        bar
    }

    fn render_main(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.state.mode {
            DocForgeMode::Word => self.render_word(cx).into_any_element(),
            DocForgeMode::Ppt => self.render_ppt(cx).into_any_element(),
        }
    }

    fn render_word(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.theme.colors();
        let ir = self.state.word.core.read_document();
        let selected = self.state.word.selected_block_id.clone();
        let blocks = match &ir {
            DocIR::Word { blocks } => blocks.clone(),
            _ => Vec::new(),
        };
        div()
            .flex()
            .flex_row()
            .size_full()
            // Editor column
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_word_toolbar(cx))
                    .child(
                        div()
                            .id("word-doc")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_4()
                            .flex_1()
                            .overflow_y_scroll()
                            .children(blocks.iter().map(|b| {
                                let id = b.id.clone();
                                let is_sel = selected.as_deref() == Some(b.id.as_str());
                                let (text, mut el) = (block_plain_text(b), div());
                                let mut bold = false;
                                let mut italic = false;
                                let mut strike = false;
                                let mut underline = false;
                                let mut code = false;
                                for run in &b.runs {
                                    bold |= run.marks.contains(&WordMark::Bold);
                                    italic |= run.marks.contains(&WordMark::Italic);
                                    strike |= run.marks.contains(&WordMark::Strike);
                                    underline |= run.marks.contains(&WordMark::Underline);
                                    code |= run.marks.contains(&WordMark::Code);
                                }
                                let size = match b.block_type {
                                    WordBlockType::Heading => match b.level.unwrap_or(1) {
                                        1 => 28.0,
                                        2 => 24.0,
                                        3 => 20.0,
                                        _ => 18.0,
                                    },
                                    WordBlockType::Paragraph => 15.0,
                                };
                                el = el
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_size(px(size))
                                    .cursor_pointer()
                                    .child(if text.is_empty() {
                                        "(空)".to_string()
                                    } else {
                                        text
                                    });
                                if bold || matches!(b.block_type, WordBlockType::Heading) {
                                    el = el.font_weight(gpui::FontWeight::BOLD);
                                }
                                if italic {
                                    el = el.italic();
                                }
                                if strike {
                                    el = el.line_through();
                                }
                                if underline {
                                    el = el.underline();
                                }
                                if code {
                                    el = el.font_family("Consolas").bg(rgba(0xffffff10));
                                }
                                if is_sel {
                                    el = el.bg(rgba(0x7c3aed40)).border_1().border_color(c.accent);
                                }
                                el.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                                        this.select_block(id.clone(), cx)
                                    }),
                                )
                            })),
                    ),
            )
            // Preview column
            .child(
                div()
                    .w(px(420.0))
                    .border_l_1()
                    .border_color(rgba(0xffffff20))
                    .bg(rgb(0xffffff))
                    .text_color(rgb(0x111111))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .bg(rgb(0xf3f4f6))
                            .text_color(rgb(0x374151))
                            .child("实时预览 (L1)"),
                    )
                    .child(
                        div()
                            .id("word-preview")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_6()
                            .overflow_y_scroll()
                            .children(blocks.iter().map(|b| {
                                let size = match b.block_type {
                                    WordBlockType::Heading => match b.level.unwrap_or(1) {
                                        1 => 26.0,
                                        2 => 22.0,
                                        3 => 18.0,
                                        _ => 16.0,
                                    },
                                    WordBlockType::Paragraph => 14.0,
                                };
                                let mut d = div().text_size(px(size)).child(block_plain_text(b));
                                if matches!(b.block_type, WordBlockType::Heading) {
                                    d = d.font_weight(gpui::FontWeight::BOLD);
                                }
                                d
                            })),
                    ),
            )
    }

    fn render_word_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.theme.colors();
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .p_2()
            .bg(c.panel)
            .border_b_1()
            .border_color(rgba(0xffffff20))
            .child(button(
                "¶ 段落",
                false,
                &self.theme,
                |this, _w, cx| this.add_paragraph(cx),
                cx,
            ))
            .child(button(
                "H1",
                false,
                &self.theme,
                |this, _w, cx| this.add_heading(1, cx),
                cx,
            ))
            .child(button(
                "H2",
                false,
                &self.theme,
                |this, _w, cx| this.add_heading(2, cx),
                cx,
            ))
            .child(button(
                "H3",
                false,
                &self.theme,
                |this, _w, cx| this.add_heading(3, cx),
                cx,
            ))
            .child(div().w(px(8.0)))
            .child(button(
                "B",
                false,
                &self.theme,
                |this, _w, cx| this.apply_mark(WordMark::Bold, cx),
                cx,
            ))
            .child(button(
                "I",
                false,
                &self.theme,
                |this, _w, cx| this.apply_mark(WordMark::Italic, cx),
                cx,
            ))
            .child(button(
                "U",
                false,
                &self.theme,
                |this, _w, cx| this.apply_mark(WordMark::Underline, cx),
                cx,
            ))
            .child(button(
                "S",
                false,
                &self.theme,
                |this, _w, cx| this.apply_mark(WordMark::Strike, cx),
                cx,
            ))
            .child(button(
                "</>",
                false,
                &self.theme,
                |this, _w, cx| this.apply_mark(WordMark::Code, cx),
                cx,
            ))
            .child(div().w(px(8.0)))
            .child(button(
                "替换选中",
                false,
                &self.theme,
                |this, _w, cx| this.replace_selected(cx),
                cx,
            ))
    }

    fn render_ppt(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.theme.colors();
        let ir = self.state.ppt.core.read_document();
        let slides = match &ir {
            DocIR::Ppt { slides } => slides.clone(),
            _ => Vec::new(),
        };
        let selected_slide = self
            .state
            .ppt
            .selected_slide_id
            .clone()
            .or_else(|| slides.first().map(|s| s.id.clone()));
        let selected_el = self.state.ppt.selected_element_id.clone();
        let current = slides
            .iter()
            .find(|s| Some(s.id.clone()) == selected_slide)
            .cloned();

        div()
            .flex()
            .flex_row()
            .size_full()
            // Slide rail
            .child(
                div()
                    .w(px(200.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .bg(c.panel)
                    .border_r_1()
                    .border_color(rgba(0xffffff20))
                    .child(button(
                        "+ 标题页",
                        false,
                        &self.theme,
                        |this, _w, cx| this.add_slide("title", cx),
                        cx,
                    ))
                    .child(button(
                        "+ 标题正文",
                        false,
                        &self.theme,
                        |this, _w, cx| this.add_slide("titleBody", cx),
                        cx,
                    ))
                    .child(button(
                        "+ 双栏",
                        false,
                        &self.theme,
                        |this, _w, cx| this.add_slide("twoContent", cx),
                        cx,
                    ))
                    .child(div().h(px(8.0)))
                    .children(slides.iter().enumerate().map(|(i, s)| {
                        let id = s.id.clone();
                        let is_sel = selected_slide.as_deref() == Some(s.id.as_str());
                        button(
                            format!("幻灯片 {}", i + 1),
                            is_sel,
                            &self.theme,
                            move |this, _w, cx| this.select_slide(id.clone(), cx),
                            cx,
                        )
                    })),
            )
            // Canvas
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x1f2937))
                    .child(self.render_ppt_canvas(current, selected_slide, selected_el, cx)),
            )
    }

    fn render_ppt_canvas(
        &mut self,
        current: Option<moonlit_doccore::Slide>,
        selected_slide: Option<String>,
        selected_el: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = self.theme.colors();
        let width = px(10.0 * PPT_SCALE);
        let height = px(5.625 * PPT_SCALE);
        let mut canvas = div()
            .relative()
            .w(width)
            .h(height)
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(c.accent)
            .on_mouse_move(cx.listener(Self::on_canvas_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_canvas_up));

        if let Some(slide) = current {
            let slide_id = slide.id.clone();
            for el in &slide.elements {
                let el_id = el.id.clone();
                let is_sel = selected_el.as_deref() == Some(el.id.as_str());
                let text = el
                    .props
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let font = el
                    .props
                    .get("fontSize")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(18.0) as f32;
                let fill = el
                    .props
                    .get("fill")
                    .and_then(|v| v.as_str())
                    .map(crate::app::parse_hex);
                let mut node = div()
                    .absolute()
                    .left(px(el.geo.x as f32 * PPT_SCALE))
                    .top(px(el.geo.y as f32 * PPT_SCALE))
                    .w(px(el.geo.w as f32 * PPT_SCALE))
                    .h(px(el.geo.h as f32 * PPT_SCALE))
                    .text_color(rgb(0x111111))
                    .text_size(px(font))
                    .cursor_pointer()
                    .overflow_hidden();
                if let Some(fill) = fill {
                    node = node.bg(fill);
                }
                if !text.is_empty() {
                    node = node.child(text);
                }
                if is_sel {
                    node = node.border_2().border_color(c.accent);
                } else {
                    node = node.border_1().border_color(rgba(0x9ca3af80));
                }
                let sid = slide_id.clone();
                node = node.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        this.select_element(sid.clone(), el_id.clone(), ev.position, cx)
                    }),
                );
                canvas = canvas.child(node);
            }
            // keep selected slide id reflected
            let _ = selected_slide;
        } else {
            canvas = canvas.flex().items_center().justify_center().child(
                div()
                    .text_color(rgb(0x9ca3af))
                    .child("无幻灯片，请在左侧添加"),
            );
        }
        canvas
    }

    fn render_composer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.theme.colors();
        let apply_label = if self.state.mode == DocForgeMode::Ppt {
            "应用到元素"
        } else {
            "替换选中"
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(c.panel)
            .border_t_1()
            .border_color(rgba(0xffffff20))
            .child(
                div()
                    .flex_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0xffffff))
                    .text_color(rgb(0x111111))
                    .child(self.composer.clone()),
            )
            .child(button(
                apply_label,
                false,
                &self.theme,
                move |this, _w, cx| {
                    if this.state.mode == DocForgeMode::Ppt {
                        this.apply_element_text(cx);
                    } else {
                        this.replace_selected(cx);
                    }
                },
                cx,
            ))
    }

    fn render_statusbar(&self) -> impl IntoElement {
        let c = self.theme.colors();
        div()
            .px_4()
            .py_1()
            .bg(rgb(0x0f172a))
            .text_color(c.muted)
            .text_size(px(12.0))
            .child(self.status.clone())
    }

    /// Workspace preview: document body only, no toolbars or composer.
    fn render_preview_only(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.state.mode {
            DocForgeMode::Word => self.render_word_preview_only().into_any_element(),
            DocForgeMode::Ppt => self.render_ppt_preview_only(cx).into_any_element(),
        }
    }

    fn render_word_preview_only(&self) -> impl IntoElement {
        let ir = self.state.word.core.read_document();
        let blocks = match &ir {
            DocIR::Word { blocks } => blocks.clone(),
            _ => Vec::new(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .p_8()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                div()
                    .w(px(720.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_10()
                    .bg(rgb(0xffffff))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0x00000010))
                    .text_color(rgb(0x111111))
                    .children(blocks.iter().map(|b| {
                        let size = match b.block_type {
                            WordBlockType::Heading => match b.level.unwrap_or(1) {
                                1 => 28.0,
                                2 => 22.0,
                                3 => 18.0,
                                _ => 16.0,
                            },
                            WordBlockType::Paragraph => 15.0,
                        };
                        let mut d = div().text_size(px(size)).child(block_plain_text(b));
                        if matches!(b.block_type, WordBlockType::Heading) {
                            d = d.font_weight(gpui::FontWeight::BOLD);
                        }
                        d
                    })),
                    ),
            )
    }

    fn render_ppt_preview_only(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ir = self.state.ppt.core.read_document();
        let slides = match &ir {
            DocIR::Ppt { slides } => slides.clone(),
            _ => Vec::new(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .p_8()
            .children(slides.iter().enumerate().map(|(i, slide)| {
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(0x6b7280))
                                    .child(format!("幻灯片 {}", i + 1)),
                            )
                            .child(self.render_ppt_slide_readonly(slide.clone(), cx))
                    }))
    }

    fn render_ppt_slide_readonly(
        &self,
        slide: moonlit_doccore::Slide,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let width = px(10.0 * PPT_SCALE);
        let height = px(5.625 * PPT_SCALE);
        let mut canvas = div()
            .relative()
            .w(width)
            .h(height)
            .bg(rgb(0xffffff))
            .rounded_md()
            .border_1()
            .border_color(rgba(0x00000012));
        for el in &slide.elements {
            let text = el
                .props
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let font = el
                .props
                .get("fontSize")
                .and_then(|v| v.as_f64())
                .unwrap_or(18.0) as f32;
            let bold = el
                .props
                .get("bold")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let fill = el
                .props
                .get("fill")
                .and_then(|v| v.as_str())
                .map(parse_hex);
            let mut node = div()
                .absolute()
                .left(px(el.geo.x as f32 * PPT_SCALE))
                .top(px(el.geo.y as f32 * PPT_SCALE))
                .w(px(el.geo.w as f32 * PPT_SCALE))
                .h(px(el.geo.h as f32 * PPT_SCALE))
                .text_color(rgb(0x111111))
                .text_size(px(font))
                .overflow_hidden();
            if bold {
                node = node.font_weight(gpui::FontWeight::BOLD);
            }
            if let Some(fill) = fill {
                node = node.bg(fill);
            }
            if !text.is_empty() {
                node = node.child(text);
            }
            canvas = canvas.child(node);
        }
        canvas
    }
}

fn block_plain_text(b: &moonlit_doccore::WordBlock) -> String {
    b.runs.iter().map(|r| r.text.as_str()).collect::<String>()
}

pub(crate) fn parse_hex(s: &str) -> gpui::Rgba {
    moonlit_uikit::hex(s)
}

#[allow(dead_code)]
fn _assert_send(_: &Arc<()>) {}
