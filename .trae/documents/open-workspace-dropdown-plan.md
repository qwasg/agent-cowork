# Open Workspace 下拉改造计划

## Summary

- 目标：把 `apps/agent-ide` 左侧边栏底部的 `Open Workspace` 从“直接触发打开目录”改为接近截图的下拉面板交互。
- 本次范围只改左侧边栏入口，不改输入框上方现有 `WorkspaceSwitcher`。
- 成功标准：
  - 点击左下角 `Open Workspace` 时弹出下拉面板，而不是立即触发目录选择。
  - 面板展示“最近工作区”列表，并支持直接切换到某个最近路径。
  - 面板提供 `Open Folder` 可用入口，继续复用现有目录选择/切换逻辑。
  - 视觉结构接近截图：标题行、最近列表、当前工作区勾选态、底部分组动作。
  - `Set Up Workspace`、`Connect SSH`、`Connect WSL` 本次不接真实能力，先不做或以禁用态保留。

## Current State Analysis

### 已确认的现有入口与职责

- 左侧边栏底部按钮定义在 `apps/agent-ide/public/components.jsx`：
  - `SessionsSidebar` 中通过 `SidebarActionRow` 渲染 `Open Workspace`
  - 点击后调用外部传入的 `onOpenWorkspace`
- 侧边栏 `onOpenWorkspace` 实现在 `apps/agent-ide/public/main.jsx` 的 `handleOpenWorkspace()`：
  - Tauri 下调用 `tauri.openFolder()`
  - 浏览器/HTTP 模式调用 `window.__moonlit_openWorkspacePicker()`
- 输入框上方已有单独的工作区切换器，定义在 `apps/agent-ide/public/workspace-switcher.jsx`：
  - 已具备 `readRecents()` / `pushRecent()` / `applyWorkspaceRoot()` / `workspaceInfo()` 能力
  - 已能展示最近工作区并切换
  - 已能打开后端驱动的目录选择弹窗 `FolderBrowserModal`

### 已确认的样式落点

- `SidebarActionRow` 的视觉样式不在 `public/styles.css`，而在 `apps/agent-ide/index.html` 的内联样式：
  - `.sess-action-row`
  - `.sessions-foot`
  - `.sess-user-menu`
- `workspace-switcher.jsx` 自己通过注入 `<style>` 的方式维护一套局部样式：
  - `.ws-switcher-menu`
  - `.ws-menu-item`
  - `.ws-menu-label`

### 当前实现与目标图的差异

- 当前左下角入口只是一个普通按钮，没有自己的展开/收起状态。
- 最近工作区数据已存在，但只在输入框上方切换器内展示，没有复用到侧边栏入口。
- 侧边栏入口当前直接执行动作，不支持截图里的“先看最近工作区，再决定是切换还是打开文件夹”。
- 项目内暂未发现 `SSH` / `WSL` / `Set Up Workspace` 的真实实现入口，因此这几项不能在本次计划中假设为可用。

## Proposed Changes

### 1. 将侧边栏 `Open Workspace` 从动作按钮改为带面板的触发器

修改文件：

- `apps/agent-ide/public/components.jsx`
- `apps/agent-ide/public/main.jsx`

计划内容：

- 在 `components.jsx` 中把当前简单的 `SidebarActionRow` 用法替换成一个专用的工作区菜单触发器组件，例如独立的 `SidebarWorkspaceMenu`。
- 该组件负责：
  - 管理展开/收起状态
  - 处理外部点击与 `Escape` 关闭
  - 渲染标题、最近工作区列表、底部动作区
- `main.jsx` 中不再让左下角入口直接调用 `handleOpenWorkspace()`；改为把“真正打开文件夹”的动作以回调形式传给该菜单组件。
- 现有 `handleOpenWorkspace()` 继续保留，作为菜单内 `Open Folder` 的实际执行逻辑，避免重复实现 Tauri/浏览器双分支。

### 2. 复用已有工作区切换逻辑，避免重新造状态源

修改文件：

- `apps/agent-ide/public/workspace-switcher.jsx`
- `apps/agent-ide/public/components.jsx`

计划内容：

- 抽出或显式暴露工作区菜单所需的复用能力，优先复用 `workspace-switcher.jsx` 中已稳定存在的以下逻辑：
  - 最近工作区读取：`readRecents()`
  - 当前工作区切换：`applyWorkspaceRoot()`
  - 当前工作区信息读取：`workspaceInfo()`
- 目标不是让侧边栏直接复用整个 `WorkspaceSwitcher` 视觉，而是复用其数据和切换语义：
  - 最近列表与输入框上方切换器读取同一份 `localStorage`
  - 选择最近工作区后仍走统一的 `applyWorkspaceRoot()`
  - 切换后继续依赖现有事件 `moonlit:workspace:changed` 刷新其他区域
- 如果当前文件作用域不便直接复用内部函数，则在 `workspace-switcher.jsx` 增加稳定的 `window` 级 helper，供侧边栏调用；避免在 `components.jsx` 重写一份 recents/apply 逻辑。

