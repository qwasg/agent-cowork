# Tauri 系统文件对话框修复计划

## Summary

- 目标：修复 `apps/agent-ide` 中“打开文件仍然失败”的问题，并把“新建文件”改成直接调用系统保存对话框选择物理路径，而不是只生成本地未保存标签页。
- 本次范围只覆盖 `Tauri` 桌面壳。
- 成功标准：
  - `File -> 打开文件…` 在桌面壳中直接弹出系统文件选择器，并成功打开所选文件。
  - `File -> 打开文件夹…` 与 `另存为…` 继续走系统文件对话框，不再退回 `prompt`。
  - `File -> 新建文件` 与 `Ctrl+N` 改成直接弹出系统保存对话框；用户选定目标路径后立即创建并打开该物理文件。
  - 当应用不在 `Tauri` 桌面壳时，不再使用 `prompt` 冒充系统文件打开器，而是明确提示“仅桌面壳支持”或禁用相关能力。

## Current State Analysis

### 已确认的实际实现

- 菜单定义在 `apps/agent-ide/public/menu-schema.jsx`。
- 当前 `open_file` 菜单项已尝试调用 `tauri.openFile()`，但如果 `tauriBridge.isAvailable()` 返回 `false`，会退回：
  - `window.prompt("输入文件路径...")`
- 当前 `open_folder` 也有同样的 `prompt` 兜底。
- 当前 `saveTab()` 在 `apps/agent-ide/public/main.jsx` 中对 `另存为…` 使用 `window.tauriBridge.saveAs()`；若 `Tauri` 不可用，也退回 `prompt`。
- 当前 `newFileTab()` 在 `apps/agent-ide/public/main.jsx` 中只是创建一个 `untitled-N.txt` 标签页，不会触发系统文件对话框，也不会立即落盘。

### 已确认的 Tauri 桌面能力

- `apps/agent-ide/src-tauri/tauri.conf.json` 已启用 `app.withGlobalTauri = true`。
- `apps/agent-ide/src-tauri/capabilities/default.json` 已授予：
  - `dialog:allow-open`
  - `dialog:allow-save`
- `apps/agent-ide/src-tauri/src/lib.rs` 已注册 `tauri_plugin_dialog::init()`。
- `apps/agent-ide/public/tauri-bridge.jsx` 当前通过 `window.__TAURI__.core.invoke` 调用：
  - `plugin:dialog|open`
  - `plugin:dialog|save`

### 当前问题判断

- 代码表面上已经在用系统文件对话框，因此“打开文件仍然失败”更可能不是缺少 dialog 插件，而是以下链路之一有问题：
  - `tauriBridge.isAvailable()` 判定不可靠，导致错误退回到 `prompt`
  - 菜单动作/快捷键仍保留浏览器降级分支，掩盖了桌面壳真实故障
  - “新建文件”的行为定义本身就与用户预期不一致，它目前不是“新建到磁盘”
- 当前桌面壳与浏览器模式混用了一套菜单行为，导致“系统文件打开器”和“手动输入路径”两种交互语义混在一起，这是需要收敛的核心问题。

## Proposed Changes

### 1. 收敛文件对话框能力到 Tauri 桌面壳

修改文件：

- `apps/agent-ide/public/tauri-bridge.jsx`
- `apps/agent-ide/public/menu-schema.jsx`
- `apps/agent-ide/public/main.jsx`

计划内容：

- 在 `tauri-bridge.jsx` 中重构 `isAvailable()` 与对话框调用封装：
  - 明确把“能否调用系统文件对话框”定义为桌面壳能力，而不是泛化成浏览器兜底。
  - 增加更稳健的 Tauri 可用性检测与统一错误信息，避免表面不可用时静默退回 `prompt`。
- 在 `menu-schema.jsx` 中移除 `open_file` / `open_folder` 的 `prompt` 分支：
  - 桌面壳：直接调用系统打开器。
  - 非桌面壳：直接提示“仅 Tauri 桌面壳支持”，不再假装支持。
- 在 `main.jsx` 中移除 `saveAs()` 的 `prompt` 分支：
  - 本次范围内只保证桌面壳，非桌面壳不再提供伪系统对话框体验。

### 2. 把“新建文件”改成真正的新建到磁盘

修改文件：

- `apps/agent-ide/public/main.jsx`
- `apps/agent-ide/public/menu-schema.jsx`
- `apps/agent-ide/public/components.jsx`

计划内容：

