# 为当前项目的 Agent 新增删除文件 Tool 计划

## 摘要

为当前项目的 Agent 补齐原生 `delete_file` 工具，使其能够在工作区内安全删除文件，并把该能力贯通到：

- 工作区文件系统服务；
- 默认工具注册表；
- Composer / 子代理可见工具列表；
- 前端工具卡片文案；
- 权限与测试回归。

本次范围聚焦“删除文件”，不扩展到删除目录、不做通用文件管理器能力。

## 当前状态分析

### 已确认的现状

- `backend/src/agent_debug/domain/tools/workspace_tools.py` 已实现并注册 `read_file`、`list_dir`、`grep`、`write_file`、`create_document`，但没有 `delete_file`。
- `backend/src/agent_debug/domain/workspace_tree.py` 提供了 `read_text()`、`write_text()`、`checkout_head()` 等能力，但没有任何工作区内删除文件的方法。
- `backend/src/agent_debug/domain/permission_service.py` 已经把 `delete_file` 放进 `MUTATING_TOOLS` 和 `DANGEROUS_TOOLS`，说明权限层预期这个工具会存在，但当前没有实现。
- `backend/src/agent_debug/prompts/composer_mode_prompts.py` 的 `_ACTION_MODE_TOOLS` 当前只暴露 `read_file`、`list_dir`、`grep`、`write_file`、`create_document`、`write_todos`、`Task`，没有 `delete_file`。
- `backend/src/agent_debug/prompts/builtin_subagents.py` 的 `DEFAULT_WRITE_TOOLS` 也没有 `delete_file`，因此通用子代理即使未来有工具实现，也拿不到删除能力。
- `apps/agent-ide/public/components.jsx` 当前只给 `read_file`、`list_dir`、`grep`、`write_file`、`create_document` 配了图标和中文短语，没有 `delete_file`。
- `backend/tests/agent_debug/test_workspace_tools.py`、`test_infra_hardening.py`、`test_subagent_task_tool.py`、`test_web_search_tools.py` 已覆盖现有工具的注册、权限和允许列表，但没有删除工具回归。

### 结论

当前问题不是“前端没显示”，而是后端从底层文件操作到工具注册都缺少 `delete_file`，只有权限常量提前预留了名字。因此要实现“确保其能删除文件”，必须同时补齐底层能力、工具层、工具暴露和测试。

## 假设与决策

- 工具名采用现有权限层已经预留的 `delete_file`，避免新增命名分叉。
- 首期只支持删除“文件”，不支持删除目录；若传入目录路径，返回结构化错误。
- 允许传入工作区相对路径；若现有工具风格允许绝对路径但仍位于工作区内，则沿用 `WorkspaceTreeService._resolve_any()` 的包含关系校验。
- 不新增“强制删除”“递归删除”“回收站”“软删除”语义，行为为直接删除。
- 若目标文件不存在，返回结构化 `PATH_NOT_FOUND` 错误，和现有读写工具保持一致。
- 计划模式下 `delete_file` 仍应被拒绝；当前权限服务的默认逻辑已经满足这一点，因此以补测试为主，不额外改权限策略。
- 自动模式下 `delete_file` 继续视为危险工具，需要确认；当前权限常量已经满足这一点。

## 方案设计

### 1. 补齐底层删除能力

修改 `backend/src/agent_debug/domain/workspace_tree.py`：

- 新增工作区内文件删除方法，建议命名为 `delete_file()`。
- 方法职责：
  - 使用现有 `_resolve_any()` 解析路径并校验必须位于当前工作区内；
  - 若路径不存在，抛出 `FileNotFoundError`；
  - 若目标是目录，抛出 `IsADirectoryError`；
  - 对文件执行 `unlink()`；
  - 返回结构化结果，至少包含 `path` 与 `deleted: True`。
- 设计理由：
  - 复用现有工作区边界约束，保持与 `read_text()` / `write_text()` 的安全模型一致；
  - 让工具层只负责参数和错误码映射，不直接操作 `Path`，避免逻辑散落。

### 2. 新增并注册 Agent 删除工具

修改 `backend/src/agent_debug/domain/tools/workspace_tools.py`：

- 新增 `DeleteFileTool`：
  - `name = "delete_file"`；
  - `description` 明确声明“删除工作区内单个文件，不支持目录”；
  - `parameters` 采用与 `read_file` / `write_file` 一致的单参数风格，只要求 `path`。
- `run()` 中：
  - 使用 `_str_arg(args, "path")` 做参数解析；
  - 调用 `workspace.delete_file(rel_path)`；
  - 将底层异常映射为统一的 `ToolExecutionError`：
    - `FileNotFoundError` -> `PATH_NOT_FOUND`
    - `IsADirectoryError` -> `PATH_IS_DIRECTORY`
    - `ValueError` -> `PATH_OUTSIDE_ROOT`
    - `OSError` -> `FILESYSTEM_ERROR`
  - 返回简洁文本摘要，例如 `deleted <path>`。
- 在 `build_default_workspace_tools()` 中注册 `DeleteFileTool`，放在 `write_file` / `create_document` 附近，保持工作区文件操作工具聚合。
- 同步更新文件头注释中的内置工具列表，确保模块说明与实际一致。

