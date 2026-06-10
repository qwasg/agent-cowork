//! `DocCore` — the single writer/reader contract, now backed by a real
//! `yrs::Doc`.
//!
//! Mutations write into the CRDT (Word = `XmlFragment("word")`, PPT =
//! `Array("ppt")` of slide `Map`s) inside `local`-tagged transactions, exactly
//! mirroring the JS `doc-core` bindings so updates interoperate with Yjs peers
//! and the embedded sync server. `read_document` projects the IR back out of
//! the CRDT, keeping the content hash byte-compatible with the JS port.

use crate::layouts::get_layout_seeds;
use crate::{
    DocChangeEvent, DocChangeOrigin, DocCoreError, DocIR, DocType, ElementType, ExportFormat, Geo,
    NewWordBlock, Outline, OutlineItem, Result, Slide, SlideElement, StyleInput, WordBlock,
    WordBlockType, WordMark, WordTextRun,
};
use moonlit_core::{content_hash, DefaultIdFactory, IdFactory};
use serde_json::{Map as JsonMap, Value};
use std::sync::{Arc, Mutex};
use yrs::types::Attrs;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Array, ArrayPrelim, Doc, GetString, In, Map, MapPrelim, MapRef, OffsetKind, Options,
    Origin, Out, ReadTxn, StateVector, Subscription, Text, Transact, Update, WriteTxn, Xml,
    XmlElementPrelim, XmlFragment, XmlFragmentRef, XmlOut, XmlTextPrelim, XmlTextRef,
};

type Exporter = Arc<dyn Fn(&DocIR, ExportFormat) -> Result<String> + Send + Sync>;
type Callback = Arc<dyn Fn(DocChangeEvent) + Send + Sync>;

const MARK_KEYS: [WordMark; 5] = [
    WordMark::Bold,
    WordMark::Italic,
    WordMark::Underline,
    WordMark::Code,
    WordMark::Strike,
];

fn mark_key(mark: &WordMark) -> &'static str {
    match mark {
        WordMark::Bold => "bold",
        WordMark::Italic => "italic",
        WordMark::Underline => "underline",
        WordMark::Code => "code",
        WordMark::Strike => "strike",
    }
}

fn origin_local() -> Origin {
    Origin::from("docforge.local".to_string())
}

fn origin_remote() -> Origin {
    Origin::from("docforge.remote".to_string())
}

#[derive(Clone)]
pub struct DocCoreOptions {
    pub doc_type: DocType,
    pub id_factory: Arc<dyn IdFactory>,
    pub exporter: Option<Exporter>,
}

impl DocCoreOptions {
    pub fn new(doc_type: DocType) -> Self {
        Self {
            doc_type,
            id_factory: Arc::new(DefaultIdFactory::new()),
            exporter: None,
        }
    }
}

/// DocCore is the single writer/reader contract for documents.
pub struct DocCore {
    doc_type: DocType,
    doc: Doc,
    id_factory: Arc<dyn IdFactory>,
    exporter: Option<Exporter>,
    callbacks: Mutex<Vec<(usize, Callback)>>,
    next_observer_id: Mutex<usize>,
    subscriptions: Mutex<Vec<Subscription>>,
}

impl DocCore {
    pub fn new(options: DocCoreOptions) -> Self {
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        Self {
            doc_type: options.doc_type,
            doc,
            id_factory: options.id_factory,
            exporter: options.exporter,
            callbacks: Mutex::new(Vec::new()),
            next_observer_id: Mutex::new(0),
            subscriptions: Mutex::new(Vec::new()),
        }
    }

    pub fn from_ir(
        ir: DocIR,
        id_factory: Arc<dyn IdFactory>,
        exporter: Option<Exporter>,
    ) -> Self {
        let doc_type = ir.doc_type();
        let core = Self::new(DocCoreOptions {
            doc_type,
            id_factory,
            exporter,
        });
        core.load_ir(&ir);
        core
    }

    pub fn doc_type(&self) -> DocType {
        self.doc_type
    }

    /// Borrow the underlying CRDT document (for sync bridging).
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /* ----------------------------- read ----------------------------- */

