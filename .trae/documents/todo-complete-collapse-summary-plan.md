# 规划：Todo 完成后收起内部工作模块并显示概括内容

## Summary

- 目标：当一个“大流程 Todo”完成后，自动收起该 Todo 内部的工作模块，并在界面上显示该 Todo 的概括内容。
- 覆盖范围：同时作用于聊天时间线中的 `todo` / 工作模块展示，以及右侧 Todo 看板卡片。
- 成功标准：
  - Todo 处于完成态后，其内部工作模块默认收起。
  - 收起后仍保留一段简洁概括内容，优先显示后端已有 `summary` 或与该 Todo 关联的子任务 summary。
  - 聊天时间线与 Todo 看板使用一致的概括内容来源与完成态判断规则。
  - 不要求新增后端接口；优先复用现有 `todo.completed` 与 `subagent.summary.generated` 事件及 `TodoItem.summary` 字段。

## Current State Analysis

### 前端现状

- `apps/agent-ide/public/interactions.jsx`
  - [buildAgentBlocksFromEvents](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/interactions.jsx#L303-L471) 目前把 `todo.*` 事件聚合为一个 `todo` block，但只保留：
    - `id`
    - `title`
    - `status`
  - 当前没有把 `summary`、`description`、`relatedSubagentRunIds` 等 Todo 完成后的概括信息聚合进前端 block。
  - 同文件对 `subagent.summary.generated` 的处理是直接生成一条独立 `text` summary block，而不是回填到 Todo。

- `apps/agent-ide/public/components.jsx`
  - 现有时间线 `todo` 内容由 `TodoBody` 渲染，仅列出任务标题和状态，没有“完成后概括内容”区域。
  - `AgentActionCard` 已经支持时间线卡片折叠，但 `todo` 卡片当前默认总是展开。
  - [CollapsibleNode](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/components.jsx#L856-L915) 是 Todo 看板卡片复用的折叠组件，目前默认运行中展开、其他状态依赖 `defaultOpen`，没有“完成后显示 summary”的逻辑。

- `apps/agent-ide/public/main.jsx`
  - [TodoBoardPage](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/main.jsx#L129-L261) 将后端 `todos` 映射为看板卡片，只取：
    - `id`
    - `title`
    - `description`
    - `owner`
    - `dueHint`
    - `status`
  - 当前没有读取 `todo.summary`，也没有展示“完成概括内容”。
  - 看板卡片使用 [CollapsibleNode](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/components.jsx#L856-L915)，并通过 `defaultOpen={cardStatus === "running"}` 控制初始展开；对已完成卡片不会额外展示 summary。

### 后端现状

- `backend/src/agent_debug/domain/models.py`
  - [TodoItem](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/models.py#L141-L161) 已有 `summary` 字段，可直接复用作为 Todo 完成概括。
  - 同时保留 `relatedSubagentRunIds`、`artifacts`、`description` 等辅助信息。

- `backend/src/agent_debug/domain/runtime.py`
  - [todo.completed 发布](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L250-L260) 会把完整 `TodoItem` payload 推给前端，因此前端理论上可以直接使用 `summary`。
  - [subagent.summary.generated 发布](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L368-L375) 会额外发布结构化子任务总结，其中包括：
    - `taskId`
    - `subagentRunId`
    - `objective`
    - `keyFindings`
    - `actions`
    - `nextActions`
  - 这意味着即使 `TodoItem.summary` 为空，前端也可回退到子任务 summary 文本。

## Proposed Changes

### 1. 扩展前端 Todo 事件聚合，补齐概括数据

- 文件：`apps/agent-ide/public/interactions.jsx`
- 变更内容：
  - 在 `buildAgentBlocksFromEvents()` 中扩展 `todoMap` 保存的字段，不再只保留 `id/title/status`，还要补充：
    - `description`
    - `summary`
    - `relatedSubagentRunIds`
    - 可选 `updatedAt`
  - 对 `subagent.summary.generated` 建立一个按 `subagentRunId` 索引的临时 summary 映射。
  - 当 Todo 事件携带 `relatedSubagentRunIds` 且 `summary` 为空时，用对应子任务 summary 的 `keyFindings/objective` 生成前端 fallback 概括文本。

- 决策规则：
  - 概括内容优先级：
    1. `todo.summary`
    2. 关联 `subagent.summary.generated` 的 `keyFindings`
    3. 关联 summary 的 `objective`
    4. `todo.description`

### 2. 聊天时间线中的 Todo 卡片：完成后默认收起并显示概括

- 文件：`apps/agent-ide/public/components.jsx`
- 变更内容：
  - 扩展 `TodoBody`，支持两部分：
    - `summary` 概括区
    - 工作模块列表（即当前 items 明细）
  - 在 `AgentActionCard` 中对 `todo` 类型加入特殊展开规则：
    - 运行中：默认展开
    - 已完成：默认收起
    - 用户手动展开后仍允许查看明细
  - 在收起状态下，卡片头部下方直接显示一段 summary 预览；展开后显示完整概括和内部工作模块列表。

- 呈现方式：
  - 收起时：显示 `任务进度 (x/y)` + 一段简洁概括
  - 展开时：先显示概括，再显示内部任务列表

### 3. Todo 看板卡片：完成后收起并显示概括

- 文件：`apps/agent-ide/public/main.jsx`
- 变更内容：
  - 在 `TodoBoardPage` 的 `backendCards` 映射中加入 `summary` 字段。
  - `summary` 来源规则与聊天时间线保持一致，优先使用后端 todo 自带 `summary`。
  - 修改 `CollapsibleNode` 的 children 内容：
    - 顶部新增 `tc-summary` 概括内容块
    - 概括内容在完成态始终可见
    - 原有描述、owner、操作按钮作为“内部工作模块”展示内容
  - 完成态卡片默认收起，运行态仍默认展开。

- 关键点：
  - 不改看板列状态映射，仅改卡片内部呈现层级。
  - 已完成卡片的核心信息由“概括内容”替代“完整工作模块展开”。

### 4. 为完成态 Todo 增加概括与收起样式

- 文件：`apps/agent-ide/public/styles.css`
- 变更内容：
  - 新增 Todo 概括类样式，例如：
    - `.todo-summary`
    - `.todo-summary--collapsed`
    - `.tc-summary`
  - 完成态卡片与时间线 Todo 保持极简风格：
    - 概括文字置灰但保持可读
    - 与内部任务明细之间用轻量分隔
    - 收起状态下不再显示冗余大块内容

### 5. 统一完成态判定与降级行为

- 文件：`apps/agent-ide/public/components.jsx`
- 文件：`apps/agent-ide/public/main.jsx`
- 文件：`apps/agent-ide/public/interactions.jsx`
- 变更内容：
  - 完成态统一判定：
    - `completed`
    - `rolled_up`
    - `rolledUp`
    - 看板侧映射后的 `done`
  - 如果没有任何 summary 可用：
    - 聊天时间线显示“已完成，暂无概括内容”
    - Todo 看板显示“已完成，暂无概括内容”
  - 避免因为缺 summary 导致完成卡片收起后信息完全消失。

## Assumptions & Decisions

- 本次优先复用现有后端数据，不新增 API 或事件协议。
- Todo 完成后的“概括内容”优先使用 `TodoItem.summary`；若后端暂未写入，则前端尝试从 `subagent.summary.generated` 衍生。
- “工作模块”在聊天时间线中指 Todo 内部任务明细，在 Todo 看板中指描述、owner、操作区等完整卡片主体。
- 只在完成态自动收起；运行中、失败中不自动收起。

## Verification Steps

1. 在聊天时间线中触发一个包含 Todo 的流程，确认：
   - 运行中 Todo 默认展开
   - 完成后 Todo 自动收起
   - 收起后仍能看到概括内容
2. 在 Todo 看板中确认：
   - 完成列中的卡片默认收起
   - 卡片上直接可见概括内容
   - 展开后仍可看到描述、owner、按钮等内部模块
3. 验证无 summary 场景：
   - 完成后显示兜底文案，而不是空白
4. 使用诊断工具检查：
   - `interactions.jsx`
   - `components.jsx`
   - `main.jsx`
   - `styles.css`
5. 执行前端构建，确认无语法或打包错误。