### 3. 补齐工具导出与可见工具集

修改 `backend/src/agent_debug/domain/tools/__init__.py`：

- 导出 `DeleteFileTool`；
- 更新模块说明里的内置工具列表，把 `delete_file` 纳入对外说明。

修改 `backend/src/agent_debug/prompts/composer_mode_prompts.py`：

- 在 `_ACTION_MODE_TOOLS` 中加入 `delete_file`，让 build/debug/multitask/plan 运行模式可以把该工具暴露给模型。

修改 `backend/src/agent_debug/prompts/builtin_subagents.py`：

- 在 `DEFAULT_WRITE_TOOLS` 中加入 `delete_file`，使通用子代理和未来写型画像能使用删除能力；
- 保持 `DEFAULT_READONLY_TOOLS` 不变，避免只读画像误拿删除能力。

### 4. 前端工具卡片展示

修改 `apps/agent-ide/public/components.jsx`：

- 在 `TOOL_ICON_MAP` 中为 `delete_file` 增加图标映射；
- 在 `TOOL_PHRASE` 中增加 `delete_file: "删除文件"`；
- 保持 `argSummary()` 复用 `args.path` 的现有逻辑，无需额外修改。

这样在工具执行事件流里，删除动作能以可读标签显示，而不是退回通用 `wrench`。

### 5. 权限与 REST 影响

权限层：

- `backend/src/agent_debug/domain/permission_service.py` 当前已包含：
  - `delete_file` 属于 `MUTATING_TOOLS`
  - `delete_file` 属于 `DANGEROUS_TOOLS`
- 本次不需要改权限主逻辑，只补测试验证：
  - `plan` 模式拒绝 `delete_file`
  - `auto` 模式默认拒绝 `delete_file`
  - allowlist 后可放行

REST 层：

- `backend/src/agent_debug/api/rest_gateway.py` 的 `list_tools()` 会直接枚举注册表。
- 只要 `DeleteFileTool` 被注册，`/api/agent-debug/tools` 就会自动返回它，无需额外改接口代码。

## 具体改动清单

### `backend/src/agent_debug/domain/workspace_tree.py`

- 新增 `delete_file()`，负责工作区内文件删除及基础异常抛出。

### `backend/src/agent_debug/domain/tools/workspace_tools.py`

- 新增 `DeleteFileTool`。
- 在 `build_default_workspace_tools()` 中注册 `delete_file`。
- 更新模块头部说明文字。

### `backend/src/agent_debug/domain/tools/__init__.py`

- 导出 `DeleteFileTool`。
- 更新内置工具列表说明。

### `backend/src/agent_debug/prompts/composer_mode_prompts.py`

- 在 `_ACTION_MODE_TOOLS` 中加入 `delete_file`。

### `backend/src/agent_debug/prompts/builtin_subagents.py`

- 在 `DEFAULT_WRITE_TOOLS` 中加入 `delete_file`。

### `apps/agent-ide/public/components.jsx`

- 新增 `delete_file` 的图标和中文短语映射。

### `backend/tests/agent_debug/test_workspace_tools.py`

- 新增 `delete_file` 成功删除文件测试。
- 新增 `delete_file` 传目录时报 `PATH_IS_DIRECTORY` 测试。
- 新增默认 registry 包含 `delete_file` 测试。
- 新增 REST `list_tools()` 返回 `delete_file` 测试。

### `backend/tests/agent_debug/test_infra_hardening.py`

- 增加权限断言：
  - `plan` 模式下 `delete_file` 被拒绝；
  - `auto` 模式下 `delete_file` 默认被拒绝；
  - `always_allow()` 后 `delete_file` 被允许。

### `backend/tests/agent_debug/test_subagent_task_tool.py`

- 更新 `DEFAULT_WRITE_TOOLS` / `session_tools` 相关断言，把 `delete_file` 纳入写型工具集可见性验证。

### `backend/tests/agent_debug/test_web_search_tools.py`

- 更新 `allowed_tools_override` 相关测试夹具和断言，确保运行时允许列表能带出 `delete_file`。

## 验证步骤

### 自动验证

- 运行后端受影响测试：
  - `backend/tests/agent_debug/test_workspace_tools.py`
  - `backend/tests/agent_debug/test_infra_hardening.py`
  - `backend/tests/agent_debug/test_subagent_task_tool.py`
  - `backend/tests/agent_debug/test_web_search_tools.py`
- 确认新增删除工具不会破坏现有 `write_file` / `create_document` 相关断言。

### 手动验证

- 调用 `/api/agent-debug/tools`，确认返回中包含 `delete_file`。
- 在 build/debug 模式触发一次删除文件工具调用，确认前端展示“删除文件”卡片及路径摘要。
- 验证删除一个真实文件后文件已不存在。
- 验证对目录路径调用时得到结构化错误，而不是静默失败。
- 验证 plan 模式下模型即使请求 `delete_file`，也会被权限层拒绝。

## 非目标

- 不实现删除目录或递归删除。
- 不实现回收站、撤销删除或软删除。
- 不新增新的 MCP 文件系统工具。
- 不修改现有 git 回滚能力 `checkout_head()` 的语义。
- 不引入新的第三方依赖。