    pub fn read_document(&self) -> DocIR {
        let txn = self.doc.transact();
        match self.doc_type {
            DocType::Word => DocIR::Word {
                blocks: read_word(&txn),
            },
            DocType::Ppt => DocIR::Ppt {
                slides: read_ppt(&txn),
            },
        }
    }

    pub fn get_outline(&self) -> Outline {
        match self.read_document() {
            DocIR::Word { blocks } => blocks
                .into_iter()
                .filter(|b| b.block_type == WordBlockType::Heading)
                .map(|b| OutlineItem {
                    level: b.level.unwrap_or(1),
                    text: b.runs.iter().map(|r| r.text.as_str()).collect(),
                    id: b.id,
                })
                .collect(),
            DocIR::Ppt { slides } => slides
                .iter()
                .enumerate()
                .map(|(i, slide)| {
                    let title = slide
                        .elements
                        .iter()
                        .find(|el| {
                            el.element_type == ElementType::Text
                                && el.props.get("text").and_then(Value::as_str).is_some()
                        })
                        .and_then(|el| el.props.get("text").and_then(Value::as_str))
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Slide {}", i + 1));
                    OutlineItem {
                        id: slide.id.clone(),
                        level: 1,
                        text: title,
                    }
                })
                .collect(),
        }
    }

    pub fn hash(&self) -> String {
        let value = serde_json::to_value(self.read_document()).expect("DocIR serializes");
        content_hash(&value)
    }

    /* ------------------------- Word mutation ------------------------ */

    pub fn insert_block(&self, after_id: Option<&str>, node: NewWordBlock) -> Result<String> {
        self.assert_type(DocType::Word)?;
        let id = self.id_factory.next("blk");
        let runs = node.runs.unwrap_or_else(|| {
            node.text
                .map(|text| {
                    vec![WordTextRun {
                        text,
                        marks: Vec::new(),
                        style: None,
                    }]
                })
                .unwrap_or_default()
        });
        let block_type = node.block_type;
        let level = node.level;
        let style = node.style;
        {
            let mut txn = self.doc.transact_mut_with(origin_local());
            let root = txn.get_or_insert_xml_fragment("word");
            let index = match after_id {
                None => 0,
                Some(after) => {
                    find_block_index(&txn, &root, after).ok_or_else(|| DocCoreError::NotFound {
                        op: "insert_block",
                        id: after.to_string(),
                    })? + 1
                }
            };
            let tag = match block_type {
                WordBlockType::Heading => "heading",
                WordBlockType::Paragraph => "paragraph",
            };
            let el = root.insert(&mut txn, index as u32, XmlElementPrelim::empty(tag));
            el.insert_attribute(&mut txn, "id", id.clone());
            if block_type == WordBlockType::Heading {
                el.insert_attribute(&mut txn, "level", level.unwrap_or(1).to_string());
            }
            if let Some(style) = &style {
                el.insert_attribute(&mut txn, "style", style.clone());
            }
            let text = el.insert(&mut txn, 0, XmlTextPrelim::new(""));
            for run in &runs {
                if run.text.is_empty() {
                    continue;
                }
                let at = text.len(&txn);
                let attrs = run_attrs(run);
                if attrs.is_empty() {
                    text.insert(&mut txn, at, &run.text);
                } else {
                    text.insert_with_attributes(&mut txn, at, &run.text, attrs);
                }
            }
        }
        self.emit(DocChangeOrigin::Local);
        Ok(id)
    }

    pub fn replace_text(&self, range_id: &str, text: impl Into<String>) -> Result<()> {
        self.assert_type(DocType::Word)?;
        let text = text.into();
        {
            let mut txn = self.doc.transact_mut_with(origin_local());
            let root = txn.get_or_insert_xml_fragment("word");
            let idx = find_block_index(&txn, &root, range_id).ok_or_else(|| {
                DocCoreError::NotFound {
                    op: "replace_text",
                    id: range_id.to_string(),
                }
            })?;
            let Some(XmlOut::Element(el)) = root.get(&txn, idx as u32) else {
                return Err(DocCoreError::NotFound {
                    op: "replace_text",
                    id: range_id.to_string(),
                });
            };
            let ytext = get_or_create_block_text(&mut txn, &el);
            let len = ytext.len(&txn);
            if len > 0 {
                ytext.remove_range(&mut txn, 0, len);
            }
            if !text.is_empty() {
                ytext.insert(&mut txn, 0, &text);
            }
        }
        self.emit(DocChangeOrigin::Local);
        Ok(())
    }

