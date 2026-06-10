use crate::{DocIR, PptDocIR, Result, WordDocIR};

pub fn to_json(ir: &DocIR) -> Result<String> {
    Ok(serde_json::to_string_pretty(ir)?)
}

pub fn from_json(json: &str) -> Result<DocIR> {
    Ok(serde_json::from_str(json)?)
}

pub fn word_from_json(json: &str) -> Result<WordDocIR> {
    Ok(serde_json::from_str(json)?)
}

pub fn ppt_from_json(json: &str) -> Result<PptDocIR> {
    Ok(serde_json::from_str(json)?)
}
