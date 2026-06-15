//! Layered system-prompt assembly for the agent runtime.
//!
//! Structure:
//!
//! ```text
//! <system_static>   persona (per agent kind) + mode overlay + tool principles
//!                   + conventions + workflow + memory/subagent guidance + tools
//! </system_static>
//! <system_dynamic>  environment + AGENT.md + project rules + relevant memories
//!                   + editor context + skills
//! </system_dynamic>
//! ```
//!
//! The persona / conventions / workflow vary by [`AgentKind`] so the platform
//! ships three distinct agents (general / document / coding) on one runtime.
//! The static block is cache-friendly; everything volatile lives in the
//! dynamic block so providers with prefix caching keep their hit rate.

pub mod coding;
pub mod document;
pub mod general;
pub mod shared;

use std::path::Path;

use serde_json::Value;

use crate::profile::AgentKind;

/// Caps applied to injected dynamic context so a single oversized file can't
/// blow the token budget before the conversation even starts.
const AGENT_MD_MAX_CHARS: usize = 6_000;
const CONTEXT_WINDOW_MAX_CHARS: usize = 4_000;
const MAX_SKILL_ITEMS: usize = 24;
const MAX_MEMORY_ITEMS: usize = 10;

/// Everything the prompt builder may know about the current turn. All dynamic
/// fields are optional: absent data simply omits its section.
#[derive(Default)]
pub struct PromptContext<'a> {
    /// Which agent profile this session runs as.
    pub kind: AgentKind,
    /// Composer mode: build / debug / ask / plan / multitask (already clamped
    /// to a mode the profile supports).
    pub mode: &'a str,
    /// Registry tool names exposed this turn (runtime tools excluded).
    pub tools: &'a [String],
    pub workspace_root: Option<&'a Path>,
    pub branch: Option<String>,
    /// Trimmed AGENT.md content (project memory), if present.
    pub agent_md: Option<String>,
    /// Project rules from `.cursor/rules/` / `.agent/rules/` as
    /// `(name, content)` pairs.
    pub rules: Vec<(String, String)>,
    /// Latest editor context window payload for the session.
    pub context_window: Option<&'a Value>,
    /// Discovered skills as `(name, one-line summary)`.
    pub skills: Vec<(String, String)>,
    /// Pre-rendered relevant memory lines (retrieved by the runtime).
    pub memories: Vec<String>,
    /// Whether the runtime-managed `todo_write` / `todo_update` tools are in
    /// the loop (depth 0, tool-enabled modes only).
    pub todo_tools: bool,
    /// Whether the runtime-managed `task` delegation tool is in the loop.
    pub task_tool: bool,
    /// Whether the runtime-managed `memory_*` tools are in the loop.
    pub memory_tools: bool,
    /// Rendered list of delegable subagent profiles (builtin + disk),
    /// supplied by the runtime when `task_tool` is true.
    pub subagents_prompt: Option<String>,
}

// ---------------------------------------------------------------------------
// Static layer — per-kind dispatch
// ---------------------------------------------------------------------------

fn persona(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Coding => coding::PERSONA,
        AgentKind::Document => document::PERSONA,
        AgentKind::General => general::PERSONA,
    }
}

fn mode_overlay(kind: AgentKind, mode: &str) -> &'static str {
    match kind {
        AgentKind::Coding => coding::mode_overlay(mode),
        AgentKind::Document => document::mode_overlay(mode),
        AgentKind::General => general::mode_overlay(mode),
    }
}

/// Conventions block (code conventions / doc conventions / answer guide).
fn conventions(kind: AgentKind) -> Option<&'static str> {
    match kind {
        AgentKind::Coding => Some(coding::CODE_CONVENTIONS),
        AgentKind::Document => Some(document::DOC_CONVENTIONS),
        AgentKind::General => Some(general::ANSWER_GUIDE),
    }
}

/// Workflow block, only injected when checklist (todo) tools are present.
fn workflow(kind: AgentKind) -> Option<&'static str> {
    match kind {
        AgentKind::Coding => Some(coding::WORKFLOW),
        AgentKind::Document => Some(document::DOC_WORKFLOW),
        AgentKind::General => None,
    }
}

// ---------------------------------------------------------------------------
// Dynamic layer
// ---------------------------------------------------------------------------

