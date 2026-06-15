//! Office / PDF document tools.
//!
//! Read text out of `.docx` / `.pptx` / `.pdf`, generate those formats from
//! structured input, and apply structural append/edit to docx/pptx through an
//! IR sidecar (`<file>.ir.json`, the DocForge IR shared with the `rust/`
//! workspace). docx/pptx creation reuses the tested `moonlit-doccore` +
//! `moonlit-compile` engine; PDF generation uses pure-Rust `printpdf`.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::{resolve_in_root, AgentTool, ToolContext};
use agent_protocol::{ApiError, ApiResult};

use moonlit_compile::{compile_to_buffer, export_to_file};
use moonlit_core::DefaultIdFactory;
use moonlit_doccore::{
    from_json, to_json, DocCore, DocCoreOptions, DocIR, DocType, ExportFormat, NewWordBlock,
    WordBlockType,
};

// ---- shared helpers --------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn doc_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::new("DOC_ERROR", e.to_string())
}

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Sidecar IR path: `report.docx` -> `report.docx.ir.json`.
fn sidecar_path(abs: &Path) -> PathBuf {
    let mut s = abs.as_os_str().to_os_string();
    s.push(".ir.json");
    PathBuf::from(s)
}

async fn write_sidecar(abs: &Path, ir: &DocIR) -> ApiResult<()> {
    let json = to_json(ir).map_err(doc_err)?;
    tokio::fs::write(sidecar_path(abs), json)
        .await
        .map_err(|e| ApiError::filesystem(e.to_string()))
}

/// Extract text from an OOXML part: collect every `<*:t>` body, inserting a
/// newline at each paragraph (`<*:p>`) boundary. Works for both `w:t`/`w:p`
/// (Word) and `a:t`/`a:p` (PowerPoint) because we match on local names.
fn extract_office_text(xml: &[u8]) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_text = true;
                }
            }
            Ok(Event::End(e)) => {
                let ln = e.local_name();
                if ln.as_ref() == b"t" {
                    in_text = false;
                } else if ln.as_ref() == b"p" {
                    out.push('\n');
                }
            }
            Ok(Event::Text(t)) => {
                if in_text {
                    out.push_str(&t.unescape().unwrap_or_default());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn read_docx_text(bytes: Vec<u8>) -> ApiResult<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(doc_err)?;
    let mut file = zip
        .by_name("word/document.xml")
        .map_err(|_| ApiError::new("DOC_ERROR", "missing word/document.xml"))?;
    let mut xml = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut xml).map_err(doc_err)?;
    Ok(extract_office_text(&xml).trim().to_string())
}

fn read_pptx_text(bytes: Vec<u8>) -> ApiResult<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(doc_err)?;
    let mut slide_names: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(doc_err)?;
        let n = f.name().to_string();
        if n.starts_with("ppt/slides/slide")
            && n.ends_with(".xml")
            && !n.contains("slideLayout")
            && !n.contains("_rels")
        {
            slide_names.push(n);
        }
    }
    // Sort by the numeric suffix so slide order is preserved.
    slide_names.sort_by_key(|n| {
        n.trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(0)
    });
    let mut out = String::new();
    for (idx, name) in slide_names.iter().enumerate() {
        let mut file = zip.by_name(name).map_err(doc_err)?;
        let mut xml = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut xml).map_err(doc_err)?;
        let text = extract_office_text(&xml);
        out.push_str(&format!("--- Slide {} ---\n", idx + 1));
        out.push_str(text.trim());
        out.push_str("\n\n");
    }
    Ok(out.trim().to_string())
}

// ---- read_document ---------------------------------------------------------

pub struct ReadDocument;

#[async_trait]
impl AgentTool for ReadDocument {
    fn name(&self) -> &str {
        "read_document"
    }
    fn read_only(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "读取并抽取 Word(.docx) / PPT(.pptx) / PDF(.pdf) 文档的纯文本内容（PPT 按幻灯片分段）。\
         读取这三类二进制文档必须用本工具，不要用 read_file（会得到乱码）。.md/.txt 也可读。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": {"type": "string", "description": "相对工作区根目录的文档路径"} },
            "required": ["path"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        let ext = ext_of(&path);
        match ext.as_str() {
            "docx" => {
                let bytes = tokio::fs::read(&abs)
                    .await
                    .map_err(|_| ApiError::path_not_found(&path))?;
                tokio::task::spawn_blocking(move || read_docx_text(bytes))
                    .await
                    .map_err(doc_err)?
            }
            "pptx" => {
                let bytes = tokio::fs::read(&abs)
                    .await
                    .map_err(|_| ApiError::path_not_found(&path))?;
                tokio::task::spawn_blocking(move || read_pptx_text(bytes))
                    .await
                    .map_err(doc_err)?
            }
            "pdf" => {
                let abs2 = abs.clone();
                tokio::task::spawn_blocking(move || {
                    pdf_extract::extract_text(&abs2).map_err(doc_err)
                })
                .await
                .map_err(doc_err)?
            }
            _ => tokio::fs::read_to_string(&abs)
                .await
                .map(|s| s.chars().take(40_000).collect())
                .map_err(|_| ApiError::path_not_found(&path)),
        }
    }
}

// ---- word build helpers ----------------------------------------------------

