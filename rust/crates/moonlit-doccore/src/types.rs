use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type Result<T> = std::result::Result<T, DocCoreError>;

#[derive(Debug, thiserror::Error)]
pub enum DocCoreError {
    #[error("mutation is only valid for {expected}, current document type is {actual}")]
    WrongType {
        expected: &'static str,
        actual: DocType,
    },
    #[error("{op}: id not found: {id}")]
    NotFound { op: &'static str, id: String },
    #[error("export: no exporter configured")]
    MissingExporter,
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocType {
    Word,
    Ppt,
}

impl DocType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Ppt => "ppt",
        }
    }
}

impl std::fmt::Display for DocType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordMark {
    Bold,
    Italic,
    Underline,
    Code,
    Strike,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordTextRun {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<WordMark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordBlockType {
    Paragraph,
    Heading,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordBlock {
    pub id: String,
    #[serde(rename = "type")]
    pub block_type: WordBlockType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default)]
    pub runs: Vec<WordTextRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordDocIR {
    #[serde(rename = "type")]
    pub doc_type: WordDocKind,
    #[serde(default)]
    pub blocks: Vec<WordBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "word")]
pub enum WordDocKind {
    Word,
}

impl Default for WordDocIR {
    fn default() -> Self {
        Self {
            doc_type: WordDocKind::Word,
            blocks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geo {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rot: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementType {
    Text,
    Shape,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideElement {
    pub id: String,
    #[serde(rename = "type")]
    pub element_type: ElementType,
    pub geo: Geo,
    #[serde(default)]
    pub props: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slide {
    pub id: String,
    pub layout: String,
    #[serde(default)]
    pub elements: Vec<SlideElement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PptDocIR {
    #[serde(rename = "type")]
    pub doc_type: PptDocKind,
    #[serde(default)]
    pub slides: Vec<Slide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "ppt")]
pub enum PptDocKind {
    Ppt,
}

impl Default for PptDocIR {
    fn default() -> Self {
        Self {
            doc_type: PptDocKind::Ppt,
            slides: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DocIR {
    Word { blocks: Vec<WordBlock> },
    Ppt { slides: Vec<Slide> },
}

impl DocIR {
    pub fn doc_type(&self) -> DocType {
        match self {
            Self::Word { .. } => DocType::Word,
            Self::Ppt { .. } => DocType::Ppt,
        }
    }
}

impl From<WordDocIR> for DocIR {
    fn from(value: WordDocIR) -> Self {
        Self::Word {
            blocks: value.blocks,
        }
    }
}

impl From<PptDocIR> for DocIR {
    fn from(value: PptDocIR) -> Self {
        Self::Ppt {
            slides: value.slides,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWordBlock {
    #[serde(rename = "type")]
    pub block_type: WordBlockType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<Vec<WordTextRun>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    pub id: String,
    pub level: u8,
    pub text: String,
}

pub type Outline = Vec<OutlineItem>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Docx,
    Pptx,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocChangeOrigin {
    Local,
    Remote,
    Init,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocChangeEvent {
    pub origin: DocChangeOrigin,
    pub hash: String,
}