    pub fn apply_style(&self, range_id: &str, style: StyleInput) -> Result<()> {
        self.assert_type(DocType::Word)?;
        {
            let mut txn = self.doc.transact_mut_with(origin_local());
            let root = txn.get_or_insert_xml_fragment("word");
            let idx = find_block_index(&txn, &root, range_id).ok_or_else(|| {
                DocCoreError::NotFound {
                    op: "apply_style",
                    id: range_id.to_string(),
                }
            })?;
            let Some(XmlOut::Element(el)) = root.get(&txn, idx as u32) else {
                return Err(DocCoreError::NotFound {
                    op: "apply_style",
                    id: range_id.to_string(),
                });
            };
            let attrs = style_attrs(&style);
            if let Some(ytext) = get_block_text(&txn, &el) {
                let len = ytext.len(&txn);
                if len > 0 && !attrs.is_empty() {
                    ytext.format(&mut txn, 0, len, attrs);
                }
            }
            if let Some(named) = &style.style {
                el.insert_attribute(&mut txn, "style", named.clone());
            }
        }
        self.emit(DocChangeOrigin::Local);
        Ok(())
    }

    /* ------------------------- PPT mutation ------------------------- */

    pub fn add_slide(&self, index: usize, layout: impl Into<String>) -> Result<String> {
        self.assert_type(DocType::Ppt)?;
        let layout = layout.into();
        let slide_id = self.id_factory.next("sld");
        {
            let mut txn = self.doc.transact_mut_with(origin_local());
            let root = txn.get_or_insert_array("ppt");
            let mut elements: Vec<In> = Vec::new();
            for seed in get_layout_seeds(&layout) {
                let mut props = seed.props.clone();
                props.insert("role".to_string(), Value::String(seed.role.to_string()));
                let el = MapPrelim::from_iter([
                    ("id", In::from(Any::String(self.id_factory.next("el").into()))),
                    ("type", In::from(Any::String(element_type_str(seed.element_type).into()))),
                    ("geo", In::from(geo_prelim(&seed.geo))),
                    ("props", In::from(props_prelim(&props))),
                ]);
                elements.push(In::from(el));
            }
            let slide = MapPrelim::from_iter([
                ("id", In::from(Any::String(slide_id.clone().into()))),
                ("layout", In::from(Any::String(layout.clone().into()))),
                ("elements", In::from(ArrayPrelim::from(elements))),
            ]);
            let at = index.min(root.len(&txn) as usize);
            root.insert(&mut txn, at as u32, slide);
        }
        self.emit(DocChangeOrigin::Local);
        Ok(slide_id)
    }

    pub fn edit_element(
        &self,
        slide_id: &str,
        el_id: &str,
        props: JsonMap<String, Value>,
    ) -> Result<()> {
        self.assert_type(DocType::Ppt)?;
        {
            let mut txn = self.doc.transact_mut_with(origin_local());
            let root = txn.get_or_insert_array("ppt");
            let props_map = locate_element_child(&txn, &root, slide_id, el_id, "props", "edit_element")?;
            for (key, value) in props {
                if value.is_null() {
                    props_map.remove(&mut txn, &key);
                } else {
                    props_map.insert(&mut txn, key, value_to_any(&value));
                }
            }
        }
        self.emit(DocChangeOrigin::Local);
        Ok(())
    }

    pub fn move_element(&self, slide_id: &str, el_id: &str, geo: PartialGeo) -> Result<()> {
        self.assert_type(DocType::Ppt)?;
        {
            let mut txn = self.doc.transact_mut_with(origin_local());
            let root = txn.get_or_insert_array("ppt");
            let geo_map = locate_element_child(&txn, &root, slide_id, el_id, "geo", "move_element")?;
            if let Some(x) = geo.x {
                geo_map.insert(&mut txn, "x", Any::Number(x));
            }
            if let Some(y) = geo.y {
                geo_map.insert(&mut txn, "y", Any::Number(y));
            }
            if let Some(w) = geo.w {
                geo_map.insert(&mut txn, "w", Any::Number(w));
            }
            if let Some(h) = geo.h {
                geo_map.insert(&mut txn, "h", Any::Number(h));
            }
            if let Some(rot) = geo.rot {
                geo_map.insert(&mut txn, "rot", Any::Number(rot));
            }
        }
        self.emit(DocChangeOrigin::Local);
        Ok(())
    }