fn word_block_from_json(b: &Value) -> NewWordBlock {
    let typ = b.get("type").and_then(|v| v.as_str()).unwrap_or("paragraph");
    let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let is_heading = typ.eq_ignore_ascii_case("heading");
    let level = b
        .get("level")
        .and_then(|v| v.as_u64())
        .map(|v| v.clamp(1, 6) as u8);
    NewWordBlock {
        block_type: if is_heading {
            WordBlockType::Heading
        } else {
            WordBlockType::Paragraph
        },
        level: if is_heading {
            Some(level.unwrap_or(1))
        } else {
            None
        },
        style: None,
        text: Some(text.to_string()),
        runs: None,
    }
}

fn build_word_ir(blocks: &[Value]) -> ApiResult<DocIR> {
    let core = DocCore::new(DocCoreOptions::new(DocType::Word));
    let mut after: Option<String> = None;
    for b in blocks {
        let id = core
            .insert_block(after.as_deref(), word_block_from_json(b))
            .map_err(doc_err)?;
        after = Some(id);
    }
    Ok(core.read_document())
}

/// Set a slide's title / body text on the seeded layout elements (matched by
/// the `role` prop the engine stores). Bullets go to the first body-like slot.
fn apply_slide_content(
    core: &DocCore,
    slide_id: &str,
    title: Option<&str>,
    bullets: &[String],
) -> ApiResult<()> {
    let doc = core.read_document();
    let DocIR::Ppt { slides } = doc else {
        return Ok(());
    };
    let Some(slide) = slides.iter().find(|s| s.id == slide_id) else {
        return Ok(());
    };
    let mut body_set = false;
    for el in &slide.elements {
        let role = el
            .props
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut m: Map<String, Value> = Map::new();
        match role {
            "title" => {
                if let Some(t) = title {
                    m.insert("text".into(), Value::String(t.to_string()));
                }
            }
            "body" | "subtitle" | "left" | "right" => {
                if !body_set && !bullets.is_empty() {
                    m.insert("text".into(), Value::String(bullets.join("\n")));
                    body_set = true;
                }
            }
            _ => {}
        }
        if !m.is_empty() {
            core.edit_element(slide_id, &el.id, m).map_err(doc_err)?;
        }
    }
    Ok(())
}

fn slide_bullets(s: &Value) -> Vec<String> {
    s.get("bullets")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn build_ppt_ir(slides: &[Value]) -> ApiResult<DocIR> {
    let core = DocCore::new(DocCoreOptions::new(DocType::Ppt));
    for (i, s) in slides.iter().enumerate() {
        let layout = s.get("layout").and_then(|v| v.as_str()).unwrap_or("titleBody");
        let slide_id = core.add_slide(i, layout).map_err(doc_err)?;
        let title = s.get("title").and_then(|v| v.as_str());
        apply_slide_content(&core, &slide_id, title, &slide_bullets(s))?;
    }
    Ok(core.read_document())
}

async fn export_with_sidecar(
    abs: &Path,
    ir: &DocIR,
    format: ExportFormat,
) -> ApiResult<()> {
    export_to_file(ir, format, abs).map_err(doc_err)?;
    write_sidecar(abs, ir).await
}

// ---- create_word_document --------------------------------------------------

pub struct CreateWordDocument;

#[async_trait]
impl AgentTool for CreateWordDocument {
    fn name(&self) -> &str {
        "create_word_document"
    }
    fn description(&self) -> &str {
        "新建一个 Word(.docx) 文档。用结构化 blocks 描述内容：每个 block 形如 \
         {\"type\":\"heading|paragraph\",\"level\":1-6(仅 heading),\"text\":\"...\"}。\
         会同时生成 .docx 与隐藏的 .ir.json（供 edit_word_document 后续追加/编辑）。\
         生成的 .docx 用 MS Word 可正常打开。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的 .docx 路径"},
                "blocks": {
                    "type": "array",
                    "description": "文档块列表，按顺序排版",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": {"type": "string", "enum": ["heading", "paragraph"]},
                            "level": {"type": "integer", "minimum": 1, "maximum": 6},
                            "text": {"type": "string"}
                        },
                        "required": ["text"]
                    }
                }
            },
            "required": ["path", "blocks"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        if ext_of(&path) != "docx" {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "path 必须以 .docx 结尾"));
        }
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        let empty = Vec::new();
        let blocks = args.get("blocks").and_then(|v| v.as_array()).unwrap_or(&empty);
        let ir = build_word_ir(blocks)?;
        export_with_sidecar(&abs, &ir, ExportFormat::Docx).await?;
        Ok(format!("created Word document {path} ({} block(s))", blocks.len()))
    }
}

// ---- create_presentation ---------------------------------------------------

pub struct CreatePresentation;

