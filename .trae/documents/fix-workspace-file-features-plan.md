# 工程文件/工作区功能补全修复计划

## Summary

- 目标：修复 `apps/agent-ide` 中“打开文件”“打开文件夹/工作区”“打开最近”“另存为”等工程相关能力不可用或行为不一致的问题。
- 成功标准：
  - `File -> 打开文件…` 可打开任意绝对路径文件，而不再被当前工作区根目录限制。
  - `File -> 打开文件夹…` 与侧边栏 `Open Workspace` 都能可靠切换工作区，并驱动右侧工作区树立即刷新。
  - “打开最近”中的绝对路径文件可重新打开；另存为到任意绝对路径可正常写入。
  - Tauri 桌面壳与浏览器降级模式都具备一致的文件/工作区语义；无法支持的场景要给出明确提示，而不是静默失败。

## Current State Analysis

### 前端入口

- 菜单定义在 `apps/agent-ide/public/menu-schema.jsx`。
- 文件读写与标签页逻辑在 `apps/agent-ide/public/main.jsx`，核心入口是 `openFileFromPath()`、`saveTab()`、`handleOpenWorkspace()`。
- Tauri 桥接在 `apps/agent-ide/public/tauri-bridge.jsx`，包含 `openFile()`、`openFolder()`、`saveAs()`、`switchWorkspaceRoot()`。
- 浏览器/IDE HTTP bridge 在 `apps/agent-ide/public/api-client.jsx`，当前只暴露工作区内文件的读写接口。

### 已确认问题

- `openFileFromPath()` 在 Tauri 下调用 `read_text_file`，该 Rust 命令通过 `resolve_within_root()` 强制要求目标文件位于当前工作区根目录内；因此“打开文件…”虽然能选出任意绝对路径，但随后读取会被拒绝。
- `saveTab()` 在 Tauri 下调用 `write_text_file`，同样受工作区根目录约束；所以“另存为”到工作区外路径会失败。
- “打开最近”复用 `openFileFromPath()`，因此最近文件列表中的工作区外绝对路径也无法重新打开。
- `tauri-bridge.jsx` 中 `switchWorkspaceRoot()` 会吞掉 `MoonlitAgentApi.setWorkspaceRoot()` 的异常，仅打印 `console.warn`；这会造成桌面侧根目录已切换，但后端工作区树未同步时，界面看起来像“打开文件夹没生效”。
- 浏览器降级模式当前只有 `readWorkspaceFile()` / `writeWorkspaceFile()`，语义也是“仅允许工作区内路径”；这与“打开文件支持任意绝对路径”的目标不一致。

### 约束与保留项

- 工作区树枚举、工作区内回滚、工作区目录读写仍应继续限制在当前工作区根内，不能因为支持外部文件打开而放开整个工作区服务。
- Tauri 与后端都保留现有 5 MB 文本文件大小限制与“目录不能按文件读取”保护。
- 不新增第三方依赖，沿用现有 Tauri 原生命令与 backend FastAPI/网关结构。

## Proposed Changes

### 1. 前端统一“工作区文件”和“任意本地文件”语义

修改文件：

- `apps/agent-ide/public/main.jsx`
- `apps/agent-ide/public/menu-schema.jsx`

计划内容：

- 在 `main.jsx` 中新增统一的路径判定与文件读写入口：
  - 工作区内路径继续走现有 workspace API。
  - 任意绝对路径优先走 Tauri unrestricted 文件接口；在非 Tauri 场景走后端新增的本地文件接口。
- 重构 `openFileFromPath()`：
  - 允许绝对路径直接打开，不再无条件走“工作区内文件读取”命令。
  - 保留相对路径对当前工作区根的支持，避免破坏 README/工作区树等既有入口。
  - 打开成功后统一写入最近文件列表。
- 重构 `saveTab()`：
  - 对已打开的绝对路径文件执行正常保存。
  - “另存为”选择任意绝对路径后，按新路径语义选择正确写入通道。
- 在 `menu-schema.jsx` 中保持菜单动作调用新语义入口，并补足错误提示文案：
  - “打开文件…”失败时提示“超出工作区限制”之类旧错误不再出现。
  - “打开文件夹…”在后端同步失败时显示明确错误，而不是只提示成功。

### 2. 修复工作区切换的同步与反馈

修改文件：

- `apps/agent-ide/public/tauri-bridge.jsx`
- `apps/agent-ide/public/main.jsx`

计划内容：

- 调整 `switchWorkspaceRoot()`：
  - 不再吞掉 `MoonlitAgentApi.setWorkspaceRoot()` 异常。
  - 返回统一成功/失败结果，让菜单入口与侧边栏入口都能正确提示。
