# 为 Agent 配置创建文档和写入工具计划

## 摘要

为当前 Agent 工具体系补齐“双工具”写入能力：

- 新增通用文件写入工具，用于在工作区内创建或覆盖文本文件。
- 新增文档创建工具，用于明确表达“创建 Markdown/文本文档”的意图，降低模型误用通用写入工具的概率。
- 将新工具接入现有注册表、运行时权限、工具列表接口与前端展示文案。
- 保持现有工作区边界校验、事件流与 plan 模式权限约束不变，不扩展到通用 MCP server 管理实现。

## 当前状态分析

### 已存在的能力

- 后端已具备 Agent 工具注册与调度能力，入口在 `backend/src/agent_debug/domain/tools/base.py` 与 `backend/src/agent_debug/domain/runtime.py`。
- 默认 workspace 工具当前只有 `read_file`、`list_dir`、`grep`、`write_todos`，定义在 `backend/src/agent_debug/domain/tools/workspace_tools.py`。
- 底层文件系统服务 `WorkspaceTreeService` 已具备可复用的 `write_text()` 能力，且会校验路径必须位于工作区内，定义在 `backend/src/agent_debug/domain/workspace_tree.py`。
- 后端已有 `/api/agent-debug/tools` 接口可枚举工具元信息，定义在 `backend/src/agent_debug/api/rest_gateway.py` 与 `backend/src/agent_debug/server.py`。
- 前端已能自动展示 `agent.tool.invoked/completed/failed/denied` 事件，但当前仅对少数工具配置了图标与中文短语，相关代码在 `apps/agent-ide/public/components.jsx` 与 `apps/agent-ide/public/interactions.jsx`。
- 权限系统已预留 `write_file`、`edit_file` 等写入类工具名；`plan` 模式下只允许写 `.md`，定义在 `backend/src/agent_debug/domain/permission_service.py`。

### 当前缺口

- `workspace_tools.py` 没有实际注册任何文件写入工具，导致 Agent 只能“读/搜”，不能直接创建文档或写文件。
- 权限系统虽然认识 `write_file` 名称，但没有对应已注册工具，形成“权限已留白、能力未落地”的状态。
- 前端工具卡片没有为“写文件/创建文档”配置图标与中文短语，新增后将显示为通用扳手，辨识度较低。
- 设置页中的“工具与 MCP”页面目前主要是说明与占位，不需要本次实现真实 MCP 管理，但需要保证新工具至少能出现在 `/api/agent-debug/tools` 枚举结果中，供前端与后续配置读取。

## 假设与决策

- 需求范围采用“双工具”方案：同时提供通用写入工具和文档创建工具。
- 文档工具首期只处理文本内容，不做富文本、模板生成、Frontmatter 自动补全或多格式导出。
- 两个工具都限制为文本写入，不处理二进制文件。
- 两个工具都复用 `WorkspaceTreeService.write_text()`，不额外新增新的落盘层。
- 文档工具以 Markdown 为默认目标，但仍允许显式创建 `.txt` / `.md` 这类文本文件；若路径不是文档型扩展名，则返回参数错误。
- 本次不新增 `edit_file` 差量编辑能力；若需要修改已有文件，先以覆盖式 `write_file` 为第一阶段能力。
- 本次不实现真实“用户自定义 MCP 服务接入”，仅补齐 Agent 原生工具。
- 前端以“展示与可理解性”增强为目标，不新增复杂交互入口，不在设置页中增加真实启停开关。

## 方案设计

### 1. 后端工具层

在 `backend/src/agent_debug/domain/tools/workspace_tools.py` 中新增两个工具类，并在默认注册表中注册：

- `WriteFileTool`
  - 名称：`write_file`
  - 作用：在工作区内创建或覆盖 UTF-8 文本文件。
  - 入参：
    - `path`: 工作区相对路径，必填。
    - `content`: 要写入的文本内容，必填。
  - 行为：
    - 复用 `_str_arg()` 做参数校验。
    - 调用 `workspace.write_text(path, content)` 落盘。
    - 捕获并映射 `FileNotFoundError`、`IsADirectoryError`、`ValueError`、`TypeError`、`OSError` 为稳定 `ToolExecutionError`。
    - 返回结构化结果，至少包含 `path`、`bytesWritten`，并在 `text` 中返回简短摘要。

- `CreateDocumentTool`
  - 名称：`create_document`
  - 作用：创建或覆盖工作区中的文档文件，强调“文档”语义，优先服务 Markdown 写作场景。
  - 入参：
    - `path`: 工作区相对路径，必填。
    - `content`: 文档正文，必填。
  - 校验：
    - 仅允许文档型扩展名，如 `.md`、`.markdown`、`.txt`。
    - 若扩展名不符合，抛出 `TOOL_INVALID_ARGS`，提示使用文档扩展名。
  - 行为：
    - 底层同样调用 `workspace.write_text()`。
    - 返回结构化结果，附带 `kind: "document"` 或同类标识，便于后续 UI 做语义展示。

同时更新 `build_default_workspace_tools()` 注册顺序，建议为：

- `read_file`
- `list_dir`
- `grep`
- `write_file`
- `create_document`
- `write_todos`

### 2. 工具导出与描述

更新 `backend/src/agent_debug/domain/tools/__init__.py`：

