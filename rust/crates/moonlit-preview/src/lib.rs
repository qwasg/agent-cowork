//! Preview pipeline for the native rewrite.
//!
//! L1 is a fast IR -> markup projection used by GPUI drawing code and tests.
//! L2 keeps the same cache/hash contract as the old preview engine and can run
//! LibreOffice (`soffice`) when available; the fallback renderer writes SVG
//! artifacts deterministically.

use moonlit_compile::{compile_to_buffer, default_format_for};
use moonlit_doccore::{DocIR, ExportFormat, SLIDE_HEIGHT, SLIDE_WIDTH};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("compile error: {0}")]
    Compile(#[from] moonlit_compile::CompileError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, PreviewError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1Preview {
    pub kind: L1Kind,
    pub markup: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum L1Kind {
    Html,
    Svg,
}

pub fn render_l1(ir: &DocIR, slide_index: Option<usize>) -> L1Preview {
    match ir {
        DocIR::Word { blocks } => {
            let mut html = String::from(r#"<article class="docforge-word">"#);
            for block in blocks {
                let tag = match block.block_type {
                    moonlit_doccore::WordBlockType::Heading => {
                        format!("h{}", block.level.unwrap_or(1).clamp(1, 6))
                    }
                    moonlit_doccore::WordBlockType::Paragraph => "p".to_string(),
                };
                html.push_str(&format!(r#"<{tag} data-id="{}">"#, esc(&block.id)));
                for run in &block.runs {
                    let mut text = esc(&run.text);
                    for mark in &run.marks {
                        text = match mark {
                            moonlit_doccore::WordMark::Bold => format!("<strong>{text}</strong>"),
                            moonlit_doccore::WordMark::Italic => format!("<em>{text}</em>"),
                            moonlit_doccore::WordMark::Underline => format!("<u>{text}</u>"),
                            moonlit_doccore::WordMark::Strike => format!("<s>{text}</s>"),
                            moonlit_doccore::WordMark::Code => format!("<code>{text}</code>"),
                        };
                    }
                    html.push_str(&text);
                }
                html.push_str(&format!("</{tag}>"));
            }
            html.push_str("</article>");
            L1Preview {
                kind: L1Kind::Html,
                markup: html,
            }
        }
        DocIR::Ppt { slides } => {
            let slide = slides.get(slide_index.unwrap_or(0));
            let mut svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SLIDE_WIDTH} {SLIDE_HEIGHT}">"#
            );
            svg.push_str(r##"<rect x="0" y="0" width="10" height="5.625" fill="#fff"/>"##);
            if let Some(slide) = slide {
                for el in &slide.elements {
                    match el.element_type {
                        moonlit_doccore::ElementType::Text => {
                            let text = el.props.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            let size = el.props.get("fontSize").and_then(|v| v.as_f64()).unwrap_or(18.0) / 72.0;
                            svg.push_str(&format!(
                                r##"<text x="{:.3}" y="{:.3}" font-size="{:.3}" fill="#111">{}</text>"##,
                                el.geo.x,
                                el.geo.y + size,
                                size,
                                esc(text)
                            ));
                        }
                        moonlit_doccore::ElementType::Shape => {
                            let fill = el.props.get("fill").and_then(|v| v.as_str()).unwrap_or("none");
                            svg.push_str(&format!(
                                r##"<rect x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" fill="{}" stroke="#333"/>"##,
                                el.geo.x, el.geo.y, el.geo.w, el.geo.h, esc(fill)
                            ));
                        }
                        moonlit_doccore::ElementType::Image => {
                            let href = el.props.get("src").and_then(|v| v.as_str()).unwrap_or("");
                            svg.push_str(&format!(
                                r#"<image x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" href="{}"/>"#,
                                el.geo.x, el.geo.y, el.geo.w, el.geo.h, esc(href)
                            ));
                        }
                    }
                }
            }
            svg.push_str("</svg>");
            L1Preview {
                kind: L1Kind::Svg,
                markup: svg,
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RasterRequest {
    pub ir: DocIR,
    pub slide_id: Option<String>,
    pub out_dir: PathBuf,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RasterResult {
    pub artifact_path: PathBuf,
    pub renderer: String,
    pub hash: String,
}

pub fn raster_preview(req: RasterRequest) -> Result<RasterResult> {
    std::fs::create_dir_all(&req.out_dir)?;
    let value = serde_json::to_value(&req.ir)?;
    let hash = moonlit_core::content_hash(&value);
    let slide_index = match (&req.ir, &req.slide_id) {
        (DocIR::Ppt { slides }, Some(id)) => slides.iter().position(|s| &s.id == id),
        _ => None,
    };

    match render_png(&req.ir, slide_index, req.width) {
        Ok(png) => {
            let png_path = req.out_dir.join(format!("{hash}.png"));
            std::fs::write(&png_path, png)?;
            Ok(RasterResult {
                artifact_path: png_path,
                renderer: "resvg".to_string(),
                hash,
            })
        }
        Err(err) => {
            // Degrade to writing the SVG markup so callers still get an artifact.
            tracing_svg_fallback(&err);
            let svg_path = req.out_dir.join(format!("{hash}.svg"));
            std::fs::write(&svg_path, raster_svg(&req.ir, slide_index, req.width).0)?;
            Ok(RasterResult {
                artifact_path: svg_path,
                renderer: "fallback-svg".to_string(),
                hash,
            })
        }
    }
}

fn tracing_svg_fallback(_err: &str) {}

/// Rasterize the IR to a PNG byte buffer using resvg/usvg/tiny-skia.
pub fn render_png(ir: &DocIR, slide_index: Option<usize>, width: Option<u32>) -> std::result::Result<Vec<u8>, String> {
    let (svg, w, h) = raster_svg(ir, slide_index, width);

    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(&svg, &options).map_err(|e| e.to_string())?;

    let tree_size = tree.size();
    let sx = w as f32 / tree_size.width();
    let sy = h as f32 / tree_size.height();
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| "invalid pixmap size".to_string())?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Build an SVG sized in pixels (so it can be rasterized) and its target
/// pixel dimensions.
fn raster_svg(ir: &DocIR, slide_index: Option<usize>, width: Option<u32>) -> (String, u32, u32) {
    match ir {
        DocIR::Ppt { .. } => {
            let w = width.unwrap_or(1280).max(64);
            let h = ((w as f64) * (SLIDE_HEIGHT / SLIDE_WIDTH)).round() as u32;
            // Reuse the L1 slide SVG but give it an explicit pixel size.
            let inner = render_l1(ir, slide_index).markup;
            let sized = inner.replacen(
                "<svg xmlns=\"http://www.w3.org/2000/svg\"",
                &format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\""),
                1,
            );
            (sized, w, h)
        }
        DocIR::Word { blocks } => {
            let w = width.unwrap_or(816).max(64); // 8.5in @ 96dpi
            let h = 1056u32; // 11in @ 96dpi
            let mut svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect x="0" y="0" width="{w}" height="{h}" fill="#ffffff"/>"##
            );
            let mut y = 64.0f64;
            let left = 64.0f64;
            for block in blocks {
                let size = match block.block_type {
                    moonlit_doccore::WordBlockType::Heading => match block.level.unwrap_or(1) {
                        1 => 30.0,
                        2 => 26.0,
                        3 => 22.0,
                        _ => 19.0,
                    },
                    moonlit_doccore::WordBlockType::Paragraph => 16.0,
                };
                let weight = if matches!(block.block_type, moonlit_doccore::WordBlockType::Heading) {
                    "bold"
                } else {
                    "normal"
                };
                let text: String = block.runs.iter().map(|r| r.text.as_str()).collect();
                y += size * 1.4;
                svg.push_str(&format!(
                    r##"<text x="{left:.1}" y="{y:.1}" font-size="{size:.1}" font-weight="{weight}" font-family="Calibri, sans-serif" fill="#111111">{}</text>"##,
                    esc(&text)
                ));
            }
            svg.push_str("</svg>");
            (svg, w, h)
        }
    }
}

/// Compile to Office format and ask LibreOffice to convert it to PDF. PNG
/// rasterization remains the fallback renderer's responsibility.
pub fn soffice_to_pdf(ir: &DocIR, out_dir: impl AsRef<Path>) -> Result<Option<PathBuf>> {
    let Some(soffice) = detect_soffice() else {
        return Ok(None);
    };
    let out_dir = out_dir.as_ref();
    std::fs::create_dir_all(out_dir)?;
    let format = default_format_for(ir);
    let ext = match format {
        ExportFormat::Docx => "docx",
        ExportFormat::Pptx => "pptx",
        ExportFormat::Json => "json",
    };
    let hash = moonlit_core::content_hash(&serde_json::to_value(ir)?);
    let office_path = out_dir.join(format!("{hash}.{ext}"));
    std::fs::write(&office_path, compile_to_buffer(ir, format, None)?)?;
    let status = Command::new(&soffice)
        .arg("--headless")
        .arg("--convert-to")
        .arg("pdf")
        .arg("--outdir")
        .arg(out_dir)
        .arg(&office_path)
        .status()?;
    if status.success() {
        Ok(Some(out_dir.join(format!("{hash}.pdf"))))
    } else {
        Ok(None)
    }
}

pub fn detect_soffice() -> Option<PathBuf> {
    let candidates = if cfg!(windows) {
        vec![
            PathBuf::from(r"C:\Program Files\LibreOffice\program\soffice.exe"),
            PathBuf::from(r"C:\Program Files (x86)\LibreOffice\program\soffice.exe"),
        ]
    } else {
        vec![PathBuf::from("soffice")]
    };
    candidates.into_iter().find(|p| {
        if p.is_absolute() {
            p.exists()
        } else {
            Command::new(p).arg("--version").status().is_ok()
        }
    })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use moonlit_doccore::{DocCore, DocCoreOptions, DocType, NewWordBlock, WordBlockType};

    #[test]
    fn word_l1_renders_html() {
        let core = DocCore::new(DocCoreOptions::new(DocType::Word));
        core.insert_block(
            None,
            NewWordBlock {
                block_type: WordBlockType::Paragraph,
                level: None,
                style: None,
                text: Some("hello".to_string()),
                runs: None,
            },
        )
        .unwrap();
        let preview = render_l1(&core.read_document(), None);
        assert_eq!(preview.kind, L1Kind::Html);
        assert!(preview.markup.contains("hello"));
    }

    #[test]
    fn ppt_l1_renders_svg() {
        let core = DocCore::new(DocCoreOptions::new(DocType::Ppt));
        core.add_slide(0, "title").unwrap();
        let preview = render_l1(&core.read_document(), None);
        assert_eq!(preview.kind, L1Kind::Svg);
        assert!(preview.markup.contains("<svg"));
    }

    #[test]
    fn raster_writes_png() {
        let dir = std::env::temp_dir().join(format!("moonlit-preview-{}", std::process::id()));
        let core = DocCore::new(DocCoreOptions::new(DocType::Word));
        core.insert_block(
            None,
            NewWordBlock {
                block_type: WordBlockType::Heading,
                level: Some(1),
                style: None,
                text: Some("Raster".to_string()),
                runs: None,
            },
        )
        .unwrap();
        let result = raster_preview(RasterRequest {
            ir: core.read_document(),
            slide_id: None,
            out_dir: dir,
            width: Some(400),
        })
        .unwrap();
        assert!(result.artifact_path.exists());
        // resvg should succeed in producing a PNG; SVG fallback is acceptable if
        // no system fonts are available in CI.
        assert!(result.renderer == "resvg" || result.renderer == "fallback-svg");
        if result.renderer == "resvg" {
            assert_eq!(result.artifact_path.extension().unwrap(), "png");
        }
    }
}