#[async_trait]
impl AgentTool for CreatePresentation {
    fn name(&self) -> &str {
        "create_presentation"
    }
    fn description(&self) -> &str {
        "新建一个 PPT(.pptx) 演示文稿。用 slides 描述每页：{\"layout\":\"title|titleBody|twoContent\",\
         \"title\":\"标题\",\"bullets\":[\"要点1\",\"要点2\"]}。layout 省略时为 titleBody。\
         会同时生成 .pptx 与隐藏的 .ir.json（供 edit_presentation 后续加页/编辑）。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的 .pptx 路径"},
                "slides": {
                    "type": "array",
                    "description": "幻灯片列表，按顺序",
                    "items": {
                        "type": "object",
                        "properties": {
                            "layout": {"type": "string", "enum": ["title", "titleBody", "twoContent"]},
                            "title": {"type": "string"},
                            "bullets": {"type": "array", "items": {"type": "string"}}
                        }
                    }
                }
            },
            "required": ["path", "slides"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        if ext_of(&path) != "pptx" {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "path 必须以 .pptx 结尾"));
        }
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        let empty = Vec::new();
        let slides = args.get("slides").and_then(|v| v.as_array()).unwrap_or(&empty);
        let ir = build_ppt_ir(slides)?;
        export_with_sidecar(&abs, &ir, ExportFormat::Pptx).await?;
        Ok(format!("created presentation {path} ({} slide(s))", slides.len()))
    }
}

// ---- create_pdf ------------------------------------------------------------

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + width).min(chars.len());
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    out
}

fn try_load_cjk_font(
    doc: &printpdf::PdfDocumentReference,
) -> Option<printpdf::IndirectFontRef> {
    const CANDIDATES: &[&str] = &[
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simfang.ttf",
        "C:/Windows/Fonts/simkai.ttf",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/msyh.ttc",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(font) = doc.add_external_font(Cursor::new(bytes)) {
                return Some(font);
            }
        }
    }
    None
}

fn generate_pdf(title: &str, lines: &[String]) -> ApiResult<Vec<u8>> {
    use printpdf::{BuiltinFont, Mm, PdfDocument};

    let (doc, page1, layer1) = PdfDocument::new(
        if title.is_empty() { "Document" } else { title },
        Mm(210.0),
        Mm(297.0),
        "Layer 1",
    );
    // Prefer an embeddable CJK font (Windows) so Chinese renders; otherwise fall
    // back to a builtin Latin font (CJK will be missing but the PDF is valid).
    let font = match try_load_cjk_font(&doc) {
        Some(f) => f,
        None => doc.add_builtin_font(BuiltinFont::Helvetica).map_err(doc_err)?,
    };

    let top = 280.0_f32;
    let left = 20.0_f32;
    let bottom = 20.0_f32;
    let line_h = 7.0_f32;
    let mut layer = doc.get_page(page1).get_layer(layer1);
    let mut y = top;
    for line in lines {
        for chunk in wrap_line(line, 90) {
            if y < bottom {
                let (p, l) = doc.add_page(Mm(210.0), Mm(297.0), "Layer");
                layer = doc.get_page(p).get_layer(l);
                y = top;
            }
            layer.use_text(chunk, 12.0, Mm(left), Mm(y), &font);
            y -= line_h;
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = std::io::BufWriter::new(&mut buf);
        doc.save(&mut writer).map_err(doc_err)?;
        writer.flush().map_err(doc_err)?;
    }
    Ok(buf)
}

pub struct CreatePdf;

#[async_trait]
impl AgentTool for CreatePdf {
    fn name(&self) -> &str {
        "create_pdf"
    }
    fn description(&self) -> &str {
        "新建一个 PDF(.pdf) 文档。提供 title（可选）与正文：用 text（多行字符串，按 \\n 分行）或 \
         blocks（[{\"text\":\"...\"}]）。纯文本排版，长行自动换行。注意：PDF 仅支持从头生成、\
         不支持原位结构化编辑；中文依赖系统字体（Windows 自动嵌入）。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的 .pdf 路径"},
                "title": {"type": "string", "description": "文档标题（可选，作为首行）"},
                "text": {"type": "string", "description": "正文（多行，用 \\n 分隔）"},
                "blocks": {
                    "type": "array",
                    "description": "正文块（与 text 二选一）",
                    "items": {"type": "object", "properties": {"text": {"type": "string"}}}
                }
            },
            "required": ["path"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        if ext_of(&path) != "pdf" {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "path 必须以 .pdf 结尾"));
        }
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        let title = arg_str(&args, "title");

        let mut lines: Vec<String> = Vec::new();
        if !title.is_empty() {
            lines.push(title.clone());
            lines.push(String::new());
        }
        if let Some(blocks) = args.get("blocks").and_then(|v| v.as_array()) {
            for b in blocks {
                let t = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                for l in t.split('\n') {
                    lines.push(l.to_string());
                }
            }
        }
        let text = arg_str(&args, "text");
        if !text.is_empty() {
            for l in text.split('\n') {
                lines.push(l.to_string());
            }
        }
        if lines.is_empty() {
            return Err(ApiError::new(
                "TOOL_INVALID_ARGS",
                "需要 title / text / blocks 至少其一",
            ));
        }

        let title_owned = title.clone();
        let bytes =
            tokio::task::spawn_blocking(move || generate_pdf(&title_owned, &lines))
                .await
                .map_err(doc_err)??;
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::filesystem(e.to_string()))?;
        }
        tokio::fs::write(&abs, &bytes)
            .await
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
        Ok(format!("created PDF {path} ({} bytes)", bytes.len()))
    }
}

// ---- edit helpers ----------------------------------------------------------

/// Read the IR sidecar JSON (async). The actual `DocCore` is built later inside
/// a blocking task because `yrs::Doc` is not `Send` and must never be held
/// across an `.await`.
async fn read_sidecar_json(abs: &Path, path: &str) -> ApiResult<String> {
    let sidecar = sidecar_path(abs);
    tokio::fs::read_to_string(&sidecar).await.map_err(|_| {
        ApiError::new(
            "DOC_NO_SIDECAR",
            format!(
                "{path} 没有可编辑的 IR sidecar（{}）。结构化编辑仅支持本工具创建的文档；\
                 外部文档请用 read_document 读取后用 create_* 重新生成。",
                sidecar.display()
            ),
        )
    })
}

