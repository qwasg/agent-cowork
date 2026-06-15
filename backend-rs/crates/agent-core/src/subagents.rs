//! Subagent profiles: builtin personas plus disk-defined profiles loaded
//! from `data/agents/*.md` (Claude-Code-agents style, hot-reloaded).
//!
//! Disk profile format — YAML-ish front matter + system prompt body:
//!
//! ```markdown
//! ---
//! name: db-migrator
//! description: 数据库迁移专家
//! tools: read_file, grep, run_command
//! model: gpt-4o-mini
//! maxSteps: 12
//! ---
//! # 任务方式
//! （system prompt 正文…）
//! ```
//!
//! Profiles never include `task` itself, which prevents recursive delegation.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct SubagentProfile {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    /// Optional model override for this profile's runs.
    pub model: Option<String>,
    /// Optional per-run tool-loop step cap (resource boundary).
    pub max_steps: Option<usize>,
    pub builtin: bool,
}

/// Full tool surface a read-write subagent may use. Must stay a subset of the
/// names registered in `ToolRegistry::build` (`task` itself is always
/// excluded to prevent recursive delegation).
const DEFAULT_WRITE_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "read_skill",
    "web_search",
    "web_fetch",
    "write_file",
    "create_document",
    "delete_file",
    "str_replace_edit",
    "apply_patch",
    "run_command",
];

const READONLY_CODE_TOOLS: &[&str] = &["read_file", "list_dir", "grep"];

fn profile(name: &str, description: &str, system_prompt: &str, tools: &[&str]) -> SubagentProfile {
    SubagentProfile {
        name: name.to_string(),
        description: description.to_string(),
        system_prompt: system_prompt.to_string(),
        allowed_tools: tools.iter().map(|s| s.to_string()).collect(),
        model: None,
        max_steps: None,
        builtin: true,
    }
}

pub fn builtin_profiles() -> Vec<SubagentProfile> {
    vec![
        profile(
            "explorer",
            "只读探索代码库：定位文件、梳理结构、回答“在哪里/怎么实现”。",
            "\
# 任务方式\n\
你专精于在陌生代码库中快速定位信息。工作方法：\n\
- 先 list_dir 把握目录结构，再用 grep 按关键词（符号名、报错文本、路由路径等）缩小范围，最后 read_file 精读关键文件；\n\
- 沿调用链追踪：找到定义后再查它的调用方/被调方，直到能回答问题为止；\n\
- 绝不修改任何文件。\n\
# 产出格式\n\
- 按要点列出发现，每条附文件路径（必要时带行号）；\n\
- 明确区分“确认的事实”与“推测”；没找到就直说没找到，不要编造。",
            READONLY_CODE_TOOLS,
        ),
        profile(
            "researcher",
            "资料调研：调用联网搜索与网页抓取工具，汇总要点与权衡。",
            "\
# 任务方式\n\
你专精于联网调研。工作方法：\n\
- 用 web_search 检索（必要时换不同关键词多查几轮），对最有价值的结果用 web_fetch 读取原文；\n\
- 交叉验证：重要结论至少有两个独立来源支撑，注意信息的时效性；\n\
- 不修改工作区。\n\
# 产出格式\n\
- 输出要点、方案对比与建议，每个关键结论标注来源 URL；\n\
- 区分事实与观点，指出尚存争议或证据不足的部分。",
            &["read_file", "grep", "web_search", "web_fetch"],
        ),
        profile(
            "code-reviewer",
            "代码评审：审查改动，指出风险、缺陷与改进点。",
            "\
# 任务方式\n\
你专精于代码评审。工作方法：\n\
- 通读指定的改动或文件，必要时 grep / read_file 查看调用方与相关上下文，确认改动的真实影响面；\n\
- 重点检查：正确性（边界条件、错误处理、并发）、安全（注入、越权、敏感信息）、性能、可维护性；\n\
- 只评审，不修改任何文件。\n\
# 产出格式\n\
- 按严重程度分组列出问题（严重 / 建议 / 可选），每条附文件路径、问题描述与具体修改建议；\n\
- 没有问题就明确说明检查了哪些方面、为何认为可以通过。",
            READONLY_CODE_TOOLS,
        ),
        profile(
            "general",
            "通用子代理：可读写工作区，完成探索、编辑、调研等综合子任务。",
            "\
# 任务方式\n\
你是可读写工作区的通用执行者。工作方法：\n\
- 先用只读工具核实现状，再做必要的修改；遵循目标文件既有风格，做最小变更；\n\
- 修改后尽量用 run_command 验证（编译 / 测试 / 运行脚本）。\n\
# 产出格式\n\
- 汇报做了什么改动（文件路径 + 要点）、验证结果、遗留问题。",
            DEFAULT_WRITE_TOOLS,
        ),
    ]
}

// ---------------------------------------------------------------- registry

/// Builtin + disk profiles. Disk profiles live in `data/agents/*.md` and are
/// re-scanned on every access (the directory is tiny), so edits take effect
/// without a restart. A disk profile with a builtin's name overrides it.
pub struct SubagentRegistry {
    agents_dir: PathBuf,
}