fn environment_section(ctx: &PromptContext) -> Option<String> {
    let root = ctx.workspace_root?;
    let mut lines = vec![format!("- 工作区根目录: `{}`", root.display())];
    if let Some(branch) = &ctx.branch {
        if !branch.is_empty() {
            lines.push(format!("- Git 分支: {branch}"));
        }
    }
    lines.push(format!("- 操作系统: {}", std::env::consts::OS));
    lines.push(format!("- UTC 时间: {}", now_utc_string()));
    Some(format!("## 环境\n{}", lines.join("\n")))
}

fn now_utc_string() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string()
}

fn agent_md_section(ctx: &PromptContext) -> Option<String> {
    let body = ctx.agent_md.as_deref()?.trim();
    if body.is_empty() {
        return None;
    }
    let clipped: String = body.chars().take(AGENT_MD_MAX_CHARS).collect();
    Some(format!(
        "## 项目记忆（AGENT.md）\n```markdown\n{clipped}\n```"
    ))
}

fn rules_section(ctx: &PromptContext) -> Option<String> {
    if ctx.rules.is_empty() {
        return None;
    }
    let mut parts = vec!["## 项目规则（必须遵守）".to_string()];
    for (name, content) in &ctx.rules {
        parts.push(format!("### {name}\n{content}"));
    }
    Some(parts.join("\n\n"))
}

fn memories_section(ctx: &PromptContext) -> Option<String> {
    if ctx.memories.is_empty() {
        return None;
    }
    let mut lines =
        vec!["## 相关记忆（来自长期记忆，供参考；如与用户当前指令冲突以用户为准）".to_string()];
    for m in ctx.memories.iter().take(MAX_MEMORY_ITEMS) {
        lines.push(format!("- {m}"));
    }
    Some(lines.join("\n"))
}

fn context_window_section(ctx: &PromptContext) -> Option<String> {
    let cw = ctx.context_window?;
    if !cw.is_object() || cw.as_object().is_some_and(|o| o.is_empty()) {
        return None;
    }
    let raw = serde_json::to_string(cw).ok()?;
    let clipped: String = raw.chars().take(CONTEXT_WINDOW_MAX_CHARS).collect();
    Some(format!(
        "## 编辑器上下文（用户当前打开/选中的内容，仅供参考，不要原样复述）\n```json\n{clipped}\n```"
    ))
}

