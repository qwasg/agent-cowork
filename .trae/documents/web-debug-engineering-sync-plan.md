# 网页调试模式工程功能同步计划

## Summary

- 目标：让 `apps/agent-ide` 在网页调试模式下，也能使用与桌面壳对应的工程能力，至少覆盖：
  - 打开文件
  - 打开文件夹 / 工作区
  - 新建文件
  - 保存 / 另存为
  - 工作区树浏览与从树中打开文件
- 用户已明确的产品方向：
  - 网页端优先使用浏览器原生文件选择器
  - 浏览器兼容范围以 Chromium 为主
  - 新建文件 / 另存为 要能保存到真实本地文件
  - 文件 / 工作区句柄需要“刷新后也尽量恢复”

## Current State Analysis

### 当前前端状态

- 菜单定义在 `apps/agent-ide/public/menu-schema.jsx`。
- 文件标签页、打开/保存逻辑、快捷键和工作区入口都在 `apps/agent-ide/public/main.jsx`。
- Tauri 能力封装在 `apps/agent-ide/public/tauri-bridge.jsx`。
- 网页模式下当前只有两类能力：
  - 后端 REST 工作区能力：`workspaceInfo` / `workspaceTree` / `setWorkspaceRoot`
  - 后端本地文件读写：`readLocalFile` / `writeLocalFile`
- 网页模式目前没有任何浏览器原生文件系统 API 实现：
  - 未发现 `showOpenFilePicker`
  - 未发现 `showSaveFilePicker`
  - 未发现 `showDirectoryPicker`
  - 未发现 `<input type="file">` / `webkitdirectory`

### 当前行为边界

- `打开文件…` 在 `menu-schema.jsx` 中已被收敛成仅 `Tauri` 桌面壳可用；网页端只提示不可用。
- `打开文件夹…` 同样仅 `Tauri` 桌面壳可用。
- `新建文件` 在 `main.jsx` 中现在被改成桌面壳 `saveAs` 对话框新建真实文件；网页端会直接报“仅 Tauri 桌面壳支持”。
- `另存为…` 仍依赖 `window.tauriBridge.saveAs()`，网页端不可用。
- 右侧 `WorkspaceTreePanel` 当前完全依赖后端 `workspaceInfo()` 与 `workspaceTree()` 返回数据；网页端若不用后端路径模式，就没有浏览器侧目录树数据源。

### 当前可复用能力

- 网页端已经有“按绝对路径读写本地文件”的后端 API：
  - `apps/agent-ide/public/api-client.jsx`
  - `backend/src/agent_debug/server.py`
  - `backend/src/agent_debug/domain/workspace_tree.py`
- 文件标签页模型本身可扩展；当前 `createFileTab()` 只存路径、内容、语言、dirty 等字段，没有浏览器句柄字段。
- 最近文件当前通过 `useStickyState("moonlit:recentFiles")` 做了本地持久化，但只保存路径字符串，不支持浏览器文件句柄。

### 已确认缺口

- 网页端没有浏览器原生文件/目录选择器接入层。
- 网页端没有“浏览器文件句柄 -> 打开/保存/另存为”的统一读写路径。
- 网页端没有“浏览器目录句柄 -> 工作区树”的数据适配层。
- 仓库中未发现现成的 `indexedDB` / `idb` 句柄持久化工具，因此“刷新后尽量恢复”需要新增浏览器端持久化实现。

## Proposed Changes

### 1. 新增浏览器文件系统桥接层

修改文件：

- `apps/agent-ide/public/tauri-bridge.jsx`
- `apps/agent-ide/public/main.jsx`
- 必要时新增 `apps/agent-ide/public/browser-fs.jsx`

计划内容：

- 把当前“桌面壳能力”和“网页原生文件系统能力”从概念上拆开：
  - `Tauri` 继续负责桌面端系统对话框
  - 新增浏览器桥接层负责 Chromium File System Access API
