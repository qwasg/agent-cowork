//! Skill discovery tool (port of `skill_tools.py` / `skills_discovery.py`).
//! Reads `SKILL.md` content from configured skill directories.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{AgentTool, ToolContext};
use agent_protocol::{ApiError, ApiResult};

pub struct ReadSkill;

#[async_trait]
impl AgentTool for ReadSkill {
    fn name(&self) -> &str {
        "read_skill"
    }
    fn read_only(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "读取一个技能（skill）的完整 SKILL.md 说明。系统提示词的“可用技能”列表给出了技能名与摘要；\
         当任务与某个技能匹配时，先用本工具读取全文，再严格按其中的指引执行。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "name": {"type": "string", "description": "技能文件夹名（如 essay-grading），不含路径"} },
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
    !name.is_empty() && !name.contains(['/', '\\', ':']) && !name.contains("..") && name != "."
}

/// Standard workspace skill roots, in precedence order.
pub fn workspace_skill_dirs(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("skills"),
        root.join(".cursor").join("skills"),
        root.join(".codex").join("skills"),
        root.join(".claude").join("skills"),
    ]
}

/// Standard user-level skill roots, in precedence order.
pub fn default_user_skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        push_unique_path(&mut dirs, PathBuf::from(codex_home).join("skills"));
    }
    if let Some(home) = home_dir() {
        for dir in [
            home.join(".cursor").join("skills"),
            home.join(".codex").join("skills"),
            home.join(".claude").join("skills"),
        ] {
            push_unique_path(&mut dirs, dir);
        }
    }
    dirs
}

/// Build the complete skill search path. Earlier roots win on duplicate names.
pub fn configured_skill_dirs(workspace_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen_roots = HashSet::new();
    for root in workspace_roots {
        let root_key = path_key(root);
        if !seen_roots.insert(root_key) {
            continue;
        }
        for dir in workspace_skill_dirs(root) {
            push_unique_path(&mut dirs, dir);
        }
    }
    for dir in default_user_skill_dirs() {
        push_unique_path(&mut dirs, dir);
    }
    dirs
}

/// Scan skill directories for available skills. The first occurrence of a skill
/// name wins, so workspace skills shadow user-level skills with the same name.
pub fn discover_skills(skill_dirs: &[PathBuf]) -> Vec<Value> {
    let user_dir_keys: HashSet<String> = default_user_skill_dirs()
        .iter()
        .map(|dir| path_key(dir))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in skill_dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        let scope = if user_dir_keys.contains(&path_key(dir)) {
            "user"
        } else {
            "workspace"
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        for entry in entries {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_safe_skill_name(&name) || seen.contains(&name) {
                continue;
            }
            let skill_md = entry.path().join("SKILL.md");
            if let Ok(content) = std::fs::read_to_string(&skill_md) {
                let summary = read_summary(&content);
                if summary.is_empty() {
                    continue;
                }
                seen.insert(name.clone());
                out.push(json!({
                    "name": name,
                    "summary": summary,
                    "description": summary,
                    "scope": scope,
                    "path": skill_md.to_string_lossy(),
                }));
            }
        }
    }
    dedupe_skill_records(out)
}

/// Keep the first record per skill name (defensive; also used by list endpoints).
pub fn dedupe_skill_records(items: Vec<Value>) -> Vec<Value> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            item.get("name")
                .and_then(|v| v.as_str())
                .map(|name| seen.insert(name.to_string()))
                .unwrap_or(false)
        })
        .collect()
}

fn read_summary(content: &str) -> String {
    const SUMMARY_CHAR_CAP: usize = 160;

    let lines: Vec<&str> = content.lines().collect();
    let mut body_start = 0;
    if lines.first().map(|line| line.trim()) == Some("---") {
        for (idx, line) in lines.iter().enumerate().skip(1) {
            let trimmed = line.trim();
            if let Some(desc) = trimmed.strip_prefix("description:") {
                let desc = desc.trim().trim_matches('"').trim_matches('\'');
                if !desc.is_empty() && !is_invalid_summary(desc) {
                    return cap_chars(desc, SUMMARY_CHAR_CAP);
                }
            }
            if trimmed == "---" {
                body_start = idx + 1;
                break;
            }
        }
    }

    let body = lines[body_start..].join("\n");
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }

    let first_line = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if first_line.trim_start().starts_with('#') {
        for chunk in body.split("\n\n") {
            let paragraph = chunk.trim();
            if !paragraph.is_empty() && !paragraph.starts_with('#') {
                return cap_chars(&paragraph.replace('\n', " "), SUMMARY_CHAR_CAP);
            }
        }
        return cap_chars(first_line.trim_start_matches('#').trim(), SUMMARY_CHAR_CAP);
    }
    let summary = cap_chars(first_line.trim(), SUMMARY_CHAR_CAP);
    if is_invalid_summary(&summary) {
        String::new()
    } else {
        summary
    }
}

fn is_invalid_summary(summary: &str) -> bool {
    let trimmed = summary.trim();
    trimmed.is_empty() || trimmed == "---" || trimmed == "..."
}

fn cap_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path_key(&path);
    if !paths.iter().any(|existing| path_key(existing) == key) {
        paths.push(path);
    }
}

fn path_key(path: &Path) -> String {
    let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
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

    #[test]
    fn discover_skills_dedupes_and_returns_frontend_shape() {
        let root =
            std::env::temp_dir().join(format!("agentd-skill-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let primary = root.join("skills");
        let secondary = root.join(".cursor").join("skills");
        std::fs::create_dir_all(primary.join("demo")).unwrap();
        std::fs::create_dir_all(secondary.join("demo")).unwrap();
        std::fs::create_dir_all(secondary.join("frontmatter")).unwrap();
        std::fs::write(
            primary.join("demo").join("SKILL.md"),
            "# Demo\n\nUse this skill from the primary root.",
        )
        .unwrap();
        std::fs::write(
            secondary.join("demo").join("SKILL.md"),
            "# Demo\n\nThis duplicate should not win.",
        )
        .unwrap();
        std::fs::write(
            secondary.join("frontmatter").join("SKILL.md"),
            "---\nname: frontmatter\ndescription: \"Read YAML summaries\"\n---\n\n# Body",
        )
        .unwrap();

        let items = discover_skills(&workspace_skill_dirs(&root));

        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "demo");
        assert_eq!(items[0]["summary"], "Use this skill from the primary root.");
        assert_eq!(items[0]["description"], items[0]["summary"]);
        assert_eq!(items[0]["scope"], "workspace");
        assert!(items[0]["path"].as_str().unwrap().ends_with("SKILL.md"));
        assert_eq!(items[1]["name"], "frontmatter");
        assert_eq!(items[1]["summary"], "Read YAML summaries");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discover_skills_dedupes_duplicate_workspace_roots() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..");
        let dirs = configured_skill_dirs(&[root.clone(), root]);
        let items = discover_skills(&dirs);
        let names: Vec<&str> = items
            .iter()
            .filter_map(|v| v.get("name").and_then(|n| n.as_str()))
            .collect();
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "duplicate skill names: {names:?}"
        );
    }
}