fn core_from_json(json: &str) -> ApiResult<DocCore> {
    let ir = from_json(json).map_err(doc_err)?;
    Ok(DocCore::from_ir(ir, Arc::new(DefaultIdFactory::new()), None))
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// Write `bytes` atomically via a same-directory temp file + rename.
fn write_bytes_atomic_sync(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    let tmp = tmp_path(path);
    std::fs::write(&tmp, bytes).map_err(|e| ApiError::filesystem(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| ApiError::filesystem(e.to_string()))?;
    Ok(())
}

async fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    let tmp = tmp_path(path);
    tokio::fs::write(&tmp, bytes)
        .await
        .map_err(|e| ApiError::filesystem(e.to_string()))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| ApiError::filesystem(e.to_string()))?;
    Ok(())
}

/// Persist sidecar (source of truth for the IDE) before the compiled OOXML file.
async fn write_file_and_sidecar(abs: &Path, bytes: &[u8], sidecar_json: &str) -> ApiResult<()> {
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::filesystem(e.to_string()))?;
    }
    let side = sidecar_path(abs);
    write_bytes_atomic(&side, sidecar_json.as_bytes()).await?;
    write_bytes_atomic(abs, bytes).await
}

fn last_word_block_id(core: &DocCore) -> Option<String> {
    match core.read_document() {
        DocIR::Word { blocks } => blocks.last().map(|b| b.id.clone()),
        _ => None,
    }
}

// ---- edit_word_document ----------------------------------------------------

pub struct EditWordDocument;

#[async_trait]
impl AgentTool for EditWordDocument {
    fn name(&self) -> &str {
        "edit_word_document"
    }
    fn description(&self) -> &str {
        "对本工具创建的 Word(.docx) 文档做结构化修改（基于 .ir.json sidecar）。ops 支持：\
         {\"op\":\"append_paragraph\",\"text\":\"...\"}、{\"op\":\"append_heading\",\"level\":1-6,\"text\":\"...\"}、\
         {\"op\":\"replace_text\",\"id\":\"块id\",\"text\":\"...\"}。块 id 可先用 read_document 配合理解结构。\
         修改后会重写 .docx 与 sidecar。无 sidecar（外部文档）会报错。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的 .docx 路径"},
                "ops": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {"type": "string", "enum": ["append_paragraph", "append_heading", "replace_text"]},
                            "level": {"type": "integer", "minimum": 1, "maximum": 6},
                            "id": {"type": "string"},
                            "text": {"type": "string"}
                        },
                        "required": ["op"]
                    }
                }
            },
            "required": ["path", "ops"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        if ext_of(&path) != "docx" {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "path 必须以 .docx 结尾"));
        }
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        let json = read_sidecar_json(&abs, &path).await?;
        let ops: Vec<Value> = args
            .get("ops")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // All `DocCore` (yrs, non-Send) work stays inside the blocking task.
        let (bytes, sidecar_json, applied) =
            tokio::task::spawn_blocking(move || -> ApiResult<(Vec<u8>, String, usize)> {
                let core = core_from_json(&json)?;
                if !matches!(core.read_document(), DocIR::Word { .. }) {
                    return Err(ApiError::new("TOOL_INVALID_ARGS", "目标不是 Word 文档"));
                }
                let mut applied = 0usize;
                for op in &ops {
                    let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
                    let text = op.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    match kind {
                        "append_paragraph" => {
                            let after = last_word_block_id(&core);
                            core.insert_block(
                                after.as_deref(),
                                NewWordBlock {
                                    block_type: WordBlockType::Paragraph,
                                    level: None,
                                    style: None,
                                    text: Some(text.to_string()),
                                    runs: None,
                                },
                            )
                            .map_err(doc_err)?;
                            applied += 1;
                        }
                        "append_heading" => {
                            let level = op
                                .get("level")
                                .and_then(|v| v.as_u64())
                                .map(|v| v.clamp(1, 6) as u8)
                                .unwrap_or(1);
                            let after = last_word_block_id(&core);
                            core.insert_block(
                                after.as_deref(),
                                NewWordBlock {
                                    block_type: WordBlockType::Heading,
                                    level: Some(level),
                                    style: None,
                                    text: Some(text.to_string()),
                                    runs: None,
                                },
                            )
                            .map_err(doc_err)?;
                            applied += 1;
                        }
                        "replace_text" => {
                            let id = op.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            if id.is_empty() {
                                return Err(ApiError::new(
                                    "TOOL_INVALID_ARGS",
                                    "replace_text 需要 id",
                                ));
                            }
                            core.replace_text(id, text).map_err(doc_err)?;
                            applied += 1;
                        }
                        other => {
                            return Err(ApiError::new(
                                "TOOL_INVALID_ARGS",
                                format!("不支持的 op: {other}"),
                            ));
                        }
                    }
                }
                let ir = core.read_document();
                let bytes =
                    compile_to_buffer(&ir, ExportFormat::Docx, None).map_err(doc_err)?;
                let sidecar_json = to_json(&ir).map_err(doc_err)?;
                Ok((bytes, sidecar_json, applied))
            })
            .await
            .map_err(doc_err)??;

        write_file_and_sidecar(&abs, &bytes, &sidecar_json).await?;
        Ok(format!("edited Word document {path} ({applied} op(s) applied)"))
    }
}

