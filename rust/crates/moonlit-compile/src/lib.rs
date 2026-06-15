//! IR -> OOXML compiler for the native rewrite.
//!
//! This crate intentionally avoids Node dependencies. Word and PPTX are emitted
//! as minimal OOXML packages using `zip`; the surface mirrors the previous
//! compile-engine contract (`compileToBuffer`, `exportToFile`,
//! hash-cache queue).

use moonlit_doccore::{to_json, DocIR, ExportFormat, Slide, WordBlockType, WordMark};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zip::write::SimpleFileOptions;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("doccore error: {0}")]
    DocCore(#[from] moonlit_doccore::DocCoreError),
    #[error("unsupported format {format:?} for document")]
    UnsupportedFormat { format: ExportFormat },
}

pub type Result<T> = std::result::Result<T, CompileError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileOptions {
    pub only_slide_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompileJob {
    pub ir: DocIR,
    pub format: ExportFormat,
    pub options: Option<CompileOptions>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CompileStats {
    pub compiles: u64,
    pub cache_hits: u64,
    pub cancels: u64,
    pub debounced: u64,
}

pub fn default_format_for(ir: &DocIR) -> ExportFormat {
    match ir {
        DocIR::Word { .. } => ExportFormat::Docx,
        DocIR::Ppt { .. } => ExportFormat::Pptx,
    }
}

pub fn compile_to_buffer(
    ir: &DocIR,
    format: ExportFormat,
    options: Option<&CompileOptions>,
) -> Result<Vec<u8>> {
    match format {
        ExportFormat::Json => Ok(to_json(ir)?.into_bytes()),
        ExportFormat::Docx => compile_word(ir),
        ExportFormat::Pptx => compile_ppt(ir, options),
    }
}

pub fn export_to_file(
    ir: &DocIR,
    format: ExportFormat,
    out_path: impl AsRef<Path>,
) -> Result<PathBuf> {
    let bytes = compile_to_buffer(ir, format, None)?;
    let path = out_path.as_ref().to_path_buf();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, bytes)?;
    Ok(path)
}

pub fn compile_job_key(job: &CompileJob) -> String {
    let effective = match (&job.ir, &job.options) {
        (DocIR::Ppt { slides }, Some(opts)) => {
            if let Some(id) = &opts.only_slide_id {
                let filtered: Vec<Slide> = slides.iter().filter(|s| &s.id == id).cloned().collect();
                serde_json::json!({ "type": "ppt", "slides": filtered })
            } else {
                serde_json::to_value(&job.ir).expect("IR serializes")
            }
        }
        _ => serde_json::to_value(&job.ir).expect("IR serializes"),
    };
    let hash = moonlit_core::content_hash(&effective);
    format!(
        "{:?}:{}:{}",
        job.format,
        job.options
            .as_ref()
            .and_then(|o| o.only_slide_id.as_deref())
            .unwrap_or(""),
        hash
    )
}

pub struct CompileQueue {
    max_cache: usize,
    cache: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
    stats: CompileStats,
}

impl CompileQueue {
    pub fn new(max_cache: usize) -> Self {
        Self {
            max_cache: max_cache.max(1),
            cache: HashMap::new(),
            order: VecDeque::new(),
            stats: CompileStats::default(),
        }
    }

    pub fn stats(&self) -> CompileStats {
        self.stats
    }

    pub fn has(&self, key: &str) -> bool {
        self.cache.contains_key(key)
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.order.clear();
    }

    pub fn run_now(&mut self, job: CompileJob) -> Result<Vec<u8>> {
        let key = compile_job_key(&job);
        if let Some(bytes) = self.cache.get(&key) {
            self.stats.cache_hits += 1;
            return Ok(bytes.clone());
        }
        let bytes = compile_to_buffer(&job.ir, job.format, job.options.as_ref())?;
        self.stats.compiles += 1;
        self.insert_cache(key, bytes.clone());
        Ok(bytes)
    }

    fn insert_cache(&mut self, key: String, bytes: Vec<u8>) {
        if !self.cache.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.cache.insert(key, bytes);
        while self.order.len() > self.max_cache {
            if let Some(old) = self.order.pop_front() {
                self.cache.remove(&old);
            }
        }
    }
}

fn compile_word(ir: &DocIR) -> Result<Vec<u8>> {
    let DocIR::Word { blocks } = ir else {
        return Err(CompileError::UnsupportedFormat {
            format: ExportFormat::Docx,
        });
    };
    let mut document = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
    );
    for block in blocks {
        let style = match block.block_type {
            WordBlockType::Heading => {
                format!(
                    r#"<w:pPr><w:pStyle w:val="Heading{}"/></w:pPr>"#,
                    block.level.unwrap_or(1)
                )
            }
            WordBlockType::Paragraph => block
                .style
                .as_ref()
                .map(|s| format!(r#"<w:pPr><w:pStyle w:val="{}"/></w:pPr>"#, xml(s)))
                .unwrap_or_default(),
        };
        document.push_str("<w:p>");
        document.push_str(&style);
        for run in &block.runs {
            document.push_str("<w:r>");
            let props = word_run_props(&run.marks, run.style.as_deref());
            if !props.is_empty() {
                document.push_str("<w:rPr>");
                document.push_str(&props);
                document.push_str("</w:rPr>");
            }
            document.push_str(&format!(
                r#"<w:t xml:space="preserve">{}</w:t>"#,
                xml(&run.text)
            ));
            document.push_str("</w:r>");
        }
        document.push_str("</w:p>");
    }
    document.push_str(
        r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>"#,
    );

    let files = vec![
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
        ("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_string()),
        ("word/styles.xml", word_styles()),
        ("word/document.xml", document),
    ];
    zip_files(files)
}

/// A `styles.xml` defining Normal + Heading1..6 so `w:pStyle` references resolve
/// to real, visually distinct styles in Word.
fn word_styles() -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>"#,
    );
    let sizes = [(1u8, 36), (2, 32), (3, 28), (4, 26), (5, 24), (6, 22)];
    for (level, half_pts) in sizes {
        s.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="Heading{level}"><w:name w:val="heading {level}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="60"/><w:outlineLvl w:val="{}"/></w:pPr><w:rPr><w:b/><w:sz w:val="{half_pts}"/></w:rPr></w:style>"#,
            level - 1
        ));
    }
    s.push_str("</w:styles>");
    s
}

