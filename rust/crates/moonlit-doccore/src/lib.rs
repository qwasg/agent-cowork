//! `moonlit-doccore` — Rust port of DocForge's `doc-core` contract.
//!
//! It provides the document IR, layout seeds, stable serialization helpers and
//! the `DocCore` mutation/observe/export API consumed by UI, agent logic,
//! compile and preview code.

mod doc_core;
pub mod layouts;
pub mod serialize;
mod types;

pub use doc_core::{DocCore, DocCoreOptions, PartialGeo};
pub use layouts::{get_layout_seeds, SeedElement, SLIDE_HEIGHT, SLIDE_WIDTH};
pub use serialize::{from_json, ppt_from_json, to_json, word_from_json};
pub use types::*;