- 在 `handleOpenWorkspace()` 与菜单 `open_folder` 逻辑中：
  - 只在 Tauri 与后端根目录都完成同步后才显示“已切换工作区”。
  - 显式触发并等待 `__moonlit_refreshWorkspaceTree()`，确保右侧树和顶部状态立即刷新。

### 3. 为桌面壳补充“不受工作区根限制”的本地文本文件命令

修改文件：

- `apps/agent-ide/src-tauri/src/lib.rs`

计划内容：

- 新增专用命令，例如：
  - `read_any_text_file(path)`
  - `write_any_text_file(path, content)`
- 这些命令只负责“任意绝对路径的文本文件读写”，不参与工作区树根目录管理。
- 约束：
  - 要求目标是文件而非目录。
  - 继续复用 5 MB 上限。
  - 对写入目标创建父目录时保持现有行为。
- 保留现有 `read_text_file` / `write_text_file` 语义不变，使工作区内能力与安全边界不受影响。

### 4. 为浏览器/后端模式补充本地文件读写 API

修改文件：

- `backend/src/agent_debug/domain/workspace_tree.py`
- `backend/src/agent_debug/api/rest_gateway.py`
- `backend/src/agent_debug/server.py`
- `apps/agent-ide/public/api-client.jsx`

计划内容：

- 在后端新增一组“本地文件”接口，避免复用现有 workspace-only 语义：
  - 读取任意绝对路径文本文件。
  - 写入任意绝对路径文本文件。
- 设计原则：
  - 仅接受绝对路径，避免与工作区相对路径语义混淆。
  - 复用现有文件大小、目录检查、错误包装风格。
  - 不让本地文件 API 影响 `workspace/info`、`workspace/tree`、`workspace/revert` 这些工作区能力。
- 在 `api-client.jsx` 中新增对应方法，并由 `main.jsx` 在非 Tauri 或需要统一回退时调用。

### 5. 补齐回归验证

修改文件：

- `backend/tests/agent_debug/test_rest_gateway.py`
- `backend/tests/agent_debug/test_server.py`
- `apps/agent-ide/scripts/smoke-test.mjs`

计划内容：

- 后端网关测试：
  - 覆盖“任意绝对路径文本文件读写”成功案例。
  - 覆盖目录路径、相对路径、超限文件等失败案例映射。
- 后端接口测试：
  - 覆盖新增 HTTP 路由的成功与错误返回。
- 前端 smoke 检查：
  - 确认 `api-client.jsx` 暴露新的本地文件接口。
  - 确认 `main.jsx` 使用新的统一文件读写入口，而不是把“打开文件…”继续绑死到 workspace-only API。

## Assumptions & Decisions

- 决策：把“工作区树/工程视图”和“任意本地文件编辑”拆成两套语义，而不是直接放开现有 workspace-only 命令。
- 决策：桌面端与浏览器端都支持“打开任意绝对路径文件”；浏览器端由于没有原生文件选择器桥接，继续允许通过输入绝对路径的方式访问。
- 决策：工作区切换必须同时同步 Tauri 状态和 backend workspace root；任一失败都视为本次切换失败并反馈给用户。
- 假设：当前问题主要集中在“功能链路不完整/语义冲突”，不涉及菜单 UI 本身点击事件失效；现有 `MenuBar` 渲染逻辑可继续复用。
- 假设：不引入最近工作区列表；本次只修复现有“最近文件”与工作区切换能力。

## Verification Steps

### 代码级验证

- 运行前端静态检查：`apps/agent-ide` 现有 `npm test` / `node scripts/smoke-test.mjs`。
- 运行后端测试：聚焦 `backend/tests/agent_debug/test_rest_gateway.py` 与 `test_server.py`。
- 编辑完成后检查被改文件诊断，确保没有新增语法或 linter 错误。

### 手工验证

- 桌面壳：
  - `File -> 打开文件…` 选择工作区外绝对路径文件，确认可打开成标签页。
  - 修改该文件后直接保存，确认可写回原路径。
  - 对 untitled 文件执行“另存为…”，选择工作区外新路径，确认保存成功。
  - `File -> 打开最近` 重新打开刚才的绝对路径文件，确认成功。
  - `File -> 打开文件夹…` 选择新目录，确认右侧工作区树与工作区根信息立即刷新。
- 浏览器/降级模式：
  - 输入工作区外绝对路径打开文件，确认走后端新增本地文件接口成功。
  - 切换工作区后，`workspace/info` 与 `workspace/tree` 返回内容一致更新。