    /* ------------------------------ export -------------------------- */

    pub fn export(&self, format: ExportFormat) -> Result<String> {
        let exporter = self.exporter.as_ref().ok_or(DocCoreError::MissingExporter)?;
        exporter(&self.read_document(), format)
    }

    /* --------------------------- sync bridge ------------------------ */

    /// Encode the whole document as a `update_v1` payload (Yjs-compatible).
    pub fn encode_state_as_update(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    }

    /// Apply a remote `update_v1` payload (e.g. from the sync server / Yjs).
    pub fn apply_update(&self, update: &[u8]) -> Result<()> {
        let update = Update::decode_v1(update)
            .map_err(|e| DocCoreError::NotFound { op: "apply_update", id: e.to_string() })?;
        {
            let mut txn = self.doc.transact_mut_with(origin_remote());
            txn.apply_update(update)
                .map_err(|e| DocCoreError::NotFound { op: "apply_update", id: e.to_string() })?;
        }
        self.emit(DocChangeOrigin::Remote);
        Ok(())
    }

    /// Subscribe to raw CRDT updates so a bridge can forward them. The callback
    /// receives the `update_v1` bytes and whether the change originated locally.
    pub fn on_update<F>(&self, f: F)
    where
        F: Fn(Vec<u8>, bool) + Send + Sync + 'static,
    {
        let local = origin_local();
        if let Ok(sub) = self.doc.observe_update_v1(move |txn, e| {
            let is_local = txn.origin() == Some(&local);
            f(e.update.clone(), is_local);
        }) {
            self.subscriptions.lock().unwrap().push(sub);
        }
    }

    /* ----------------------------- observe -------------------------- */

    pub fn observe<F>(&self, callback: F) -> usize
    where
        F: Fn(DocChangeEvent) + Send + Sync + 'static,
    {
        let mut next = self.next_observer_id.lock().unwrap();
        *next += 1;
        let id = *next;
        self.callbacks.lock().unwrap().push((id, Arc::new(callback)));
        id
    }

    pub fn unobserve(&self, id: usize) -> bool {
        let mut callbacks = self.callbacks.lock().unwrap();
        let before = callbacks.len();
        callbacks.retain(|(sub_id, _)| *sub_id != id);
        callbacks.len() != before
    }

    /* ----------------------------- internal ------------------------- */

    fn load_ir(&self, ir: &DocIR) {
        match ir {
            DocIR::Word { blocks } => {
                for (i, block) in blocks.iter().enumerate() {
                    let after = if i == 0 {
                        None
                    } else {
                        Some(blocks[i - 1].id.as_str())
                    };
                    let _ = self.insert_block_with_id(after, block);
                }
            }
            DocIR::Ppt { slides } => {
                let mut txn = self.doc.transact_mut_with(origin_local());
                let root = txn.get_or_insert_array("ppt");
                for slide in slides {
                    let elements: Vec<In> = slide
                        .elements
                        .iter()
                        .map(|el| {
                            In::from(MapPrelim::from_iter([
                                ("id", In::from(Any::String(el.id.clone().into()))),
                                ("type", In::from(Any::String(element_type_str(el.element_type).into()))),
                                ("geo", In::from(geo_prelim(&el.geo))),
                                ("props", In::from(props_prelim(&el.props))),
                            ]))
                        })
                        .collect();
                    let slide_map = MapPrelim::from_iter([
                        ("id", In::from(Any::String(slide.id.clone().into()))),
                        ("layout", In::from(Any::String(slide.layout.clone().into()))),
                        ("elements", In::from(ArrayPrelim::from(elements))),
                    ]);
                    let at = root.len(&txn);
                    root.insert(&mut txn, at, slide_map);
                }
            }
        }
    }