### 3. 设计侧边栏下拉菜单结构，贴近截图但只落可用功能

修改文件：

- `apps/agent-ide/public/components.jsx`
- `apps/agent-ide/index.html`

计划内容：

- 菜单结构按“功能相似、布局接近截图”实现：
  - 顶部标题：`Open Workspace`
  - 最近区域：列出最近工作区路径，当前工作区显示勾选态
  - 底部分组动作：至少保留 `Open Folder`
- 对截图中暂未实现的条目采用以下策略：
  - `Set Up Workspace`
  - `Connect SSH`
  - `Connect WSL`
  本次默认显示为禁用态，或在视觉上保留但不触发真实逻辑。
- 不实现截图中的 `Run Cursor anywhere...` 搜索输入能力，因为当前仓库没有对应命令面板/过滤器语义，加入会扩大范围。
- 可保留 `Home` 静态项仅作为视觉占位，但更建议先不做，避免用户点击后产生错误预期。

### 4. 为新菜单补充与侧边栏风格一致的样式

修改文件：

- `apps/agent-ide/index.html`

计划内容：

- 在现有侧边栏样式区附近增加工作区下拉相关样式，保持与 `.sess-user-menu`、`.sess-action-row` 同一视觉体系。
- 样式重点覆盖：
  - 触发器打开态
  - 浮层定位与阴影
  - 最近工作区行 hover/active/check 状态
  - 底部动作区分隔线与禁用态
- 保持当前项目的低对比、浅色、圆角风格，不引入额外样式文件或 CSS 依赖。
- 如果需要让路径更接近截图表现，可采用“文件夹名主显示 + 完整路径次显示”或单行省略路径，但最终以当前项目可快速复用的文本结构为准。

### 5. 控制两套工作区入口的职责边界

修改文件：

- `apps/agent-ide/public/components.jsx`
- `apps/agent-ide/public/workspace-switcher.jsx`

计划内容：

- 输入框上方 `WorkspaceSwitcher` 继续保留，避免本次改动波及聊天区交互。
- 侧边栏新菜单与上方切换器共享同一数据源和切换行为，但不强制统一为同一个组件。
- 两者职责定义：
  - 侧边栏入口：偏“全局入口/最近工作区入口”
  - 输入框上方入口：偏“当前会话上下文中的快捷切换器”
- 通过共享 helper 保证它们不会出现“最近列表不一致”或“一个能切换、一个不刷新”的行为偏差。

## Assumptions & Decisions

- 决策：只改左下角 `Open Workspace`，不移除也不重做输入框上方现有切换器。
- 决策：优先实现“最近工作区 + Open Folder”两项真实能力，其余截图项不接真实后端能力。
- 决策：视觉目标是“功能相似、风格接近截图”，不是逐像素高仿。
- 决策：最近工作区与工作区切换必须复用现有 `workspace-switcher.jsx` 的状态与事件，不在 `components.jsx` 内复制一套业务逻辑。
- 假设：当前 `applyWorkspaceRoot()`、`workspaceInfo()`、`moonlit:workspace:changed` 链路已经稳定可用，本次只是在侧边栏新增一个更合适的入口包装。
- 假设：`SSH` / `WSL` / `Set Up Workspace` 当前没有现成实现，因此不会在本次范围内补产品能力。

## Verification Steps

### 代码级验证

- 检查 `apps/agent-ide/public/components.jsx`：
  - 左下角 `Open Workspace` 不再直接绑定 `handleOpenWorkspace()`
  - 新增菜单组件拥有展开/关闭与最近列表渲染逻辑
- 检查 `apps/agent-ide/public/workspace-switcher.jsx`：
  - 最近工作区与切换 helper 可被侧边栏入口复用
  - 不会破坏现有输入框上方切换器行为
- 检查 `apps/agent-ide/index.html`：
  - 新增的菜单样式与侧边栏现有内联样式放在同一区域

### 手工验收

- 点击左下角 `Open Workspace`
  - 应打开下拉面板，而不是立刻弹系统目录选择器
- 菜单中存在最近工作区列表
  - 点击任意最近路径后应切换工作区
  - 当前工作区应有明显选中/勾选态
- 点击菜单中的 `Open Folder`
  - Tauri 下继续弹系统文件夹选择器
  - 浏览器模式继续打开现有后端目录浏览弹窗
- 切换工作区后
  - 侧边栏菜单当前项更新
  - 输入框上方 `WorkspaceSwitcher` 同步更新
  - 工作区树继续刷新
- 未实现项若显示出来
  - 应为禁用态或只展示不可点击视觉，不应误触发错误逻辑

### 执行后检查

- 对修改过的前端文件运行诊断检查，确认没有新增语法错误。
- 如仓库现有前端静态检查脚本可用，运行 `apps/agent-ide/scripts/smoke-test.mjs` 或同级最小检查脚本验证基础渲染不报错。