// ---- edit_presentation -----------------------------------------------------

pub struct EditPresentation;

#[async_trait]
impl AgentTool for EditPresentation {
    fn name(&self) -> &str {
        "edit_presentation"
    }
    fn description(&self) -> &str {
        "对本工具创建的 PPT(.pptx) 做结构化修改（基于 .ir.json sidecar）。ops 支持：\
         {\"op\":\"add_slide\",\"layout\":\"titleBody\",\"title\":\"...\",\"bullets\":[...]}、\
         {\"op\":\"edit_element\",\"slideId\":\"...\",\"elementId\":\"...\",\"text\":\"...\"}。\
         可先用 read_document 了解内容。修改后会重写 .pptx 与 sidecar。无 sidecar 会报错。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "相对工作区根目录的 .pptx 路径"},
                "ops": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {"type": "string", "enum": ["add_slide", "edit_element"]},
                            "layout": {"type": "string"},
                            "title": {"type": "string"},
                            "bullets": {"type": "array", "items": {"type": "string"}},
                            "slideId": {"type": "string"},
                            "elementId": {"type": "string"},
                            "text": {"type": "string"}
                        },
                        "required": ["op"]
                    }
                }
            },
            "required": ["path", "ops"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let path = arg_str(&args, "path");
        if ext_of(&path) != "pptx" {
            return Err(ApiError::new("TOOL_INVALID_ARGS", "path 必须以 .pptx 结尾"));
        }
        let abs = resolve_in_root(&ctx.workspace_root, &path)?;
        let json = read_sidecar_json(&abs, &path).await?;
        let ops: Vec<Value> = args
            .get("ops")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let (bytes, sidecar_json, applied) =
            tokio::task::spawn_blocking(move || -> ApiResult<(Vec<u8>, String, usize)> {
                let core = core_from_json(&json)?;
                if !matches!(core.read_document(), DocIR::Ppt { .. }) {
                    return Err(ApiError::new("TOOL_INVALID_ARGS", "目标不是 PPT 文档"));
                }
                let mut applied = 0usize;
                for op in &ops {
                    let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
                    match kind {
                        "add_slide" => {
                            let index = match core.read_document() {
                                DocIR::Ppt { slides } => slides.len(),
                                _ => 0,
                            };
                            let layout =
                                op.get("layout").and_then(|v| v.as_str()).unwrap_or("titleBody");
                            let slide_id = core.add_slide(index, layout).map_err(doc_err)?;
                            let title = op.get("title").and_then(|v| v.as_str());
                            apply_slide_content(&core, &slide_id, title, &slide_bullets(op))?;
                            applied += 1;
                        }
                        "edit_element" => {
                            let slide_id =
                                op.get("slideId").and_then(|v| v.as_str()).unwrap_or("");
                            let el_id =
                                op.get("elementId").and_then(|v| v.as_str()).unwrap_or("");
                            let text = op.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if slide_id.is_empty() || el_id.is_empty() {
                                return Err(ApiError::new(
                                    "TOOL_INVALID_ARGS",
                                    "edit_element 需要 slideId 与 elementId",
                                ));
                            }
                            let mut m: Map<String, Value> = Map::new();
                            m.insert("text".into(), Value::String(text.to_string()));
                            core.edit_element(slide_id, el_id, m).map_err(doc_err)?;
                            applied += 1;
                        }
                        other => {
                            return Err(ApiError::new(
                                "TOOL_INVALID_ARGS",
                                format!("不支持的 op: {other}"),
                            ));
                        }
                    }
                }
                let ir = core.read_document();
                let bytes =
                    compile_to_buffer(&ir, ExportFormat::Pptx, None).map_err(doc_err)?;
                let sidecar_json = to_json(&ir).map_err(doc_err)?;
                Ok((bytes, sidecar_json, applied))
            })
            .await
            .map_err(doc_err)??;

        write_file_and_sidecar(&abs, &bytes, &sidecar_json).await?;
        Ok(format!("edited presentation {path} ({applied} op(s) applied)"))
    }
}

// ---- workspace IDE integration -----------------------------------------------

fn write_file_and_sidecar_sync(abs: &Path, bytes: &[u8], sidecar_json: &str) -> ApiResult<()> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ApiError::filesystem(e.to_string()))?;
    }
    let side = sidecar_path(abs);
    write_bytes_atomic_sync(&side, sidecar_json.as_bytes())?;
    write_bytes_atomic_sync(abs, bytes)?;
    Ok(())
}

