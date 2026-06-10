pub mod app;

use moonlit_compile::{compile_to_buffer, export_to_file};
use moonlit_doccore::{
    DocCore, DocCoreOptions, DocIR, DocType, ExportFormat, NewWordBlock, PartialGeo, Result as DocResult,
    StyleInput, WordBlockType,
};
use moonlit_preview::{raster_preview, render_l1, L1Preview, RasterRequest, RasterResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub struct WordEditorState {
    pub core: DocCore,
    pub selected_block_id: Option<String>,
}

impl Default for WordEditorState {
    fn default() -> Self {
        Self {
            core: DocCore::new(DocCoreOptions::new(DocType::Word)),
            selected_block_id: None,
        }
    }
}

impl WordEditorState {
    pub fn insert_paragraph(&mut self, after_id: Option<&str>, text: &str) -> DocResult<String> {
        let id = self.core.insert_block(
            after_id,
            NewWordBlock {
                block_type: WordBlockType::Paragraph,
                level: None,
                style: None,
                text: Some(text.to_string()),
                runs: None,
            },
        )?;
        self.selected_block_id = Some(id.clone());
        Ok(id)
    }

    pub fn insert_heading(&mut self, after_id: Option<&str>, level: u8, text: &str) -> DocResult<String> {
        let id = self.core.insert_block(
            after_id,
            NewWordBlock {
                block_type: WordBlockType::Heading,
                level: Some(level.clamp(1, 6)),
                style: None,
                text: Some(text.to_string()),
                runs: None,
            },
        )?;
        self.selected_block_id = Some(id.clone());
        Ok(id)
    }

    pub fn replace_selected_text(&self, text: &str) -> DocResult<()> {
        if let Some(id) = &self.selected_block_id {
            self.core.replace_text(id, text)
        } else {
            Ok(())
        }
    }

    pub fn apply_toolbar_style(&self, style: StyleInput) -> DocResult<()> {
        if let Some(id) = &self.selected_block_id {
            self.core.apply_style(id, style)
        } else {
            Ok(())
        }
    }

    pub fn preview(&self) -> L1Preview {
        render_l1(&self.core.read_document(), None)
    }
}

pub struct PptEditorState {
    pub core: DocCore,
    pub selected_slide_id: Option<String>,
    pub selected_element_id: Option<String>,
}

impl Default for PptEditorState {
    fn default() -> Self {
        Self {
            core: DocCore::new(DocCoreOptions::new(DocType::Ppt)),
            selected_slide_id: None,
            selected_element_id: None,
        }
    }
}

impl PptEditorState {
    pub fn add_slide(&mut self, index: usize, layout: &str) -> DocResult<String> {
        let id = self.core.add_slide(index, layout)?;
        self.selected_slide_id = Some(id.clone());
        self.selected_element_id = first_element_id(&self.core.read_document(), &id);
        Ok(id)
    }

    pub fn select(&mut self, slide_id: impl Into<String>, element_id: Option<String>) {
        self.selected_slide_id = Some(slide_id.into());
        self.selected_element_id = element_id;
    }

    pub fn edit_selected_text(&self, text: &str) -> DocResult<()> {
        let (Some(slide_id), Some(el_id)) = (&self.selected_slide_id, &self.selected_element_id) else {
            return Ok(());
        };
        self.core.edit_element(
            slide_id,
            el_id,
            Map::from_iter([("text".to_string(), Value::String(text.to_string()))]),
        )
    }

    pub fn move_selected(&self, dx: f64, dy: f64) -> DocResult<()> {
        let (Some(slide_id), Some(el_id)) = (&self.selected_slide_id, &self.selected_element_id) else {
            return Ok(());
        };
        let Some(current) = find_element_geo(&self.core.read_document(), slide_id, el_id) else {
            return Ok(());
        };
        self.core.move_element(
            slide_id,
            el_id,
            PartialGeo {
                x: Some(current.x + dx),
                y: Some(current.y + dy),
                w: None,
                h: None,
                rot: None,
            },
        )
    }

    pub fn preview(&self) -> L1Preview {
        render_l1(&self.core.read_document(), None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: PathBuf,
    pub bytes: usize,
}

pub struct ExportService;

impl ExportService {
    pub fn export(ir: &DocIR, format: ExportFormat, out_path: impl AsRef<Path>) -> anyhow::Result<ExportResult> {
        let bytes = compile_to_buffer(ir, format, None)?.len();
        let path = export_to_file(ir, format, out_path)?;
        Ok(ExportResult { path, bytes })
    }

    pub fn preview_raster(ir: DocIR, out_dir: impl Into<PathBuf>) -> anyhow::Result<RasterResult> {
        Ok(raster_preview(RasterRequest {
            ir,
            slide_id: None,
            out_dir: out_dir.into(),
            width: None,
        })?)
    }
}

pub struct DocForgeState {
    pub word: WordEditorState,
    pub ppt: PptEditorState,
    pub mode: DocForgeMode,
}

impl Default for DocForgeState {
    fn default() -> Self {
        Self {
            word: WordEditorState::default(),
            ppt: PptEditorState::default(),
            mode: DocForgeMode::Word,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocForgeMode {
    Word,
    Ppt,
}

fn first_element_id(ir: &DocIR, slide_id: &str) -> Option<String> {
    let DocIR::Ppt { slides } = ir else {
        return None;
    };
    slides
        .iter()
        .find(|s| s.id == slide_id)
        .and_then(|s| s.elements.first())
        .map(|el| el.id.clone())
}

fn find_element_geo(ir: &DocIR, slide_id: &str, el_id: &str) -> Option<moonlit_doccore::Geo> {
    let DocIR::Ppt { slides } = ir else {
        return None;
    };
    slides
        .iter()
        .find(|s| s.id == slide_id)?
        .elements
        .iter()
        .find(|el| el.id == el_id)
        .map(|el| el.geo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_editor_applies_toolbar_style() {
        let mut editor = WordEditorState::default();
        let id = editor.insert_heading(None, 2, "标题").unwrap();
        assert_eq!(editor.selected_block_id.as_deref(), Some(id.as_str()));
        editor
            .apply_toolbar_style(StyleInput {
                bold: Some(true),
                ..StyleInput::default()
            })
            .unwrap();
        assert!(editor.preview().markup.contains("<strong>"));
    }

    #[test]
    fn ppt_editor_adds_and_edits_slide() {
        let mut editor = PptEditorState::default();
        let slide = editor.add_slide(0, "title").unwrap();
        assert_eq!(editor.selected_slide_id.as_deref(), Some(slide.as_str()));
        editor.edit_selected_text("Native Rust").unwrap();
        assert!(editor.preview().markup.contains("Native Rust"));
        editor.move_selected(0.5, 0.5).unwrap();
    }

    #[test]
    fn export_service_writes_docx() {
        let mut editor = WordEditorState::default();
        editor.insert_paragraph(None, "hello").unwrap();
        let dir = std::env::temp_dir().join(format!("moonlit-docforge-{}", std::process::id()));
        let out = dir.join("demo.docx");
        let result = ExportService::export(&editor.core.read_document(), ExportFormat::Docx, &out).unwrap();
        assert!(result.path.exists());
        assert!(result.bytes > 0);
    }

    #[test]
    fn raster_preview_writes_artifact() {
        let state = DocForgeState::default();
        let dir = std::env::temp_dir().join(format!("moonlit-docforge-preview-{}", std::process::id()));
        let result = ExportService::preview_raster(state.word.core.read_document(), dir).unwrap();
        assert!(result.artifact_path.exists());
    }
}
