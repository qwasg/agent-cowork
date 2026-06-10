---
name: essay-grading
description: Analyze a Chinese essay prompt, build scoring rubric, delegate multiple graders for batch essay review, and generate a polished HTML grading report. Use when the user asks for 作文批改、审题、评分标准、作文评语、批量作文评分、写作报告.
---

# Essay Grading Workflow

## Goal

批量完成中文作文批改，产出可复核的评分依据，并保存为可直接展示的 HTML 报告。

## Required Inputs

- 作文题目文件（通常是 `.md`）
- 学生作文文件列表（一个或多个 `.md`）
- 输出目录（默认可使用 `C:/Users/苍/Desktop/测试`）

如果缺少任一输入，先向用户补齐再继续。

## Workflow

1. **审题子代理（必须）**
   - 先委派 1 个子代理专门完成审题，不直接开始评分。
   - 子代理输出：
     - 题目核心任务（写什么、怎么写、边界）
     - 常见跑题风险
     - 评分维度与分值（建议总分 60）
     - 分档标准（优秀/良好/及格/待提高）

2. **生成统一评分标准**
   - 主代理整合审题结果，形成最终评分 Rubric。
   - Rubric 至少包含以下维度：
     - 立意与审题
     - 结构与逻辑
     - 论证与素材
     - 语言表达
     - 规范与完成度

3. **按作文数量并行委派批改子代理**
   - 根据作文数量 `N`，委派 `N` 个批改子代理（每篇 1 个，允许并发）。
   - 每个批改子代理必须读取「题目 + Rubric + 对应学生作文」。
   - 每个子代理输出统一结构：
     - `student_name`
     - `total_score`
     - `dimension_scores`（按 Rubric）
     - `strengths`（2-3 条）
     - `improvements`（2-3 条）
     - `line_edits`（可选，给出原句与建议）
     - `final_comment`（80-150 字）

4. **主代理汇总与校验**
   - 校验总分与维度分一致。
   - 生成总览：均分、最高分、最低分、常见问题 Top3。
   - 给出全班写作改进建议（3-5 条）。

5. **生成美观 HTML 报告**
   - 将结果写入 HTML 文件，包含：
     - 报告标题、题目原文、评分 Rubric
     - 每位学生的评分卡片
     - 班级统计与建议
   - 默认输出文件名：`essay_grading_report.html`
   - 默认输出路径：`C:/Users/苍/Desktop/测试/essay_grading_report.html`

## HTML Style Requirements

- 页面风格简洁、卡片化、易打印
- 使用系统字体（中文可读性优先）
- 颜色分层清晰（标题、正文、弱文本、边框）
- 分数高低有视觉区分（如徽章/色块）
- 桌面端优先，移动端可自适应

## Output Contract

最终回复必须包含：

1. 本次使用的评分标准摘要
2. 每篇作文分数与一句总评
3. 生成的 HTML 文件路径
4. 若有失败作文，明确失败原因与补救建议

## Notes

- 若用户未指定总分，默认总分 60。
- 不要跳过“先审题再批改”的顺序。
- 批改评语要具体引用作文内容，避免模板化空话。
