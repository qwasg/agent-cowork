# 删干净左侧栏计划

## Summary

- 目标：删除你框选的左侧竖向图标栏 `ActivityBar`，并把它占用的布局列、切换逻辑和失效样式一起清理，做到视觉和代码层面都“删干净”。
- 范围：仅处理 `apps/agent-ide` 前端源码，不改后端、不改功能页内容、不处理与该侧栏无关的其它面板。

## Current State Analysis

- 当前左侧栏由 `apps/agent-ide/public/components.jsx` 中的 `ActivityBar` 组件渲染，按钮包含 `Agents`、`Sessions`、`Plans`、`Swarm`、`Logs`、`Metrics`、`个人资料`、`设置`。
- 主布局位于 `apps/agent-ide/public/main.jsx`；`AppShell` 的 `.body` 第一列固定为 `48px`，专门用于放置 `ActivityBar`，第二列才是 `SessionsSidebar`。
- `changeActivity()` 负责处理左栏点击后的跳转逻辑：`plans/swarm` 切主标签，`logs/metrics` 打开底部区域，`sessions/agents` 回到 `readme`。
- `activity` 状态在 `apps/agent-ide/public/interactions.jsx` 里持久化为 `moonlit:activity`，默认值是 `agents`。
- 左栏样式实际来自 `apps/agent-ide/index.html` 内联 CSS 中的 `.activitybar`、`.activitybar .abtn`、`.activitybar .spacer`；`public/styles.css` 里没有这组样式定义。
- `index.html` 直接加载 `/components.jsx`、`/main.jsx`、`/styles.css`，因此应修改 `public` 源码和 `index.html`，不需要直接维护 `dist`。

## Proposed Changes

### 1. 移除左栏渲染

- 文件：`apps/agent-ide/public/main.jsx`
- 改动：
  - 删除 `AppShell` 中 `<ActivityBar ... />` 的挂载。
  - 调整 `bodyColumns`，去掉原本预留给左栏的 `48px` 第一列。
  - 保持 `SessionsSidebar` 作为最左侧首列，不留下空白占位。
- 原因：仅删组件本身会留下 48px 空白列，界面看起来没有“删干净”。

### 2. 清理与左栏绑定的交互状态

- 文件：`apps/agent-ide/public/main.jsx`
- 改动：
  - 删除只服务于左栏点击的 `changeActivity()`。
  - 清理 `ActivityBar` 相关 props 传递。
- 文件：`apps/agent-ide/public/interactions.jsx`
- 改动：
  - 移除 `activity` / `setActivity` 的持久化状态，前提是确认它没有被其它 UI 直接使用。
  - 如果仍被少量逻辑间接依赖，则退而求其次：保留状态但断开 UI 挂载，避免一次改动过大。
- 原因：左栏删除后，这组状态大概率会变成死代码和无效本地存储键。

### 3. 清理左栏组件与无用样式

- 文件：`apps/agent-ide/public/components.jsx`
- 改动：
  - 删除 `ActivityBar` 组件定义。
  - 保留 `SessionsSidebar`、`TitleBar` 等仍在使用的组件。
- 文件：`apps/agent-ide/index.html`
- 改动：
  - 删除 `.activitybar` 相关内联 CSS 规则。
  - 如 `.body` 的静态默认列定义仍包含左栏宽度，也一并更新为新的列结构。
- 原因：避免留下未引用组件和孤儿样式。

### 4. 控制影响范围，保留现有功能入口

- 文件：`apps/agent-ide/public/main.jsx`
- 改动策略：
  - 不删除 `plan`、`swarm`、`logs`、`metrics` 等页面能力本身。
  - 现有其它入口若仍可打开这些面板，则保持不变；若左栏是唯一入口，则先记录为已知影响，不额外扩展新入口，除非你后续要求。
- 原因：本次目标是“删掉侧栏”，不是重做整套导航。

## Assumptions & Decisions

- 决策：将“这侧栏删干净”理解为删除整条左侧竖向图标栏，而不是删除 `SessionsSidebar` 会话列表。
- 决策：修改以 `public` 源码和 `index.html` 为准，不手改 `dist`。
- 假设：你当前不要求为 `Plans / Swarm / Logs / Metrics` 补新的可视化入口，只要求把侧栏移除。
- 假设：删除 `ActivityBar` 后，最左侧显示会话栏是可接受结果。
- 风险控制：如果执行时发现 `activity` 状态在别处仍承担关键行为，会改为最小清理方案，先移除 UI 与布局，再保留内部状态避免回归。

## Verification Steps

- 运行前端后确认左侧竖向图标栏完全消失。
- 确认主内容区左侧没有残留 48px 空白带。
- 确认会话栏正常贴到最左侧，宽度拖拽与收起/展开行为不受影响。
- 确认 `plan`、`swarm`、`logs`、`metrics` 既有非侧栏入口若存在仍可用。
- 检查最近修改文件的诊断信息，确保没有 JSX/CSS 语法错误或未引用符号报错。