    fn insert_block_with_id(&self, after_id: Option<&str>, block: &WordBlock) -> Result<()> {
        let mut txn = self.doc.transact_mut_with(origin_local());
        let root = txn.get_or_insert_xml_fragment("word");
        let index = match after_id {
            None => 0,
            Some(after) => find_block_index(&txn, &root, after).map(|i| i + 1).unwrap_or(0),
        };
        let tag = match block.block_type {
            WordBlockType::Heading => "heading",
            WordBlockType::Paragraph => "paragraph",
        };
        let el = root.insert(&mut txn, index as u32, XmlElementPrelim::empty(tag));
        el.insert_attribute(&mut txn, "id", block.id.clone());
        if block.block_type == WordBlockType::Heading {
            el.insert_attribute(&mut txn, "level", block.level.unwrap_or(1).to_string());
        }
        if let Some(style) = &block.style {
            el.insert_attribute(&mut txn, "style", style.clone());
        }
        let text = el.insert(&mut txn, 0, XmlTextPrelim::new(""));
        for run in &block.runs {
            if run.text.is_empty() {
                continue;
            }
            let at = text.len(&txn);
            let attrs = run_attrs(run);
            if attrs.is_empty() {
                text.insert(&mut txn, at, &run.text);
            } else {
                text.insert_with_attributes(&mut txn, at, &run.text, attrs);
            }
        }
        Ok(())
    }

    fn emit(&self, origin: DocChangeOrigin) {
        let event = DocChangeEvent {
            origin,
            hash: self.hash(),
        };
        let callbacks = self.callbacks.lock().unwrap().clone();
        for (_, callback) in callbacks {
            callback(event.clone());
        }
    }

