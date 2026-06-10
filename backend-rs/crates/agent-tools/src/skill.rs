//! Skill discovery tool (port of `skill_tools.py` / `skills_discovery.py`).
//! Reads `SKILL.md` content from configured skill directories.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::contracts::{ApiError, ApiResult};
use crate::tools::{AgentTool, ToolContext};

pub struct ReadSkill;

#[async_trait]
impl AgentTool for ReadSkill {
    fn name(&self) -> &str {
        "read_skill"
    }
    fn description(&self) -> &str {
        "Read the full SKILL.md content of a named skill."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "name": {"type": "string", "description": "skill folder name"} },
            "required": ["name"]
        })
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> ApiResult<String> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !is_safe_skill_name(&name) {
            return Err(ApiError::new(
                "TOOL_INVALID_ARGS",
                "name must be a plain skill folder name",
            ));
        }
        for dir in &ctx.skill_dirs {
            let candidate = dir.join(&name).join("SKILL.md");
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                return Ok(content);
            }
        }
        Err(ApiError::new(
            "SKILL_NOT_FOUND",
            format!("skill not found: {name}"),
        ))
    }
}

/// A skill name must be a single plain folder name — no separators, drive
/// letters or `..` segments that could escape the skill directories.
pub fn is_safe_skill_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..")
        && name != "."
}

/// Scan skill directories for available skills (name + first description line).
pub fn discover_skills(skill_dirs: &[std::path::PathBuf]) -> Vec<Value> {
    let mut out = Vec::new();
    for dir in skill_dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let skill_md = entry.path().join("SKILL.md");
            if let Ok(content) = std::fs::read_to_string(&skill_md) {
                let desc = content
                    .lines()
                    .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>();
                out.push(json!({ "name": name, "description": desc }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_name_validation() {
        assert!(is_safe_skill_name("essay-grading"));
        assert!(is_safe_skill_name("web_design"));
        assert!(!is_safe_skill_name(""));
        assert!(!is_safe_skill_name(".."));
        assert!(!is_safe_skill_name("../../etc"));
        assert!(!is_safe_skill_name("a/b"));
        assert!(!is_safe_skill_name("a\\b"));
        assert!(!is_safe_skill_name("C:evil"));
    }
}
