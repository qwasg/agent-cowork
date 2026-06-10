//! Shared UI toolkit for the native frontends.
//!
//! The crate is split deliberately:
//! - default build: headless component state, markdown rendering, code buffers,
//!   diff helpers, theme tokens. This keeps Windows builds green.
//! - `gpui-backend`: optional GPUI integration/smoke surface once the platform
//!   backend is available.

use pulldown_cmark::{html, Options, Parser};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub mode: ThemeMode,
    pub background: String,
    pub panel: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub danger: String,
}

impl Theme {
    pub fn moonlit_dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            background: "#0b1020".to_string(),
            panel: "#111827".to_string(),
            text: "#e5e7eb".to_string(),
            muted: "#9ca3af".to_string(),
            accent: "#7c3aed".to_string(),
            danger: "#ef4444".to_string(),
        }
    }

    pub fn moonlit_light() -> Self {
        Self {
            mode: ThemeMode::Light,
            background: "#f8fafc".to_string(),
            panel: "#ffffff".to_string(),
            text: "#111827".to_string(),
            muted: "#6b7280".to_string(),
            accent: "#6d28d9".to_string(),
            danger: "#dc2626".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitPane {
    pub left: f32,
    pub center: f32,
    pub right: f32,
}

impl Default for SplitPane {
    fn default() -> Self {
        Self {
            left: 260.0,
            center: 1.0,
            right: 320.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Toast {
    pub id: u64,
    pub title: String,
    pub body: Option<String>,
    pub kind: ToastKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Default, Clone)]
pub struct ToastStack {
    next_id: u64,
    items: Vec<Toast>,
}

impl ToastStack {
    pub fn push(&mut self, title: impl Into<String>, body: Option<String>, kind: ToastKind) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(Toast {
            id,
            title: title.into(),
            body,
            kind,
        });
        id
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|t| t.id != id);
        self.items.len() != before
    }

    pub fn items(&self) -> &[Toast] {
        &self.items
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: String,
    pub title: String,
    pub section: String,
    pub shortcut: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct CommandPalette {
    commands: Vec<Command>,
    query: String,
}

impl CommandPalette {
    pub fn set_commands(&mut self, commands: Vec<Command>) {
        self.commands = commands;
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
    }

    pub fn matches(&self) -> Vec<&Command> {
        let needle = self.query.to_lowercase();
        self.commands
            .iter()
            .filter(|cmd| {
                needle.is_empty()
                    || cmd.title.to_lowercase().contains(&needle)
                    || cmd.id.to_lowercase().contains(&needle)
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeBuffer {
    pub path: Option<String>,
    pub language: String,
    text: String,
}

impl CodeBuffer {
    pub fn new(path: Option<String>, text: impl Into<String>) -> Self {
        let language = path
            .as_deref()
            .map(language_from_path)
            .unwrap_or("plaintext")
            .to_string();
        Self {
            path,
            language,
            text: text.into(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn replace_all(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    pub fn line_count(&self) -> usize {
        self.text.lines().count().max(1)
    }
}

pub fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "md" => "markdown",
        "json" => "json",
        "toml" => "toml",
        "html" => "html",
        "css" => "css",
        _ => "plaintext",
    }
}

pub fn render_markdown_html(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(markdown, options);
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub tag: DiffTag,
    pub old_lineno: Option<usize>,
    pub new_lineno: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffTag {
    Equal,
    Delete,
    Insert,
}

pub fn compute_line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(old, new);
    let mut old_lineno = 0usize;
    let mut new_lineno = 0usize;
    diff.iter_all_changes()
        .map(|change| match change.tag() {
            ChangeTag::Equal => {
                old_lineno += 1;
                new_lineno += 1;
                DiffLine {
                    tag: DiffTag::Equal,
                    old_lineno: Some(old_lineno),
                    new_lineno: Some(new_lineno),
                    text: change.to_string(),
                }
            }
            ChangeTag::Delete => {
                old_lineno += 1;
                DiffLine {
                    tag: DiffTag::Delete,
                    old_lineno: Some(old_lineno),
                    new_lineno: None,
                    text: change.to_string(),
                }
            }
            ChangeTag::Insert => {
                new_lineno += 1;
                DiffLine {
                    tag: DiffTag::Insert,
                    old_lineno: None,
                    new_lineno: Some(new_lineno),
                    text: change.to_string(),
                }
            }
        })
        .collect()
}

#[cfg(feature = "gpui-backend")]
pub mod gpui_ui;

#[cfg(feature = "gpui-backend")]
pub use gpui_ui::{
    hex, register_text_input_keybindings, toast_color, TextInput, TextInputEvent, ThemeColors,
    Tokens, BORDER, FONT_MONO, FONT_MONO_FALLBACK, FONT_SANS, FONT_SANS_FALLBACK, FONT_SERIF,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders_html() {
        let html = render_markdown_html("# Hello\n\n- [x] done");
        assert!(html.contains("<h1>Hello</h1>"));
        assert!(html.contains("checkbox"));
    }

    #[test]
    fn diff_marks_insertions_and_deletions() {
        let lines = compute_line_diff("a\nb\n", "a\nc\n");
        assert!(lines.iter().any(|l| l.tag == DiffTag::Delete));
        assert!(lines.iter().any(|l| l.tag == DiffTag::Insert));
    }

    #[test]
    fn code_buffer_detects_language() {
        let buf = CodeBuffer::new(Some("main.rs".to_string()), "fn main() {}\n");
        assert_eq!(buf.language, "rust");
        assert_eq!(buf.line_count(), 1);
    }

    #[test]
    fn command_palette_filters() {
        let mut palette = CommandPalette::default();
        palette.set_commands(vec![Command {
            id: "file.open".to_string(),
            title: "Open File".to_string(),
            section: "File".to_string(),
            shortcut: Some("Ctrl+O".to_string()),
        }]);
        palette.set_query("open");
        assert_eq!(palette.matches().len(), 1);
    }
}