fn word_run_props(marks: &[WordMark], style: Option<&str>) -> String {
    let mut props = String::new();
    if marks.contains(&WordMark::Bold) {
        props.push_str("<w:b/>");
    }
    if marks.contains(&WordMark::Italic) {
        props.push_str("<w:i/>");
    }
    if marks.contains(&WordMark::Underline) {
        props.push_str(r#"<w:u w:val="single"/>"#);
    }
    if marks.contains(&WordMark::Strike) {
        props.push_str("<w:strike/>");
    }
    if marks.contains(&WordMark::Code) {
        props.push_str(r#"<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/>"#);
    }
    if let Some(style) = style {
        props.push_str(&format!(r#"<w:rStyle w:val="{}"/>"#, xml(style)));
    }
    props
}

fn compile_ppt(ir: &DocIR, options: Option<&CompileOptions>) -> Result<Vec<u8>> {
    let DocIR::Ppt { slides } = ir else {
        return Err(CompileError::UnsupportedFormat {
            format: ExportFormat::Pptx,
        });
    };
    let selected: Vec<&Slide> = match options.and_then(|o| o.only_slide_id.as_deref()) {
        Some(id) => slides.iter().filter(|s| s.id == id).collect(),
        None => slides.iter().collect(),
    };
    let slide_count = selected.len().max(1);
    let mut all: Vec<(String, String)> = Vec::new();
    all.push(("[Content_Types].xml".into(), ppt_content_types(slide_count)));
    all.push(("_rels/.rels".into(), r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#.into()));
    all.push(("ppt/presentation.xml".into(), ppt_presentation(slide_count)));
    all.push((
        "ppt/_rels/presentation.xml.rels".into(),
        ppt_presentation_rels(slide_count),
    ));
    all.push((
        "ppt/slideMasters/slideMaster1.xml".into(),
        ppt_slide_master(),
    ));
    all.push((
        "ppt/slideMasters/_rels/slideMaster1.xml.rels".into(),
        ppt_master_rels(),
    ));
    all.push((
        "ppt/slideLayouts/slideLayout1.xml".into(),
        ppt_slide_layout(),
    ));
    all.push((
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels".into(),
        ppt_layout_rels(),
    ));
    all.push(("ppt/theme/theme1.xml".into(), ppt_theme()));

    for (idx, slide) in selected.iter().enumerate() {
        let n = idx + 1;
        all.push((format!("ppt/slides/slide{n}.xml"), ppt_slide(slide)));
        all.push((
            format!("ppt/slides/_rels/slide{n}.xml.rels"),
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#.into(),
        ));
    }
    let borrowed: Vec<(&str, String)> = all.iter().map(|(p, b)| (p.as_str(), b.clone())).collect();
    zip_files(borrowed)
}

fn ppt_content_types(slide_count: usize) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#,
    );
    for idx in 1..=slide_count {
        s.push_str(&format!(r#"<Override PartName="/ppt/slides/slide{idx}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#));
    }
    s.push_str("</Types>");
    s
}

fn ppt_presentation(slide_count: usize) -> String {
    let mut ids = String::new();
    for idx in 1..=slide_count {
        ids.push_str(&format!(
            r#"<p:sldId id="{}" r:id="rId{}"/>"#,
            255 + idx,
            idx
        ));
    }
    let master_rid = slide_count + 1;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId{master_rid}"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="5143500" type="screen16x9"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
    )
}

fn ppt_presentation_rels(slide_count: usize) -> String {
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for idx in 1..=slide_count {
        rels.push_str(&format!(r#"<Relationship Id="rId{idx}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{idx}.xml"/>"#));
    }
    rels.push_str(&format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#, slide_count + 1));
    rels.push_str("</Relationships>");
    rels
}

fn ppt_master_rels() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#.to_string()
}

fn ppt_layout_rels() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#.to_string()
}

const CLR_MAP: &str = r#"<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;

fn ppt_slide_master() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld>{CLR_MAP}<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#
    )
}

fn ppt_slide_layout() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1"><p:cSld name="Blank"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:overrideClrMapping bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr></p:sldLayout>"#.to_string()
}

fn ppt_theme() -> String {
    let scheme_color =
        |tag: &str, val: &str| format!(r#"<a:{tag}><a:srgbClr val="{val}"/></a:{tag}>"#);
    let clr = format!(
        r#"<a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>{}{}{}{}{}{}{}{}{}{}</a:clrScheme>"#,
        scheme_color("dk2", "44546A"),
        scheme_color("lt2", "E7E6E6"),
        scheme_color("accent1", "4472C4"),
        scheme_color("accent2", "ED7D31"),
        scheme_color("accent3", "A5A5A5"),
        scheme_color("accent4", "FFC000"),
        scheme_color("accent5", "5B9BD5"),
        scheme_color("accent6", "70AD47"),
        scheme_color("hlink", "0563C1"),
        scheme_color("folHlink", "954F72"),
    );
    let font = r#"<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>"#;
    let fill_lst = r#"<a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>"#;
    let ln_lst = r#"<a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>"#;
    let effect_lst = r#"<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>"#;
    let bg_lst = r#"<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>"#;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements>{clr}{font}<a:fmtScheme name="Office">{fill_lst}{ln_lst}{effect_lst}{bg_lst}</a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#
    )
}

fn ppt_slide(slide: &Slide) -> String {
    let mut sp_tree = String::from(
        r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>"#,
    );
    for (idx, el) in slide.elements.iter().enumerate() {
        let id = idx + 2;
        let text = el
            .props
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let font_size = el
            .props
            .get("fontSize")
            .and_then(|v| v.as_f64())
            .unwrap_or(18.0)
            * 100.0;
        sp_tree.push_str(&format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="{}"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>"#,
            xml(&el.id),
            inch_to_emu(el.geo.x),
            inch_to_emu(el.geo.y),
            inch_to_emu(el.geo.w),
            inch_to_emu(el.geo.h),
            font_size.round() as i64,
            xml(text)
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree>{sp_tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn inch_to_emu(v: f64) -> i64 {
    (v * 914_400.0).round() as i64
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn zip_files<'a>(files: Vec<(&'a str, String)>) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, body) in files {
            zip.start_file(path, options)?;
            zip.write_all(body.as_bytes())?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use moonlit_doccore::{DocCore, DocCoreOptions, DocType, NewWordBlock, WordBlockType};

    #[test]
    fn json_compile_works() {
        let ir = DocIR::Word { blocks: Vec::new() };
        let bytes = compile_to_buffer(&ir, ExportFormat::Json, None).unwrap();
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains("\"type\": \"word\""));
    }

    #[test]
    fn docx_compile_emits_zip() {
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
        let bytes = compile_to_buffer(&core.read_document(), ExportFormat::Docx, None).unwrap();
        assert!(bytes.starts_with(b"PK"));
    }

    #[test]
    fn pptx_has_office_required_parts() {
        let core = DocCore::new(DocCoreOptions::new(DocType::Ppt));
        core.add_slide(0, "title").unwrap();
        let bytes = compile_to_buffer(&core.read_document(), ExportFormat::Pptx, None).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        for required in [
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/theme/theme1.xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        ] {
            assert!(names.iter().any(|n| n == required), "missing {required}");
        }
    }

    #[test]
    fn docx_has_styles_part() {
        let core = DocCore::new(DocCoreOptions::new(DocType::Word));
        core.insert_block(
            None,
            NewWordBlock {
                block_type: WordBlockType::Heading,
                level: Some(1),
                style: None,
                text: Some("Title".to_string()),
                runs: None,
            },
        )
        .unwrap();
        let bytes = compile_to_buffer(&core.read_document(), ExportFormat::Docx, None).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "word/styles.xml"));
        assert!(names.iter().any(|n| n == "word/_rels/document.xml.rels"));
    }

    #[test]
    fn pptx_compile_emits_zip_and_queue_caches() {
        let ir = DocIR::Ppt { slides: Vec::new() };
        let job = CompileJob {
            ir,
            format: ExportFormat::Pptx,
            options: None,
        };
        let mut queue = CompileQueue::new(2);
        let a = queue.run_now(job.clone()).unwrap();
        let b = queue.run_now(job).unwrap();
        assert_eq!(a, b);
        assert_eq!(queue.stats().cache_hits, 1);
    }
}