fn skills_section(ctx: &PromptContext) -> Option<String> {
    if ctx.skills.is_empty() {
        return None;
    }
    let mut lines = vec![
        "## 可用技能（skills）".to_string(),
        "当任务与某个技能匹配时，先用 read_skill 读取其完整说明，再按说明执行：".to_string(),
    ];
    for (name, summary) in ctx.skills.iter().take(MAX_SKILL_ITEMS) {
        if summary.is_empty() {
            lines.push(format!("- {name}"));
        } else {
            lines.push(format!("- {name}: {summary}"));
        }
    }
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// Build the full system prompt for a main-loop agent turn.
pub fn build_system_prompt(ctx: &PromptContext) -> String {
    let tools_enabled = !ctx.tools.is_empty() && ctx.mode != "ask";

    let mut static_parts: Vec<String> = vec![
        persona(ctx.kind).to_string(),
        mode_overlay(ctx.kind, ctx.mode).to_string(),
    ];
    if tools_enabled {
        static_parts.push(shared::TOOL_PRINCIPLES.to_string());
        if let Some(c) = conventions(ctx.kind) {
            static_parts.push(c.to_string());
        }
        if ctx.todo_tools {
            if ctx.mode == "plan" {
                static_parts.push(coding::PLAN_WORKFLOW.to_string());
            } else if let Some(w) = workflow(ctx.kind) {
                static_parts.push(w.to_string());
            }
        }
        if ctx.memory_tools {
            static_parts.push(shared::MEMORY_GUIDANCE.to_string());
        }
        if ctx.task_tool {
            let blurb = ctx
                .subagents_prompt
                .clone()
                .unwrap_or_else(|| "可委派的子代理类型见 task 工具说明。".to_string());
            static_parts.push(format!("{}\n{}", shared::SUBAGENT_GUIDANCE_HEADER, blurb));
        }
        static_parts.push(format!(
            "# 可用工具\n本轮可通过函数调用使用：{}。",
            ctx.tools.join(", ")
        ));
    } else {
        static_parts.push("# 可用工具\n本轮无可用工具，仅以文本回答。".to_string());
    }

    let mut out = format!(
        "<system_static>\n{}\n</system_static>",
        static_parts.join("\n\n")
    );

    let dynamic_parts: Vec<String> = [
        environment_section(ctx),
        agent_md_section(ctx),
        rules_section(ctx),
        // Memories are useful even on tool-free (ask) turns.
        memories_section(ctx),
        context_window_section(ctx),
        // Skills require the read_skill tool, so a tool-free turn skips them.
        if tools_enabled {
            skills_section(ctx)
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    .collect();
    if !dynamic_parts.is_empty() {
        out.push_str(&format!(
            "\n\n<system_dynamic>\n{}\n</system_dynamic>",
            dynamic_parts.join("\n\n")
        ));
    }
    out
}

/// Build the system prompt for a delegated subagent run. Deliberately slimmer
/// than the main prompt: no todo workflow, no delegation guidance, but the
/// same grounding rules plus the profile's persona and a result contract.
pub fn build_subagent_system_prompt(
    profile: &crate::subagents::SubagentProfile,
    tools: &[String],
    workspace_root: Option<&Path>,
    branch: Option<String>,
) -> String {
    let mut parts: Vec<String> = vec![format!(
        "# 角色\n你是受主代理委派的子代理（类型：{}），只负责完成下面这一个任务。\
         你的结论必须来自真实的文件与工具输出。使用简体中文。\n\n{}",
        profile.name, profile.system_prompt
    )];
    parts.push(shared::TOOL_PRINCIPLES.to_string());
    parts.push(
        "# 结果要求\n\
         - 任务完成后输出一段自包含的最终结果：关键结论、涉及的文件路径、必要的证据。\n\
         - 主代理只能看到你的最终回复，过程性内容不要赘述。\n\
         - 你不能委派新的子代理；遇到无法完成的部分，明确说明阻塞原因。"
            .to_string(),
    );
    if tools.is_empty() {
        parts.push("# 可用工具\n本轮无可用工具，仅以文本回答。".to_string());
    } else {
        parts.push(format!(
            "# 可用工具\n本轮可通过函数调用使用：{}。",
            tools.join(", ")
        ));
    }
    if let Some(root) = workspace_root {
        let mut env = format!("## 环境\n- 工作区根目录: `{}`", root.display());
        if let Some(b) = branch.filter(|b| !b.is_empty()) {
            env.push_str(&format!("\n- Git 分支: {b}"));
        }
        parts.push(env);
    }
    parts.join("\n\n")
}

/// Extra instruction appended to read-only exploration todos in plan runs.
pub const EXPLORE_TASK_SUFFIX: &str = "\n\n这是一个只读调研任务：只收集信息，不要修改任何文件。\
最终回复请输出结论要点（关键发现、相关文件路径、对后续实施的建议），不要描述探索过程。";

/// System prompt for the context-window compactor (code-agent aware: paths,
/// diffs and pending work survive the squeeze).
pub const COMPACTION_SYSTEM_PROMPT: &str = "\
你是代理的上下文压缩器。把下面的对话历史压缩为结构化要点，后续对话将只能看到这份摘要，\
所以必须保留继续工作所需的全部事实：\n\
- 用户的原始需求与后续修正；\n\
- 已完成的修改：具体文件路径、改动内容要点；\n\
- 探索得到的关键结论（重要文件路径、函数/结构名、调用关系）；\n\
- 已执行的命令及其结果（成功/失败、关键报错）；\n\
- 尚未完成的事项与已知问题。\n\
直接输出要点列表，不要寒暄或解释。";

/// System prompt used to squash an oversized subagent result before it is
/// handed back to the main loop.
pub const SUBAGENT_SUMMARY_SYSTEM_PROMPT: &str = "\
请把下面这段子任务执行结果压缩为要点摘要，供主代理继续工作使用。\
必须保留：关键结论、涉及的文件路径、数据/报错原文中的关键部分、未完成事项。\
直接输出摘要，不要其它说明。";

/// System prompt for the plan-engine task drafter (JSON-only contract).
pub const PLAN_DRAFT_SYSTEM_PROMPT: &str = "\
你是任务规划器。把用户目标拆解为 2-5 个可执行子任务，遵循“先调研、后实施”。\
只输出一个 JSON 数组，每项为 {\"title\": string, \"description\": string, \
\"kind\": \"explore\"|\"edit\", \"dependsOn\": number[]}。规则：\n\
- kind=explore 表示只读调研（读代码、搜索、收集信息），kind=edit 表示实施修改（写文件、执行命令）；\n\
- description 写明该子任务的完成标准，子任务执行者看不到本次对话；\n\
- dependsOn 是所依赖任务的数组下标，只能引用更靠前的任务，互相独立的任务留空数组；\n\
- 调研类任务放在最前并尽量互相独立（可并行）；\n\
- 目标本身很简单时输出单个 edit 任务即可。\n\
不要输出 JSON 以外的任何文字。";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn tools(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn build_mode_includes_workflow_and_tools() {
        let t = tools(&["read_file", "write_file"]);
        let ctx = PromptContext {
            mode: "build",
            tools: &t,
            todo_tools: true,
            task_tool: true,
            memory_tools: true,
            ..Default::default()
        };
        let p = build_system_prompt(&ctx);
        assert!(p.contains("工作流程"));
        assert!(p.contains("task"));
        assert!(p.contains("memory_search"));
        assert!(p.contains("read_file, write_file"));
        assert!(p.contains("<system_static>"));
        assert!(!p.contains("<system_dynamic>"));
    }

    #[test]
    fn ask_mode_is_tool_free() {
        let ctx = PromptContext {
            mode: "ask",
            ..Default::default()
        };
        let p = build_system_prompt(&ctx);
        assert!(p.contains("ASK"));
        assert!(p.contains("无可用工具"));
    }

    #[test]
    fn document_kind_uses_document_persona() {
        let t = tools(&["read_file", "create_document"]);
        let ctx = PromptContext {
            kind: AgentKind::Document,
            mode: "build",
            tools: &t,
            todo_tools: true,
            ..Default::default()
        };
        let p = build_system_prompt(&ctx);
        assert!(p.contains("文档处理代理"));
        assert!(p.contains("文档写作约定"));
    }

    #[test]
    fn general_kind_has_no_workflow() {
        let t = tools(&["read_file"]);
        let ctx = PromptContext {
            kind: AgentKind::General,
            mode: "build",
            tools: &t,
            todo_tools: true,
            ..Default::default()
        };
        let p = build_system_prompt(&ctx);
        assert!(p.contains("通用 AI 助手"));
        // General profile has no checklist workflow block.
        assert!(!p.contains("# 工作流程"));
    }

    #[test]
    fn dynamic_sections_render_when_present() {
        let t = tools(&["read_file"]);
        let root = PathBuf::from("/tmp/ws");
        let cw = json!({ "activeFile": { "path": "a.rs" } });
        let ctx = PromptContext {
            mode: "build",
            tools: &t,
            workspace_root: Some(&root),
            branch: Some("main".to_string()),
            agent_md: Some("约定：所有提交信息用英文。".to_string()),
            context_window: Some(&cw),
            skills: vec![("essay-grading".into(), "作文批改".into())],
            memories: vec!["[preference] 用户偏好简体中文".to_string()],
            todo_tools: true,
            task_tool: false,
            ..Default::default()
        };
        let p = build_system_prompt(&ctx);
        assert!(p.contains("<system_dynamic>"));
        assert!(p.contains("工作区根目录"));
        assert!(p.contains("Git 分支: main"));
        assert!(p.contains("AGENT.md"));
        assert!(p.contains("activeFile"));
        assert!(p.contains("essay-grading"));
        assert!(p.contains("read_skill"));
        assert!(p.contains("相关记忆"));
    }

    #[test]
    fn oversized_agent_md_is_clipped() {
        let t = tools(&["read_file"]);
        let ctx = PromptContext {
            mode: "build",
            tools: &t,
            workspace_root: None,
            agent_md: Some("x".repeat(50_000)),
            ..Default::default()
        };
        let p = build_system_prompt(&ctx);
        assert!(p.len() < 30_000);
    }

    #[test]
    fn subagent_prompt_carries_profile_and_contract() {
        let profile = crate::subagents::get_builtin("explorer").unwrap();
        let t = tools(&["read_file", "grep"]);
        let p = build_subagent_system_prompt(&profile, &t, None, None);
        assert!(p.contains("explorer"));
        assert!(p.contains("结果要求"));
        assert!(p.contains("read_file, grep"));
        assert!(!p.contains("todo_write"));
    }
}