/// Load a workspace document for the native IDE: IR sidecar for docx/pptx, text for pdf.
pub fn read_workspace_document(root: &Path, path: &str) -> ApiResult<Value> {
    let root_buf = root.to_path_buf();
    let abs = resolve_in_root(&root_buf, path)?;
    match ext_of(path).as_str() {
        "docx" | "pptx" => {
            let side = sidecar_path(&abs);
            if !side.is_file() {
                return Err(ApiError::new(
                    "DOC_NO_SIDECAR",
                    "此文档无 IR sidecar，无法可视化编辑。请用 Agent 的 create_* 工具生成，或先用 read_document 查看文本。",
                ));
            }
            let json = std::fs::read_to_string(&side)
                .map_err(|_| ApiError::path_not_found(path))?;
            let ir: DocIR = from_json(&json).map_err(doc_err)?;
            Ok(json!({
                "path": path,
                "kind": ext_of(path),
                "editable": true,
                "ir": serde_json::to_value(&ir).map_err(doc_err)?,
            }))
        }
        "pdf" => {
            let text = pdf_extract::extract_text(&abs).map_err(doc_err)?;
            Ok(json!({
                "path": path,
                "kind": "pdf",
                "editable": false,
                "text": text,
            }))
        }
        _ => Err(ApiError::new(
            "UNSUPPORTED_DOCUMENT",
            "仅支持 .docx / .pptx / .pdf",
        )),
    }
}

