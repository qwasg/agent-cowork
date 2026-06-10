# 规划：重新设计极简主义的 Agent 动作卡片 UI

## Summary

- 目标：重新设计 Agent 行动时间线的 UI，去除当前嵌套冗余的卡片边框和标题，实现真正的“极简主义”，同时保证折叠功能正常、工具与输出顺序正确，提升 DOM 和显示效率。
- 成功标准：
  - 移除多重嵌套的卡片（如 `AgentActionCard` 内部又套 `ToolUseCard` 导致的双重 Header）。
  - 统一由 `AgentActionCard` 处理所有非文本动作（思考、工具、待办、计划）的折叠逻辑。
  - 文本输出不再包裹在卡片中，而是作为平铺的 Markdown 文本直接显示在时间线中，实现“对话输出不乱序”且显示高效。
  - 样式调整为极简风格：更少的边框、更小的内边距、更柔和的背景色。

## Current State Analysis

- 当前 `apps/agent-ide/public/components.jsx` 中，`AgentActionTimeline` 遍历所有 block，并使用 `AgentActionCard` 包裹。
- 但 `AgentActionCard` 内部又渲染了带有自身 Header 和折叠逻辑的 `ReasoningBlock`、`ToolUseCard` 等组件，导致严重的 UI 冗余（截图中的“思考”框里还有一个“思考过程 >”按钮）。
- `styles.css` 中 `.agent-action-card` 具有较重的阴影、边距和背景色，不够极简。

## Proposed Changes

### 1. 精简 `components.jsx` 中的冗余组件

- 文件：`apps/agent-ide/public/components.jsx`
- 动作：
  - 删除带有独立头部和折叠状态的冗余组件：`ToolUseCard`、`ProcessGroup`、`ReasoningBlock`、`TaskProgressCard`。
  - 新增纯内容组件：`ReasoningBody`、`ToolBody`、`TodoBody`、`PlanBody`，它们只负责渲染展开后的主体内容。

### 2. 重构 `AgentActionCard` 作为唯一的折叠容器

- 文件：`apps/agent-ide/public/components.jsx`
- 动作：
  - 将 `AgentActionCard` 改造为自带 `open` 状态和 toggle 按钮的统一卡片容器。
  - Header 直接渲染：图标、标题（对于工具，直接显示工具名称及参数摘要）、状态（运行中/成功/失败）、折叠箭头。
  - Body 区域根据 `block.type` 渲染对应的 `*Body` 组件。
  - 保留默认折叠逻辑：思考过程完成后默认折叠，工具调用成功后默认折叠，正在运行或失败时展开。

### 3. 更新 `AgentActionTimeline` 渲染逻辑

- 文件：`apps/agent-ide/public/components.jsx`
- 动作：
  - 当 `block.type === "text"` 时，**直接**渲染 `<MarkdownText>`，不使用 `AgentActionCard` 包裹。这使得文本对话直接与工具卡片平级穿插，极致高效。
  - 当 `block.type !== "text"` 时，渲染 `<AgentActionCard>`。

### 4. 优化 `styles.css` 极简样式

- 文件：`apps/agent-ide/public/styles.css`
- 动作：
  - 修改 `.agent-action-card`：取消内边距（`padding: 12px` 改为 `0`），使用较轻的边框和极淡的背景，取消或减弱阴影。
  - 修改 `.agent-action-card-head`：设置为可点击的按钮，横向排列内容，添加 `hover` 效果。
  - 修改 `.agent-action-card-body`：添加顶部边框和适当的内边距。
  - 调整 `.tool-use-args` 和 `.tool-use-result`：保持无横向滚动条，自动换行（继承上一任务的优化）。

## Assumptions & Decisions

- 文本对话输出不再需要卡片框，直接显示文本是最极简、高效的方式。
- 工具调用和思考过程依然需要卡片框以容纳复杂的输入输出，但去除了冗余嵌套，只保留一层 Header。
- MCP 标签和工具状态等视觉元素将被精简并融入 `AgentActionCard` 的统一头部。

## Verification Steps

1. 修改代码后，执行 `npm run lint` 和 `npm run build` 确保无语法和构建错误。
2. 确认 `ReasoningBlock` 和 `ToolUseCard` 的原有逻辑被正确迁移到 `AgentActionCard` 及对应的 Body 组件中。
3. 检查应用逻辑，确保点击卡片头部能正常展开/折叠上下文。
4. 确认在实际对话流中，工具卡片和文本输出交替出现且顺序正确。