- 导出 `WriteFileTool`、`CreateDocumentTool`。
- 更新文件头说明，使内置 workspace 工具描述与实际一致。

这样可保证后续代码引用与测试导入保持统一。

### 3. 权限与运行时行为

更新 `backend/src/agent_debug/domain/permission_service.py`：

- 将 `create_document` 视为写入类工具，加入 `MUTATING_TOOLS`。
- 在 `plan` 模式下，对 `create_document` 应按“仅允许 `.md`”约束处理。
- 保持 `write_file` 现有逻辑：`plan` 模式下仅允许 `.md` 路径。
- 若 `create_document` 的扩展名允许 `.txt`，也不要在 `plan` 模式放宽到 `.txt`；`plan` 仍只允许 `.md`，以符合系统约束。

无需修改 `runtime.py` 的主流程；它已经能：

- 自动校验 allowlist。
- 走权限服务拒绝或放行。
- 统一发布 `agent.tool.*` 事件。

只需确保新增工具名能够被 `tool_registry` 自动枚举即可。

### 4. REST 与前端可见性

后端：

- `rest_gateway.list_tools()` 无需新增接口；只要工具注册完成，`/api/agent-debug/tools` 会自动带出新工具。

前端：

更新 `apps/agent-ide/public/components.jsx` 中的工具映射：

- `TOOL_ICON_MAP`
  - 为 `write_file` 配置更贴切的图标，例如 `file-pen` 或当前图标集合中最接近“写入”的图标。
  - 为 `create_document` 配置文档类图标。
- `TOOL_PHRASE`
  - `write_file`: `写入文件`
  - `create_document`: `创建文档`
- `argSummary()`
  - 保持现有逻辑即可，因为已有 `args.path` 优先展示路径。

可选微调：

- 若当前图标集不存在理想图标，可退回通用 `file-text` / `edit` 风格，避免额外引入资源。
- 不改动 `interactions.jsx` 的事件归并逻辑，因为现有逻辑已经支持新增工具事件卡片。

### 5. 测试

扩展 `backend/tests/agent_debug/test_infra_hardening.py`，并按已有风格补充后端单测：

- `WorkspaceTreeService.write_text()` 已有间接覆盖，可新增工具级测试而非重复测底层。
- 新增权限测试：
  - `plan` 模式下 `create_document` 对 `notes.md` 放行。
  - `plan` 模式下 `create_document` 对 `notes.txt` 拒绝。
- 新增工具执行测试，建议新建或补到现有测试文件：
  - `write_file` 能创建新文件并返回 `bytesWritten`。
  - `write_file` 对目录路径报错。
  - `create_document` 能创建 `.md` 文件。
  - `create_document` 对 `.py` 等非文档扩展名报 `TOOL_INVALID_ARGS`。
- 新增 registry 测试：
  - `build_default_workspace_tools()` 返回的 registry 包含 `write_file`、`create_document`。

如现有测试布局更适合，也可拆分到新的 `test_workspace_tools.py`，但优先保持与当前测试组织风格一致，避免无谓扩散。

## 具体改动清单

### `backend/src/agent_debug/domain/tools/workspace_tools.py`

- 新增 `WriteFileTool`。
- 新增 `CreateDocumentTool`。
- 新增文档扩展名白名单常量。
- 在 `build_default_workspace_tools()` 中注册两个新工具。
- 复用现有错误映射风格，保证运行时事件与前端错误展示一致。

### `backend/src/agent_debug/domain/tools/__init__.py`

- 导出 `WriteFileTool`、`CreateDocumentTool`。
- 更新模块说明中的内置工具列表。

### `backend/src/agent_debug/domain/permission_service.py`

- 把 `create_document` 纳入写入类工具。
- 在 `plan` 模式分支中显式处理 `create_document` 的 Markdown 写入许可。

### `apps/agent-ide/public/components.jsx`

- 为 `write_file`、`create_document` 增加图标映射。
- 为 `write_file`、`create_document` 增加中文短语。

### `backend/tests/agent_debug/test_infra_hardening.py`

- 补充 `create_document` 的权限测试。

### `backend/tests/agent_debug/...`

- 增加或补充 workspace tool 相关测试，覆盖写入成功、路径错误、扩展名错误和注册结果。
- 执行时根据现有测试文件结构选择最小变更位置。

## 验证步骤

### 自动验证

- 运行后端相关测试，仅覆盖受影响范围：
  - 权限测试。
  - workspace tools 测试。
- 确认新增测试通过且未破坏现有 `build_default_workspace_tools()` 相关断言。

### 手动验证

- 调用 `/api/agent-debug/tools`，确认返回中包含：
  - `write_file`
  - `create_document`
- 在一个会话里触发 Agent 工具调用，确认前端过程卡片显示：
  - 正确中文标签。
  - 正确路径摘要。
  - 成功/失败结果可读。
- 将会话切换到 `plan` 权限模式后验证：
  - `create_document` 写入 `.md` 被允许。
  - `write_file` 写入 `.py` 被拒绝。

## 非目标

- 不实现真正的用户自定义 MCP server 连接、持久化和运行。
- 不新增二进制文件写入能力。
- 不实现 patch/diff 级别的精细编辑工具。
- 不修改前端设置页中的 MCP 原型表单数据结构。
- 不引入新的第三方依赖。