- 浏览器桥接层应封装至少这些能力：
  - `isAvailable()`
  - `openFile() -> FileSystemFileHandle`
  - `saveAs() -> FileSystemFileHandle`
  - `openDirectory() -> FileSystemDirectoryHandle`
  - `readFile(handle)`
  - `writeFile(handle, content)`
  - `queryPermission(handle)` / `requestPermission(handle)`
- 统一错误语义：
  - 用户取消选择
  - 浏览器不支持
  - 权限被拒绝
  - 句柄失效 / 恢复失败

### 2. 统一前端“文件来源”模型

修改文件：

- `apps/agent-ide/public/main.jsx`

计划内容：

- 扩展当前 `fileTabs` 模型，使一个文件标签不仅能表示 `path`，还要能表示浏览器句柄来源。
- 为文件标签补充可区分来源的字段，例如：
  - `source: "tauri" | "workspace" | "local-api" | "browser-handle"`
  - `browserHandle`
  - `browserHandleKey`
  - `browserPathLabel`
- 重构 `openFileFromPath()` 和 `writeOut()` 为更抽象的入口，拆成：
  - 按路径打开/保存
  - 按浏览器句柄打开/保存
- `新建文件`、`另存为`、`打开最近`、编辑器保存快捷键都要复用统一入口，避免网页端出现特例分支。

### 3. 网页端接入浏览器原生文件对话框

修改文件：

- `apps/agent-ide/public/menu-schema.jsx`
- `apps/agent-ide/public/main.jsx`

计划内容：

- `打开文件…`
  - Tauri：继续走 `tauri.openFile()`
  - 网页 Chromium：走 `showOpenFilePicker()`
  - 读出内容后以真实文件标签打开
- `新建文件`
  - Tauri：继续走 `saveAs`
  - 网页 Chromium：走 `showSaveFilePicker()`，得到文件句柄后创建空文件并打开
- `另存为…`
  - Tauri：继续走 `saveAs`
  - 网页 Chromium：对当前标签执行 `showSaveFilePicker()`，并把后续保存目标切换为新句柄
- `Ctrl+N` / `Ctrl+O`
  - 按当前运行环境分别命中 Tauri 或浏览器桥接
- 非支持环境的网页模式
  - 不再静默失败；明确提示“当前浏览器不支持原生文件系统 API”

### 4. 网页端工作区切换与工作区树适配

修改文件：

- `apps/agent-ide/public/main.jsx`
- 必要时新增 `apps/agent-ide/public/browser-workspace.jsx`

计划内容：

- 现有 `WorkspaceTreePanel` 只会调用后端 `workspaceInfo()` / `workspaceTree()`，需要改造成双数据源：
  - Tauri / 后端工作区：继续走现有 REST 数据
  - 浏览器工作区：基于 `FileSystemDirectoryHandle` 递归列目录
- 浏览器工作区需要提供与现有 UI 相同的数据结构，至少包含：
  - `name`
  - `kind`
  - `relPath`
  - `hasChildren` 或等价展开能力
- `打开文件夹…`
  - Tauri：继续走 `openFolder() + setWorkspaceRoot()`
  - 网页 Chromium：走 `showDirectoryPicker()`，并把该目录设为前端当前工作区
- 从工作区树点击文件时：
  - 若来源是浏览器工作区，使用目录句柄递归解析目标文件句柄并打开
- 刷新按钮与 `__moonlit_refreshWorkspaceTree`
  - 需要在浏览器工作区模式下也能刷新目录内容

### 5. 句柄持久化与“刷新后尽量恢复”

修改文件：

- 新增浏览器端持久化模块，例如 `apps/agent-ide/public/browser-handle-store.jsx`
- `apps/agent-ide/public/main.jsx`
- `apps/agent-ide/public/menu-schema.jsx`

计划内容：

- 使用 `indexedDB` 存储：
  - 最近文件句柄元数据
  - 最近工作区目录句柄元数据
  - 当前活动浏览器工作区句柄
- 页面启动时尝试恢复：
  - 最近文件列表的显示项
  - 最近工作区 / 当前工作区
  - 当前已打开标签的可恢复句柄
- 恢复策略：
  - 先恢复句柄对象
  - 再调用 `queryPermission` 检查权限
  - 权限不足时向用户提示重新授权