    fn assert_type(&self, expected: DocType) -> Result<()> {
        if self.doc_type == expected {
            Ok(())
        } else {
            Err(DocCoreError::WrongType {
                expected: expected.as_str(),
                actual: self.doc_type,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PartialGeo {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub rot: Option<f64>,
}

impl From<Geo> for PartialGeo {
    fn from(value: Geo) -> Self {
        Self {
            x: Some(value.x),
            y: Some(value.y),
            w: Some(value.w),
            h: Some(value.h),
            rot: value.rot,
        }
    }
}

/* ---------------------------- read helpers --------------------------- */

fn read_word<T: ReadTxn>(txn: &T) -> Vec<WordBlock> {
    let Some(root) = txn.get_xml_fragment("word") else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    let len = root.len(txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = root.get(txn, i) {
            let id = el.get_attribute(txn, "id").unwrap_or_default();
            let block_type = if el.tag().as_ref() == "heading" {
                WordBlockType::Heading
            } else {
                WordBlockType::Paragraph
            };
            let level = if block_type == WordBlockType::Heading {
                Some(
                    el.get_attribute(txn, "level")
                        .and_then(|s| s.parse::<u8>().ok())
                        .unwrap_or(1),
                )
            } else {
                None
            };
            let style = el.get_attribute(txn, "style");
            let mut runs = Vec::new();
            let clen = el.len(txn);
            for j in 0..clen {
                if let Some(XmlOut::Text(t)) = el.get(txn, j) {
                    runs.extend(read_runs(txn, &t));
                }
            }
            blocks.push(WordBlock {
                id,
                block_type,
                level,
                style,
                runs,
            });
        }
    }
    blocks
}

fn read_runs<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> Vec<WordTextRun> {
    let mut runs = Vec::new();
    for diff in text.diff(txn, |_| ()) {
        let Out::Any(Any::String(s)) = diff.insert else {
            continue;
        };
        let mut marks = Vec::new();
        let mut style = None;
        if let Some(attrs) = diff.attributes {
            for mark in MARK_KEYS.iter() {
                if let Some(v) = attrs.get(mark_key(mark)) {
                    if is_truthy(v) {
                        marks.push(mark.clone());
                    }
                }
            }
            if let Some(Any::String(st)) = attrs.get("style") {
                style = Some(st.to_string());
            }
        }
        runs.push(WordTextRun {
            text: s.to_string(),
            marks,
            style,
        });
    }
    runs
}

fn read_ppt<T: ReadTxn>(txn: &T) -> Vec<Slide> {
    let Some(root) = txn.get_array("ppt") else {
        return Vec::new();
    };
    let mut slides = Vec::new();
    let len = root.len(txn);
    for i in 0..len {
        if let Some(Out::YMap(slide_map)) = root.get(txn, i) {
            slides.push(read_slide(txn, &slide_map));
        }
    }
    slides
}

fn read_slide<T: ReadTxn>(txn: &T, m: &MapRef) -> Slide {
    let id = map_string(txn, m, "id").unwrap_or_default();
    let layout = map_string(txn, m, "layout").unwrap_or_else(|| "blank".to_string());
    let mut elements = Vec::new();
    if let Some(Out::YArray(arr)) = m.get(txn, "elements") {
        let len = arr.len(txn);
        for i in 0..len {
            if let Some(Out::YMap(el_map)) = arr.get(txn, i) {
                elements.push(read_element(txn, &el_map));
            }
        }
    }
    Slide {
        id,
        layout,
        elements,
    }
}

fn read_element<T: ReadTxn>(txn: &T, m: &MapRef) -> SlideElement {
    let id = map_string(txn, m, "id").unwrap_or_default();
    let element_type = match map_string(txn, m, "type").as_deref() {
        Some("shape") => ElementType::Shape,
        Some("image") => ElementType::Image,
        _ => ElementType::Text,
    };
    let geo = match m.get(txn, "geo") {
        Some(Out::YMap(geo_map)) => read_geo(txn, &geo_map),
        _ => Geo {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            rot: None,
        },
    };
    let props = match m.get(txn, "props") {
        Some(Out::YMap(props_map)) => read_props(txn, &props_map),
        _ => JsonMap::new(),
    };
    SlideElement {
        id,
        element_type,
        geo,
        props,
    }
}

fn read_geo<T: ReadTxn>(txn: &T, m: &MapRef) -> Geo {
    Geo {
        x: map_number(txn, m, "x").unwrap_or(0.0),
        y: map_number(txn, m, "y").unwrap_or(0.0),
        w: map_number(txn, m, "w").unwrap_or(1.0),
        h: map_number(txn, m, "h").unwrap_or(1.0),
        rot: map_number(txn, m, "rot"),
    }
}

fn read_props<T: ReadTxn>(txn: &T, m: &MapRef) -> JsonMap<String, Value> {
    let mut props = JsonMap::new();
    for (key, value) in m.iter(txn) {
        props.insert(key.to_string(), out_to_value(txn, &value));
    }
    props
}

fn out_to_value<T: ReadTxn>(txn: &T, out: &Out) -> Value {
    match out {
        Out::Any(any) => any_to_value(any),
        Out::YMap(m) => {
            let mut obj = JsonMap::new();
            for (k, v) in m.iter(txn) {
                obj.insert(k.to_string(), out_to_value(txn, &v));
            }
            Value::Object(obj)
        }
        Out::YArray(a) => {
            let mut items = Vec::new();
            let len = a.len(txn);
            for i in 0..len {
                if let Some(v) = a.get(txn, i) {
                    items.push(out_to_value(txn, &v));
                }
            }
            Value::Array(items)
        }
        Out::YText(t) => Value::String(t.get_string(txn)),
        Out::YXmlText(t) => Value::String(t.get_string(txn)),
        _ => Value::Null,
    }
}

fn any_to_value(any: &Any) -> Value {
    match any {
        Any::Null | Any::Undefined => Value::Null,
        Any::Bool(b) => Value::Bool(*b),
        Any::Number(n) => number_value(*n),
        Any::BigInt(i) => Value::from(*i),
        Any::String(s) => Value::String(s.to_string()),
        Any::Buffer(b) => Value::Array(b.iter().map(|x| Value::from(*x)).collect()),
        Any::Array(items) => Value::Array(items.iter().map(any_to_value).collect()),
        Any::Map(map) => {
            let mut obj = JsonMap::new();
            for (k, v) in map.iter() {
                obj.insert(k.clone(), any_to_value(v));
            }
            Value::Object(obj)
        }
    }
}

fn number_value(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)
    }
}

fn value_to_any(value: &Value) -> Any {
    match value {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => Any::Number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Any::String(s.as_str().into()),
        Value::Array(items) => Any::Array(items.iter().map(value_to_any).collect::<Vec<_>>().into()),
        Value::Object(obj) => Any::Map(Arc::new(
            obj.iter()
                .map(|(k, v)| (k.clone(), value_to_any(v)))
                .collect(),
        )),
    }
}

fn map_string<T: ReadTxn>(txn: &T, m: &MapRef, key: &str) -> Option<String> {
    match m.get(txn, key) {
        Some(Out::Any(Any::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn map_number<T: ReadTxn>(txn: &T, m: &MapRef, key: &str) -> Option<f64> {
    match m.get(txn, key) {
        Some(Out::Any(Any::Number(n))) => Some(n),
        Some(Out::Any(Any::BigInt(i))) => Some(i as f64),
        _ => None,
    }
}

fn is_truthy(any: &Any) -> bool {
    !matches!(any, Any::Null | Any::Undefined | Any::Bool(false))
}

/* --------------------------- write helpers --------------------------- */

fn element_type_str(t: ElementType) -> &'static str {
    match t {
        ElementType::Text => "text",
        ElementType::Shape => "shape",
        ElementType::Image => "image",
    }
}

fn run_attrs(run: &WordTextRun) -> Attrs {
    let mut attrs = Attrs::new();
    for mark in &run.marks {
        attrs.insert(mark_key(mark).into(), Any::Bool(true));
    }
    if let Some(style) = &run.style {
        attrs.insert("style".into(), Any::String(style.as_str().into()));
    }
    attrs
}

fn style_attrs(style: &StyleInput) -> Attrs {
    let mut attrs = Attrs::new();
    let mut put = |key: &str, value: Option<bool>| {
        if let Some(v) = value {
            attrs.insert(key.into(), if v { Any::Bool(true) } else { Any::Null });
        }
    };
    put("bold", style.bold);
    put("italic", style.italic);
    put("underline", style.underline);
    put("code", style.code);
    put("strike", style.strike);
    if let Some(named) = &style.style {
        attrs.insert("style".into(), Any::String(named.as_str().into()));
    }
    attrs
}

fn geo_prelim(geo: &Geo) -> MapPrelim {
    let mut entries: Vec<(&str, In)> = vec![
        ("x", In::from(Any::Number(geo.x))),
        ("y", In::from(Any::Number(geo.y))),
        ("w", In::from(Any::Number(geo.w))),
        ("h", In::from(Any::Number(geo.h))),
    ];
    if let Some(rot) = geo.rot {
        entries.push(("rot", In::from(Any::Number(rot))));
    }
    MapPrelim::from_iter(entries)
}

fn props_prelim(props: &JsonMap<String, Value>) -> MapPrelim {
    MapPrelim::from_iter(
        props
            .iter()
            .map(|(k, v)| (k.clone(), In::from(value_to_any(v)))),
    )
}

fn find_block_index<T: ReadTxn>(txn: &T, root: &XmlFragmentRef, id: &str) -> Option<usize> {
    let len = root.len(txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = root.get(txn, i) {
            if el.get_attribute(txn, "id").as_deref() == Some(id) {
                return Some(i as usize);
            }
        }
    }
    None
}

fn get_block_text<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> Option<XmlTextRef> {
    let len = el.len(txn);
    for i in 0..len {
        if let Some(XmlOut::Text(t)) = el.get(txn, i) {
            return Some(t);
        }
    }
    None
}

fn get_or_create_block_text(txn: &mut yrs::TransactionMut, el: &yrs::XmlElementRef) -> XmlTextRef {
    if let Some(existing) = get_block_text(txn, el) {
        return existing;
    }
    let len = el.len(txn);
    el.insert(txn, len, XmlTextPrelim::new(""))
}

fn locate_element_child<T: ReadTxn>(
    txn: &T,
    root: &yrs::ArrayRef,
    slide_id: &str,
    el_id: &str,
    child: &'static str,
    op: &'static str,
) -> Result<MapRef> {
    let slide_idx = find_slide_index(txn, root, slide_id).ok_or_else(|| DocCoreError::NotFound {
        op,
        id: slide_id.to_string(),
    })?;
    let Some(Out::YMap(slide_map)) = root.get(txn, slide_idx) else {
        return Err(DocCoreError::NotFound {
            op,
            id: slide_id.to_string(),
        });
    };
    let Some(Out::YArray(elements)) = slide_map.get(txn, "elements") else {
        return Err(DocCoreError::NotFound {
            op,
            id: el_id.to_string(),
        });
    };
    let len = elements.len(txn);
    for i in 0..len {
        if let Some(Out::YMap(el_map)) = elements.get(txn, i) {
            if map_string(txn, &el_map, "id").as_deref() == Some(el_id) {
                if let Some(Out::YMap(child_map)) = el_map.get(txn, child) {
                    return Ok(child_map);
                }
            }
        }
    }
    Err(DocCoreError::NotFound {
        op,
        id: el_id.to_string(),
    })
}

fn find_slide_index<T: ReadTxn>(txn: &T, root: &yrs::ArrayRef, id: &str) -> Option<u32> {
    let len = root.len(txn);
    for i in 0..len {
        if let Some(Out::YMap(slide_map)) = root.get(txn, i) {
            if map_string(txn, &slide_map, "id").as_deref() == Some(id) {
                return Some(i);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementType, WordBlockType};
    use moonlit_core::SeqIdFactory;

    fn word_core() -> DocCore {
        DocCore::new(DocCoreOptions {
            doc_type: DocType::Word,
            id_factory: Arc::new(SeqIdFactory::new()),
            exporter: None,
        })
    }

    fn ppt_core() -> DocCore {
        DocCore::new(DocCoreOptions {
            doc_type: DocType::Ppt,
            id_factory: Arc::new(SeqIdFactory::new()),
            exporter: None,
        })
    }

    #[test]
    fn word_insert_replace_style_outline() {
        let core = word_core();
        let id = core
            .insert_block(
                None,
                NewWordBlock {
                    block_type: WordBlockType::Heading,
                    level: Some(2),
                    style: None,
                    text: Some("Hello".to_string()),
                    runs: None,
                },
            )
            .unwrap();
        assert_eq!(id, "blk_1");
        core.replace_text(&id, "World").unwrap();
        core.apply_style(
            &id,
            StyleInput {
                bold: Some(true),
                style: Some("accent".to_string()),
                ..StyleInput::default()
            },
        )
        .unwrap();
        let outline = core.get_outline();
        assert_eq!(outline[0].text, "World");
        let DocIR::Word { blocks } = core.read_document() else {
            panic!("word")
        };
        assert_eq!(blocks[0].runs[0].marks, vec![WordMark::Bold]);
        assert_eq!(blocks[0].style.as_deref(), Some("accent"));
    }

    #[test]
    fn ppt_add_slide_seeds_and_edit() {
        let core = ppt_core();
        let slide_id = core.add_slide(0, "title").unwrap();
        let DocIR::Ppt { slides } = core.read_document() else {
            panic!("ppt")
        };
        assert_eq!(slides.len(), 1);
        assert_eq!(slides[0].elements.len(), 2);
        assert_eq!(slides[0].elements[0].element_type, ElementType::Text);
        let el_id = slides[0].elements[0].id.clone();
        drop(slides);
        core.edit_element(
            &slide_id,
            &el_id,
            serde_json::Map::from_iter([("text".to_string(), Value::String("新标题".to_string()))]),
        )
        .unwrap();
        let outline = core.get_outline();
        assert_eq!(outline[0].text, "新标题");
    }

    #[test]
    fn observers_receive_hash() {
        let core = word_core();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        core.observe(move |evt| sink.lock().unwrap().push(evt));
        core.insert_block(
            None,
            NewWordBlock {
                block_type: WordBlockType::Paragraph,
                level: None,
                style: None,
                text: Some("x".to_string()),
                runs: None,
            },
        )
        .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(seen.lock().unwrap()[0].origin, DocChangeOrigin::Local);
        assert_eq!(seen.lock().unwrap()[0].hash.len(), 16);
    }

    #[test]
    fn updates_round_trip_between_docs() {
        let a = word_core();
        a.insert_block(
            None,
            NewWordBlock {
                block_type: WordBlockType::Paragraph,
                level: None,
                style: None,
                text: Some("sync me".to_string()),
                runs: None,
            },
        )
        .unwrap();
        let update = a.encode_state_as_update();
        let b = word_core();
        b.apply_update(&update).unwrap();
        assert_eq!(a.hash(), b.hash());
        let DocIR::Word { blocks } = b.read_document() else {
            panic!("word")
        };
        assert_eq!(blocks[0].runs[0].text, "sync me");
    }
}