- 以当前 `newFileTab()` 为基础拆分成两类动作：
  - 内部保留一个“创建标签页”的底层能力，供已有编辑器逻辑复用。
  - 新的对外 `新建文件` 动作改为：
    1. 调用系统 `save` 对话框让用户选择目标路径
    2. 以空内容创建该文件
    3. 将其作为已落盘文件标签页打开
- `Ctrl+N` 与菜单中的“新建文件”统一绑定到这个新行为。
- `components.jsx` 中的快捷键说明需要同步，避免文案仍暗示“新建未保存标签页”。

### 3. 明确“打开文件”和“打开工作区”的职责边界

修改文件：

- `apps/agent-ide/public/menu-schema.jsx`
- `apps/agent-ide/public/main.jsx`

计划内容：

- `打开文件…` 只负责调用系统文件打开器并打开单个文件。
- `打开文件夹…` / 侧边栏工作区入口只负责调用系统文件夹选择器并切换工作区。
- 相关错误提示改成可区分的桌面壳提示：
  - “无法调用系统文件选择器”
  - “当前不在 Tauri 桌面壳”
  - “用户取消选择”
- 避免把“打开文件失败”误导成“项目内文件创建/工作区限制”问题。

### 4. 清理不符合本次目标的浏览器降级逻辑

修改文件：

- `apps/agent-ide/public/menu-schema.jsx`
- `apps/agent-ide/public/main.jsx`
- `apps/agent-ide/scripts/smoke-test.mjs`

计划内容：

- 去掉或封装掉 `window.prompt(...)` 形式的文件/文件夹/保存兜底。
- smoke 检查改为保证：
  - `打开文件` 走 `tauri.openFile()`
  - `打开文件夹` 走 `tauri.openFolder()`
  - `新建文件` / `另存为` 走 `tauri.saveAs()`
  - 菜单层不再保留 prompt 分支

### 5. 补充桌面壳回归验证

修改文件：

- `apps/agent-ide/scripts/smoke-test.mjs`
- 必要时补充前端侧无浏览器降级分支的静态校验

计划内容：

- 增加针对菜单/快捷键绑定的静态校验：
  - `Ctrl+N` 不再绑定到旧 `newFileTab()` 行为
  - `open_file` 不再出现 `window.prompt`
  - `saveAs` 路径选择必须来自系统保存对话框
- 如现有测试结构允许，补一条最小化的桌面壳行为断言，确保对话框调用入口不再退化。

## Assumptions & Decisions

- 决策：本次只修 `Tauri` 桌面壳，不处理浏览器模式的文件选择体验。
- 决策：非桌面壳下不再使用 `prompt` 模拟系统文件打开器。
- 决策：`新建文件` 的产品语义改为“先选磁盘路径，再创建文件”，而不是“新建未保存标签页”。
- 假设：当前主要断点在前端桌面壳能力检测与错误兜底分支，而不是 `tauri_plugin_dialog` 权限本身缺失；因为配置、权限和插件注册都已存在。
- 假设：工作区树、最近文件、文件内容读写主链路可以继续复用现有实现；本次优先修正“如何选择路径”和“动作定义”。

## Verification Steps

### 代码级验证

- 检查 `menu-schema.jsx` 中：
  - `open_file`
  - `open_folder`
  - `new_file`
  - `save_as`
  不再包含 `window.prompt` 分支。
- 检查 `main.jsx` 中：
  - `Ctrl+N` 绑定已从旧 `newFileTab()` 切换到“系统保存对话框 + 创建文件”的新流程。
- 检查 `tauri-bridge.jsx` 中：
  - 对 `plugin:dialog|open`
  - `plugin:dialog|save`
  的调用入口清晰、错误处理统一。

### 运行验证

- 前端静态校验：`apps/agent-ide` 下现有 `node scripts/smoke-test.mjs`
- 如修改触及 Tauri 桥接：运行桌面端编译检查 `cargo check`
- 编辑完成后检查最近修改文件诊断，确保无新增语法错误

### 手工验收

- 桌面壳下点击 `File -> 打开文件…`
  - 必须弹出系统文件选择器
  - 选择文本文件后应直接打开为标签页
- 桌面壳下点击 `File -> 新建文件`
  - 必须先弹出系统保存对话框
  - 选择路径后应创建空文件并直接打开
- 桌面壳下按 `Ctrl+N`
  - 行为必须与“新建文件”菜单一致
- 桌面壳下点击 `另存为…`
  - 必须走系统保存对话框
- 在非桌面壳环境下触发这些动作
  - 不应再出现手输路径 `prompt`
  - 应明确提示该能力仅桌面壳支持