impl SubagentRegistry {
    pub fn new(agents_dir: PathBuf) -> Self {
        SubagentRegistry { agents_dir }
    }

    pub fn all(&self) -> Vec<SubagentProfile> {
        let mut profiles = builtin_profiles();
        for disk in load_disk_profiles(&self.agents_dir) {
            match profiles.iter_mut().find(|p| p.name == disk.name) {
                Some(slot) => *slot = disk,
                None => profiles.push(disk),
            }
        }
        profiles
    }

    pub fn get(&self, name: &str) -> Option<SubagentProfile> {
        self.all().into_iter().find(|p| p.name == name)
    }

    /// `GET /subagents` payload entries.
    pub fn as_dicts(&self) -> Vec<Value> {
        self.all()
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "description": p.description,
                    "allowedTools": p.allowed_tools,
                    "builtin": p.builtin,
                    "model": p.model,
                    "maxSteps": p.max_steps,
                })
            })
            .collect()
    }

    /// System-prompt blurb listing delegable subagents.
    pub fn render_prompt(&self) -> String {
        let mut lines = vec!["可委派的子代理类型：".to_string()];
        for p in self.all() {
            lines.push(format!("- {}: {}", p.name, p.description));
        }
        lines.join("\n")
    }

    /// Profile names for the `task` tool's `subagent_type` enum.
    pub fn type_names(&self) -> Vec<String> {
        self.all().into_iter().map(|p| p.name).collect()
    }
}

fn load_disk_profiles(dir: &Path) -> Vec<SubagentProfile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut profiles: Vec<SubagentProfile> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .filter_map(|e| {
            let content = std::fs::read_to_string(e.path()).ok()?;
            let fallback = e
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("agent")
                .to_string();
            parse_profile_md(&fallback, &content)
        })
        .collect();
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

/// Parse a `---` front-matter profile file; returns None when there is no
/// usable front matter or the resulting profile would be empty/dangerous.
fn parse_profile_md(fallback_name: &str, content: &str) -> Option<SubagentProfile> {
    let text = content.trim_start_matches('\u{feff}').trim_start();
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let front = &rest[..end];
    let body = rest[end + 4..].trim().to_string();
    if body.is_empty() {
        return None;
    }

    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut tools: Vec<String> = Vec::new();
    let mut model: Option<String> = None;
    let mut max_steps: Option<usize> = None;
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "name" if !value.is_empty() => name = value.to_string(),
            "description" => description = value.to_string(),
            "tools" => {
                tools = value
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty() && t != "task")
                    .collect();
            }
            "model" if !value.is_empty() => model = Some(value.to_string()),
            "maxsteps" | "max_steps" => max_steps = value.parse().ok(),
            _ => {}
        }
    }
    if tools.is_empty() {
        tools = READONLY_CODE_TOOLS.iter().map(|s| s.to_string()).collect();
    }
    Some(SubagentProfile {
        name,
        description,
        system_prompt: body,
        allowed_tools: tools,
        model,
        max_steps,
        builtin: false,
    })
}

/// Builtin-only lookup (used by tests and as a registry-free fallback).
pub fn get_builtin(name: &str) -> Option<SubagentProfile> {
    builtin_profiles().into_iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_lookup() {
        assert!(get_builtin("explorer").is_some());
        assert!(get_builtin("nope").is_none());
        // No profile may delegate further.
        for p in builtin_profiles() {
            assert!(!p.allowed_tools.iter().any(|t| t == "task"));
        }
    }

    #[test]
    fn registry_merges_disk_profiles() {
        let dir =
            std::env::temp_dir().join(format!("agentd_agents_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let reg = SubagentRegistry::new(dir.clone());
        assert_eq!(reg.all().len(), 4);

        // New disk profile is added…
        std::fs::write(
            dir.join("db-migrator.md"),
            "---\nname: db-migrator\ndescription: 数据库迁移\ntools: read_file, run_command, task\nmaxSteps: 12\n---\n只做数据库迁移。",
        )
        .unwrap();
        // …and a disk profile overrides the builtin with the same name.
        std::fs::write(
            dir.join("explorer.md"),
            "---\nname: explorer\ndescription: 自定义探索者\n---\n自定义探索 prompt。",
        )
        .unwrap();
        // Files without front matter are ignored.
        std::fs::write(dir.join("notes.md"), "随便写的笔记").unwrap();

        let all = reg.all();
        assert_eq!(all.len(), 5);
        let db = reg.get("db-migrator").unwrap();
        assert!(!db.builtin);
        assert_eq!(db.max_steps, Some(12));
        // `task` is stripped from disk tool lists.
        assert!(!db.allowed_tools.iter().any(|t| t == "task"));
        let explorer = reg.get("explorer").unwrap();
        assert_eq!(explorer.description, "自定义探索者");
        assert!(!explorer.builtin);
        // Hot reload: deleting the override restores the builtin.
        std::fs::remove_file(dir.join("explorer.md")).unwrap();
        assert!(reg.get("explorer").unwrap().builtin);
    }
}