/// Persist DocForge IR back to sidecar and recompile the office file.
pub fn write_workspace_document(root: &Path, path: &str, ir: Value) -> ApiResult<Value> {
    let ir: DocIR =
        serde_json::from_value(ir).map_err(|e| ApiError::new("INVALID_IR", e.to_string()))?;
    let root_buf = root.to_path_buf();
    let abs = resolve_in_root(&root_buf, path)?;
    let (expected, format) = match ext_of(path).as_str() {
        "docx" => (DocType::Word, ExportFormat::Docx),
        "pptx" => (DocType::Ppt, ExportFormat::Pptx),
        _ => {
            return Err(ApiError::new(
                "UNSUPPORTED_DOCUMENT",
                "仅 .docx / .pptx 可保存结构化编辑",
            ));
        }
    };
    if ir.doc_type() != expected {
        return Err(ApiError::new(
            "DOC_TYPE_MISMATCH",
            "IR 类型与文件扩展名不匹配",
        ));
    }
    let bytes = compile_to_buffer(&ir, format, None).map_err(doc_err)?;
    let sidecar_json = to_json(&ir).map_err(doc_err)?;
    write_file_and_sidecar_sync(&abs, &bytes, &sidecar_json)?;
    Ok(json!({ "ok": true, "path": path, "bytes": bytes.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_create_round_trips_through_read() {
        let blocks = vec![
            json!({"type": "heading", "level": 1, "text": "报告标题"}),
            json!({"type": "paragraph", "text": "正文内容 ABC"}),
        ];
        let ir = build_word_ir(&blocks).unwrap();
        let bytes = compile_to_buffer(&ir, ExportFormat::Docx, None).unwrap();
        let text = read_docx_text(bytes).unwrap();
        assert!(text.contains("报告标题"), "got: {text}");
        assert!(text.contains("正文内容 ABC"), "got: {text}");
    }

    #[test]
    fn presentation_create_round_trips_through_read() {
        let slides = vec![json!({
            "layout": "titleBody",
            "title": "封面标题",
            "bullets": ["要点甲", "要点乙"]
        })];
        let ir = build_ppt_ir(&slides).unwrap();
        let bytes = compile_to_buffer(&ir, ExportFormat::Pptx, None).unwrap();
        let text = read_pptx_text(bytes).unwrap();
        assert!(text.contains("封面标题"), "got: {text}");
        assert!(text.contains("要点甲"), "got: {text}");
    }

    #[test]
    fn pdf_generation_emits_valid_header() {
        let bytes = generate_pdf("标题X", &["hello world".to_string()]).unwrap();
        assert!(bytes.starts_with(b"%PDF"), "missing PDF magic");
        assert!(bytes.len() > 200);
    }

    #[test]
    fn word_edit_appends_paragraph_via_sidecar() {
        let blocks = vec![json!({"type": "paragraph", "text": "first"})];
        let ir = build_word_ir(&blocks).unwrap();
        let sidecar = to_json(&ir).unwrap();

        let core = core_from_json(&sidecar).unwrap();
        let after = last_word_block_id(&core);
        core.insert_block(
            after.as_deref(),
            NewWordBlock {
                block_type: WordBlockType::Paragraph,
                level: None,
                style: None,
                text: Some("second".to_string()),
                runs: None,
            },
        )
        .unwrap();

        let ir2 = core.read_document();
        match &ir2 {
            DocIR::Word { blocks } => assert_eq!(blocks.len(), 2),
            _ => panic!("expected word doc"),
        }
        let bytes = compile_to_buffer(&ir2, ExportFormat::Docx, None).unwrap();
        let text = read_docx_text(bytes).unwrap();
        assert!(text.contains("first") && text.contains("second"), "got: {text}");
    }

    /// End-to-end simulation: drive every document tool through `ToolRegistry`
    /// the same way the agent runtime does (create → read → edit → guardrails).
    #[tokio::test]
    async fn full_document_tool_flow_via_registry() {
        use std::sync::Arc;

        use crate::shell::ShellManager;
        use crate::{OutputLimits, ToolContext, ToolRegistry, WebConfig};

        fn make_ctx(root: PathBuf) -> ToolContext {
            let store = Arc::new(agent_store::Store::open(root.join("t.redb")).unwrap());
            let crypto = agent_store::CryptoStore::open(root.join("k.key"));
            let cfg = agent_config::Config::load();
            ToolContext {
                session_id: "sim-session".into(),
                run_id: "sim-run".into(),
                workspace_root: root.clone(),
                web: WebConfig {
                    fetch_max_chars: 1000,
                    allow_private: false,
                },
                search: crate::SearchConfigService::new(store, crypto, &cfg),
                skill_dirs: vec![],
                tool_output_dir: root.join("tool-outputs"),
                shell: ShellManager::new(root.join("shell-outputs")),
            }
        }

        let root = std::env::temp_dir().join(format!(
            "doc_tool_flow_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = make_ctx(root.clone());
        let reg = ToolRegistry::build_with_limits(false, OutputLimits::default());

        // ---- registry surface ------------------------------------------------
        for name in [
            "read_document",
            "create_word_document",
            "create_presentation",
            "create_pdf",
            "edit_word_document",
            "edit_presentation",
        ] {
            assert!(
                reg.names().contains(&name.to_string()),
                "missing tool: {name}"
            );
        }
        assert!(reg.is_read_only("read_document"));
        assert!(!reg.is_read_only("create_word_document"));

        // ---- Word: create → read → edit → read -------------------------------
        eprintln!("[sim] create_word_document …");
        let w = reg
            .run(
                "create_word_document",
                json!({
                    "path": "reports/demo.docx",
                    "blocks": [
                        {"type": "heading", "level": 1, "text": "季度报告"},
                        {"type": "paragraph", "text": "第一段正文"}
                    ]
                }),
                &ctx,
            )
            .await
            .expect("create_word_document");
        assert!(w.content.contains("created Word"));
        assert!(root.join("reports/demo.docx").is_file());
        assert!(root.join("reports/demo.docx.ir.json").is_file());

        eprintln!("[sim] read_document (docx) …");
        let read1 = reg
            .run(
                "read_document",
                json!({"path": "reports/demo.docx"}),
                &ctx,
            )
            .await
            .expect("read_document docx");
        assert!(
            read1.content.contains("季度报告") && read1.content.contains("第一段正文"),
            "docx text: {}",
            read1.content
        );

        eprintln!("[sim] edit_word_document …");
        let edited = reg
            .run(
                "edit_word_document",
                json!({
                    "path": "reports/demo.docx",
                    "ops": [{"op": "append_paragraph", "text": "追加的第二段"}]
                }),
                &ctx,
            )
            .await
            .expect("edit_word_document");
        assert!(edited.content.contains("1 op"));

        let read2 = reg
            .run(
                "read_document",
                json!({"path": "reports/demo.docx"}),
                &ctx,
            )
            .await
            .expect("read_document docx after edit");
        assert!(
            read2.content.contains("追加的第二段"),
            "after edit: {}",
            read2.content
        );

        // ---- PPT: create → read → add slide ----------------------------------
        eprintln!("[sim] create_presentation …");
        reg.run(
            "create_presentation",
            json!({
                "path": "slides/demo.pptx",
                "slides": [{
                    "layout": "titleBody",
                    "title": "项目汇报",
                    "bullets": ["里程碑 A", "里程碑 B"]
                }]
            }),
            &ctx,
        )
        .await
        .expect("create_presentation");

        let ppt_read = reg
            .run(
                "read_document",
                json!({"path": "slides/demo.pptx"}),
                &ctx,
            )
            .await
            .expect("read_document pptx");
        assert!(
            ppt_read.content.contains("项目汇报") && ppt_read.content.contains("里程碑 A"),
            "pptx: {}",
            ppt_read.content
        );

        eprintln!("[sim] edit_presentation …");
        reg.run(
            "edit_presentation",
            json!({
                "path": "slides/demo.pptx",
                "ops": [{
                    "op": "add_slide",
                    "layout": "titleBody",
                    "title": "第二页",
                    "bullets": ["新要点"]
                }]
            }),
            &ctx,
        )
        .await
        .expect("edit_presentation");

        let ppt2 = reg
            .run(
                "read_document",
                json!({"path": "slides/demo.pptx"}),
                &ctx,
            )
            .await
            .expect("read pptx after edit");
        assert!(
            ppt2.content.contains("第二页") && ppt2.content.contains("新要点"),
            "ppt after edit: {}",
            ppt2.content
        );

        // ---- PDF: create → read ----------------------------------------------
        eprintln!("[sim] create_pdf …");
        reg.run(
            "create_pdf",
            json!({
                "path": "out/summary.pdf",
                "title": "PDF 摘要",
                "text": "这是 PDF 正文行。\n第二行。"
            }),
            &ctx,
        )
        .await
        .expect("create_pdf");
        assert!(root.join("out/summary.pdf").is_file());

        let pdf_read = reg
            .run(
                "read_document",
                json!({"path": "out/summary.pdf"}),
                &ctx,
            )
            .await
            .expect("read_document pdf");
        // pdf-extract may omit some CJK depending on font embedding; Latin should work.
        assert!(
            pdf_read.content.contains("PDF") || pdf_read.content.contains("正文"),
            "pdf text: {}",
            pdf_read.content
        );

        // ---- guardrail: edit external docx without sidecar -------------------
        eprintln!("[sim] edit without sidecar (expect DOC_NO_SIDECAR) …");
        let orphan_bytes =
            compile_to_buffer(&build_word_ir(&[json!({"type":"paragraph","text":"orphan"})]).unwrap(), ExportFormat::Docx, None).unwrap();
        std::fs::write(root.join("external.docx"), orphan_bytes).unwrap();
        let err = reg
            .run(
                "edit_word_document",
                json!({
                    "path": "external.docx",
                    "ops": [{"op": "append_paragraph", "text": "nope"}]
                }),
                &ctx,
            )
            .await
            .expect_err("should reject missing sidecar");
        assert_eq!(err.code, "DOC_NO_SIDECAR", "got {:?}", err);

        eprintln!("[sim] full document tool flow OK");
    }

    /// IDE integration: read a sidecar-backed pptx as IR, mutate it, write it
    /// back, and confirm the recompiled file + sidecar reflect the change.
    #[test]
    fn workspace_document_roundtrip_pptx() {
        let root = std::env::temp_dir().join(format!(
            "ws_doc_rt_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let rel = "docs/deck.pptx";
        let abs = root.join(rel);

        // Seed a pptx + sidecar the way create_presentation would.
        let ir = build_ppt_ir(&[json!({
            "layout": "titleBody",
            "title": "原标题",
            "bullets": ["要点一"]
        })])
        .unwrap();
        let bytes = compile_to_buffer(&ir, ExportFormat::Pptx, None).unwrap();
        write_file_and_sidecar_sync(&abs, &bytes, &to_json(&ir).unwrap()).unwrap();

        // Read back as editable IR.
        let read = read_workspace_document(&root, rel).unwrap();
        assert_eq!(read["kind"], "pptx");
        assert_eq!(read["editable"], true);
        let mut ir_val = read["ir"].clone();

        // Mutate the first text element's text.
        let elements = ir_val["slides"][0]["elements"].as_array_mut().unwrap();
        let mut changed = false;
        for el in elements.iter_mut() {
            if el["props"]["text"].is_string() {
                el["props"]["text"] = json!("新标题");
                changed = true;
                break;
            }
        }
        assert!(changed, "expected a text element with text prop");

        // Persist and verify the recompile + sidecar update round-trips.
        let w = write_workspace_document(&root, rel, ir_val).unwrap();
        assert_eq!(w["ok"], true);

        let reread = read_workspace_document(&root, rel).unwrap();
        let dump = reread["ir"].to_string();
        assert!(dump.contains("新标题"), "ir missing new text: {dump}");
        assert!(!dump.contains("原标题"), "ir still has old text: {dump}");

        let side = std::fs::read_to_string(sidecar_path(&abs)).unwrap();
        assert!(side.contains("新标题"), "sidecar not updated: {side}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Sidecar is written before the OOXML file so the IDE never sees stale IR
    /// after a partial failure (sidecar is the source of truth).
    #[test]
    fn write_file_and_sidecar_writes_sidecar_before_office() {
        let root = std::env::temp_dir().join(format!(
            "ws_doc_order_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let abs = root.join("doc.docx");
        let ir = build_word_ir(&[json!({"type": "paragraph", "text": "order test"})]).unwrap();
        let bytes = compile_to_buffer(&ir, ExportFormat::Docx, None).unwrap();
        let sidecar = to_json(&ir).unwrap();

        write_file_and_sidecar_sync(&abs, &bytes, &sidecar).unwrap();

        let side = sidecar_path(&abs);
        assert!(side.is_file() && abs.is_file());
        let side_mtime = side.metadata().unwrap().modified().unwrap();
        let office_mtime = abs.metadata().unwrap().modified().unwrap();
        assert!(
            side_mtime <= office_mtime,
            "sidecar should be written before office file"
        );
        assert!(!tmp_path(&side).exists());
        assert!(!tmp_path(&abs).exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// IDE integration error paths: no sidecar, pdf read-only, unsupported ext,
    /// and IR/extension type mismatch on write.
    #[test]
    fn workspace_document_error_paths() {
        let root = std::env::temp_dir().join(format!(
            "ws_doc_err_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();

        // docx without sidecar => DOC_NO_SIDECAR
        let ir = build_word_ir(&[json!({"type":"paragraph","text":"x"})]).unwrap();
        let bytes = compile_to_buffer(&ir, ExportFormat::Docx, None).unwrap();
        std::fs::write(root.join("nofile.docx"), bytes).unwrap();
        let err = read_workspace_document(&root, "nofile.docx").unwrap_err();
        assert_eq!(err.code, "DOC_NO_SIDECAR", "got {err:?}");

        // pdf => read-only text preview
        let pdf = generate_pdf("标题", &["hello world".to_string()]).unwrap();
        std::fs::write(root.join("doc.pdf"), pdf).unwrap();
        let read = read_workspace_document(&root, "doc.pdf").unwrap();
        assert_eq!(read["kind"], "pdf");
        assert_eq!(read["editable"], false);
        assert!(read["text"].is_string());

        // unsupported extension
        std::fs::write(root.join("a.txt"), b"hi").unwrap();
        let err2 = read_workspace_document(&root, "a.txt").unwrap_err();
        assert_eq!(err2.code, "UNSUPPORTED_DOCUMENT", "got {err2:?}");

        // write: word IR into a .pptx path => DOC_TYPE_MISMATCH
        let word_ir = serde_json::to_value(
            build_word_ir(&[json!({"type":"paragraph","text":"y"})]).unwrap(),
        )
        .unwrap();
        let err3 = write_workspace_document(&root, "mismatch.pptx", word_ir).unwrap_err();
        assert_eq!(err3.code, "DOC_TYPE_MISMATCH", "got {err3:?}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