- “尽量恢复”意味着：
  - 能恢复则恢复
  - 不能恢复时清理失效句柄并保留可读提示，不阻塞应用启动

### 6. 最近文件与标签恢复语义更新

修改文件：

- `apps/agent-ide/public/main.jsx`
- `apps/agent-ide/public/menu-schema.jsx`

计划内容：

- 当前 `recentFiles` 只保存路径字符串，需要扩展为可表示两类来源：
  - 路径型最近文件
  - 浏览器句柄型最近文件
- 菜单“打开最近”要能区分：
  - Tauri / 后端路径文件
  - 浏览器句柄文件
- 恢复失败时菜单项要么提示重新授权，要么自动清理失效项，避免点了无反馈。

### 7. 测试与静态校验补充

修改文件：

- `apps/agent-ide/scripts/smoke-test.mjs`
- 必要时新增前端单测脚本

计划内容：

- smoke 规则补充以下约束：
  - 存在浏览器原生文件系统桥接入口
  - `menu-schema.jsx` 不再把网页端直接全部判成“仅 Tauri 可用”
  - `main.jsx` 中 `新建文件` / `打开文件` / `另存为` 支持浏览器句柄分支
  - `WorkspaceTreePanel` 支持浏览器工作区数据源
  - 存在 `indexedDB` 句柄持久化实现
- 如测试结构允许，补最小化逻辑单测：
  - 最近文件序列化/反序列化
  - 句柄权限恢复失败时的降级行为

## Assumptions & Decisions

- 决策：网页调试模式优先采用 Chromium 的 File System Access API，而不是继续依赖后端绝对路径模式作为主交互。
- 决策：`打开文件` / `新建文件` / `另存为` 在网页端都面向“真实本地文件”，不是内存标签页加导出。
- 决策：工作区树在网页端也要可用，因此不能只修单文件打开；需要补浏览器目录句柄到树形 UI 的完整链路。
- 决策：句柄恢复目标是“尽量恢复”，不是强保证；恢复失败时允许提示重新授权并清理失效数据。
- 假设：当前项目允许在 React state 中暂存 `FileSystemFileHandle` / `FileSystemDirectoryHandle` 对象，同时用 `indexedDB` 做跨刷新恢复。
- 假设：以 Chromium 为主意味着可以接受 Safari / Firefox 下功能不完整，只需给出明确提示。

## Verification Steps

### 代码级验证

- 确认新增浏览器文件系统桥接层存在，并暴露打开/保存/目录选择/读写/权限查询能力。
- 确认 `menu-schema.jsx` 中网页端对：
  - `打开文件`
  - `打开文件夹`
  - `新建文件`
  - `另存为`
  已有浏览器分支，而非统一提示 Tauri-only。
- 确认 `main.jsx` 中文件标签模型已支持浏览器句柄来源。
- 确认 `WorkspaceTreePanel` 不再只依赖后端 `workspaceInfo/workspaceTree`。
- 确认存在 `indexedDB` 相关实现，用于句柄恢复。

### 运行验证

- 运行前端静态检查：`apps/agent-ide` 下现有 `node scripts/smoke-test.mjs`
- 如修改影响桌面壳桥接兼容性，补跑 `apps/agent-ide/src-tauri` 下 `cargo check`
- 编辑完成后对最近修改文件运行诊断，确保无新增语法错误

### 手工验收

- 网页 Chromium 模式：
  - 点击 `打开文件…`，弹出浏览器原生文件选择器并成功打开文件
  - 点击 `新建文件`，弹出浏览器原生保存对话框，选定后创建真实文件并打开
  - 修改后执行保存，内容写回真实本地文件
  - 点击 `另存为…`，可以切换到新的真实文件目标
  - 点击 `打开文件夹…`，弹出目录选择器并刷新右侧工作区树
  - 从工作区树点击文件，能正确打开
- 刷新页面后：
  - 最近文件 / 最近工作区尽量恢复
  - 若浏览器需要重新授权，界面有明确提示而非静默失败
