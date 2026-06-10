# 为 Agent 制定命令工具计划

## 摘要

为当前项目的 Agent 增加一套可在 Windows 环境下运行的命令工具，采用一个通用执行入口为主，并补齐长时命令所需的状态轮询与停止能力。

本次计划明确采用以下方案：

- 主执行工具：`run_command`
- 状态查询工具：`check_command_status`
- 终止工具：`stop_command`
- `run_command` 支持 `powershell` 与 `bash` 等 shell 类型选择
- 首期即支持长时命令，且允许 Agent 基于 `command_id` 自主定时回看命令状态

## 当前状态分析

### 已存在能力

- `backend/src/agent_debug/domain/tools/workspace_tools.py` 当前只注册了文件/目录类工具：
  - `read_file`
  - `list_dir`
  - `grep`
  - `write_file`
  - `create_document`
  - `delete_file`
- `backend/src/agent_debug/domain/tools/__init__.py` 只导出了上述 workspace 工具，没有任何命令执行类工具。
- `backend/src/agent_debug/domain/permission_service.py` 已预留：
  - `run_command`
  - `shell`
  作为写/执行类工具；
  其中 `run_command` 已被视为危险工具，`auto` 模式默认拒绝。
- `backend/src/agent_debug/domain/runtime.py` 已支持通过 `tool_registry.json_schemas()` 将工具暴露给模型，并以 `agent.tool.invoked/completed/failed/denied` 统一发布事件，因此新增工具后无需改主循环框架。
- `backend/src/agent_debug/api/rest_gateway.py` 的 `list_tools()` 会直接枚举注册表，因此新工具注册后会自动出现在 `/api/agent-debug/tools`。
- `apps/agent-ide/public/components.jsx` 已有工具卡片图标/短语映射机制，可直接为命令类工具补显示文案。
- `backend/tests/agent_debug/test_web_search_tools.py`、`test_subagent_task_tool.py`、`test_infra_hardening.py` 已存在针对工具 allowlist、模式权限、运行时可见性的回归测试模式，可复用。

### 当前缺口

- 仓库里没有任何真实的命令执行工具实现，也没有命令状态查询/终止工具。
- 仓库里没有现成的命令进程注册表或命令生命周期管理层。
- 当前虽然有 `run_command` 权限名，但没有与之对应的工具、参数 schema、输出结构、前端文案与测试。
- 若只实现“同步执行命令”，无法满足用户已经确认的“支持长时命令，并支持 Agent 自主设计定时回看”要求。

### 结论

本次不是简单新增一个 `shell` 工具，而是要补一套最小命令编排闭环。仅实现 `run_command` 不足以满足需求，必须同时设计命令状态存储和后续轮询工具。

## 假设与决策

### 产品形态

- 采用“一个通用命令执行工具”而不是拆成多个 `bash` / `powershell` 工具。
- 通过 `shell` 参数选择解释器，而不是把 shell 类型编码进工具名。
- 为满足长时命令场景，额外新增两个工具：
  - `check_command_status`
  - `stop_command`

### Shell 范围

- Windows 环境下默认 shell 为 `powershell`。
- `run_command.shell` 首期建议支持：
  - `powershell`
  - `bash`
- 若后续实现成本可控，可同时预留 `cmd`；但本次计划以 `powershell` / `bash` 为必须项。
- `bash` 的底层策略采用“显式寻找可执行 bash（如 Git Bash / WSL bash）”；若运行环境中不存在，返回结构化错误，而不是静默降级到 PowerShell。

### 生命周期设计

- `run_command` 同时支持短时命令和长时命令。
- 使用参数区分运行模式，建议采用：
  - `blocking: true` 表示等待命令结束并直接返回完整结果；
  - `blocking: false` 表示启动后台命令并尽快返回 `command_id`。
- 当 `blocking: false` 时，必须把子进程状态登记到一个命令注册表中，以供后续轮询和停止。
- Agent 的“定时回看”不需要在后端内建 cron 或定时器；由模型在后续回合重复调用 `check_command_status(command_id=...)` 即可达成。换言之，后端提供轮询能力，调度策略交给 Agent。

### 安全与权限

- `run_command` 继续作为危险工具：
  - `plan` 模式拒绝
  - `auto` 模式默认拒绝
  - `bypass` 模式允许
- `check_command_status` 作为只读工具，加入 `READ_ONLY_TOOLS`。
- `stop_command` 作为危险工具，加入 `MUTATING_TOOLS` 与 `DANGEROUS_TOOLS`。
- 本次不做复杂命令沙箱、不做网络/路径白名单、不做资源配额治理；仅在工作区目录下执行，并通过 shell 选择和错误回传保持行为透明。

## 方案设计

### 1. 新增命令运行域模型与注册表

新增后端域层模块，建议文件：

- `backend/src/agent_debug/domain/command_runner.py`

职责：

- 维护命令生命周期与进程句柄；
- 生成稳定的 `command_id`；
- 启动 PowerShell / Bash 子进程；
- 存储执行元数据与最新输出摘要；
- 查询状态；
- 停止进程。

建议抽象：

- `CommandRecord`
  - `id`
  - `session_id`
  - `shell`
  - `command`
  - `cwd`
  - `status`（`running` / `completed` / `failed` / `terminated`）
  - `exit_code`
  - `stdout`
  - `stderr`
  - `started_at`
  - `ended_at`
- `CommandRunnerService`
  - `start_command(...)`
  - `run_blocking(...)`
  - `get_status(command_id, session_id)`
  - `stop(command_id, session_id)`

实现原则：

- 使用 `subprocess.Popen` 或等价方案启动进程；
- 工作目录固定为当前 workspace root，或允许可选 `cwd` 参数但必须校验位于 workspace 内；
- `blocking=false` 时返回后立即登记记录；
- `check_command_status` 负责读取进程当前状态并在进程结束后补齐退出码和输出；
- `stop_command` 负责终止存活进程并更新记录状态。

### 2. 新增工具实现

建议新增工具模块：

- `backend/src/agent_debug/domain/tools/command_tools.py`

包含三个工具类：

#### `RunCommandTool`

- 工具名：`run_command`
- 参数建议：
  - `command`: string，必填
  - `shell`: string，可选，枚举 `powershell` / `bash`
  - `blocking`: boolean，可选，默认 `true`
  - `cwd`: string，可选，工作区内相对路径
  - `timeout_seconds`: integer，可选，仅对阻塞模式生效
- 阻塞模式返回：
  - `commandId`
  - `status`
  - `exitCode`
  - `stdout`
  - `stderr`
- 非阻塞模式返回：
  - `commandId`
  - `status: running`
  - `pid`（若可安全暴露）
  - `shell`
  - `cwd`

#### `CheckCommandStatusTool`

- 工具名：`check_command_status`
- 参数建议：
  - `command_id`: string，必填
  - `tail_lines`: integer，可选，用于限制最近输出，避免上下文过大
- 返回：
  - `commandId`
  - `status`
  - `exitCode`
  - `stdoutTail`
  - `stderrTail`
  - `running`
  - `endedAt`

#### `StopCommandTool`

- 工具名：`stop_command`
- 参数建议：
  - `command_id`: string，必填
- 返回：
  - `commandId`
  - `stopped: true`
  - `status`

### 3. 接入工具注册表

修改：

- `backend/src/agent_debug/domain/tools/__init__.py`
- `backend/src/agent_debug/domain/tools/workspace_tools.py`

具体策略：

- 不建议把命令工具继续塞进 `workspace_tools.py`，避免文件职责继续膨胀；
- 改为在 `build_default_workspace_tools()` 中引入并注册 `RunCommandTool`、`CheckCommandStatusTool`、`StopCommandTool`；
- 同步更新 `__init__.py` 导出和模块说明文案。

### 4. 权限层更新

修改：

- `backend/src/agent_debug/domain/permission_service.py`

具体改动：

- 将 `check_command_status` 加入 `READ_ONLY_TOOLS`；
- 将 `stop_command` 加入 `MUTATING_TOOLS`；
- 将 `stop_command` 加入 `DANGEROUS_TOOLS`；
- 保留 `run_command` 在 `MUTATING_TOOLS` / `DANGEROUS_TOOLS` 中的现有定位；
- `plan` 模式下：
  - 允许 `check_command_status`
  - 拒绝 `run_command`
  - 拒绝 `stop_command`

### 5. 暴露给 Composer 与子代理

修改：

- `backend/src/agent_debug/prompts/composer_mode_prompts.py`
- `backend/src/agent_debug/prompts/builtin_subagents.py`

具体改动：

- 在 `_ACTION_MODE_TOOLS` 中加入：
  - `run_command`
  - `check_command_status`
  - `stop_command`
- 在 `DEFAULT_WRITE_TOOLS` 中加入命令工具，使通用写型子代理具备自主执行命令、轮询结果和停止进程的能力；
- 只读画像仍不新增 `run_command` / `stop_command`；
- 若需保守，可让 `check_command_status` 出现在部分只读场景，但本次计划以保持只读画像最小权限为主。

### 6. 前端展示

修改：

- `apps/agent-ide/public/components.jsx`

具体改动：

- 在 `TOOL_ICON_MAP` 中新增：
  - `run_command`
  - `check_command_status`
  - `stop_command`
- 在 `TOOL_PHRASE` 中新增：
  - `run_command`: `执行命令`
  - `check_command_status`: `查看命令状态`
  - `stop_command`: `停止命令`
- 复用现有 `argSummary()` 对 `command` 或 `command_id` 的摘要逻辑；必要时扩展优先取 `args.command`。

如需更好可读性，可在后续补一轮 `interactions.jsx` 微调，但本次计划不把它设为必须改动项，优先确保卡片能正确显示。

### 7. REST 与会话影响

修改：

- `backend/src/agent_debug/api/rest_gateway.py`

本次主要影响：

- `/api/agent-debug/tools` 会因注册表自动新增三项工具，无需单独接口；
- 若后续前端需要直接查看命令记录，可再扩 REST；
- 本次首期不要求新增独立 REST API，因为 Agent 工具调用即可覆盖主要使用路径。

### 8. 测试策略

重点补充以下测试文件：

- `backend/tests/agent_debug/test_infra_hardening.py`
  - `plan` 模式拒绝 `run_command`
  - `plan` 模式允许 `check_command_status`
  - `auto` 模式拒绝 `run_command` / `stop_command`
  - allowlist 后允许 `run_command` / `stop_command`

- `backend/tests/agent_debug/test_web_search_tools.py`
  - `allowed_tools_override` 能带出命令工具
  - build 模式默认允许列表包含新命令工具

- `backend/tests/agent_debug/test_subagent_task_tool.py`
  - 通用 action 模式与 `DEFAULT_WRITE_TOOLS` 中能看到新命令工具

- 新增建议测试文件：
  - `backend/tests/agent_debug/test_command_tools.py`

覆盖：

- `run_command` 阻塞模式成功执行 PowerShell 命令
- `run_command` 非阻塞模式返回 `command_id`
- `check_command_status` 能读到运行中/已结束状态
- `stop_command` 能终止长时命令
- `bash` 不存在时返回结构化错误
- 越界 `cwd` 被拒绝

## 具体改动清单

### `backend/src/agent_debug/domain/command_runner.py`

- 新增命令运行/状态管理服务。
- 保存命令记录、进程句柄和状态查询逻辑。

### `backend/src/agent_debug/domain/tools/command_tools.py`

- 新增 `RunCommandTool`
- 新增 `CheckCommandStatusTool`
- 新增 `StopCommandTool`

### `backend/src/agent_debug/domain/tools/workspace_tools.py`

- 在默认注册表里接入命令工具注册。
- 如需最小改动，可直接在此处导入并注册；但不再把命令实现写进本文件。

### `backend/src/agent_debug/domain/tools/__init__.py`

- 导出新的命令工具类。
- 更新模块说明。

### `backend/src/agent_debug/domain/permission_service.py`

- 补 `check_command_status` / `stop_command` 的权限分类。

### `backend/src/agent_debug/prompts/composer_mode_prompts.py`

- 将三项命令工具加入 action 模式 allowlist。

### `backend/src/agent_debug/prompts/builtin_subagents.py`

- 将命令工具加入通用写型子代理默认工具集。

### `apps/agent-ide/public/components.jsx`

- 补图标与中文短语映射。
- 需要时补 `argSummary()` 对 `command` / `command_id` 的摘要支持。

### `backend/tests/agent_debug/test_command_tools.py`

- 新增命令工具核心测试。

### `backend/tests/agent_debug/test_infra_hardening.py`

- 扩权限测试。

### `backend/tests/agent_debug/test_web_search_tools.py`

- 扩运行时 allowlist 可见性测试。

### `backend/tests/agent_debug/test_subagent_task_tool.py`

- 扩 action 模式与子代理工具集测试。

## 验证步骤

### 自动验证

- 运行新增和受影响测试：
  - `backend/tests/agent_debug/test_command_tools.py`
  - `backend/tests/agent_debug/test_infra_hardening.py`
  - `backend/tests/agent_debug/test_web_search_tools.py`
  - `backend/tests/agent_debug/test_subagent_task_tool.py`
- 确认：
  - 阻塞命令通过
  - 非阻塞命令可返回 `command_id`
  - `check_command_status` 可读到状态变化
  - `stop_command` 可终止后台命令
  - 权限模式符合预期

### 手动验证

- 通过 `/api/agent-debug/tools` 确认能看到：
  - `run_command`
  - `check_command_status`
  - `stop_command`
- 在 build/debug 模式下让 Agent 执行一个短命令，确认卡片展示正常。
- 在 build/debug 模式下让 Agent 启动一个长时命令，再多轮调用 `check_command_status`，确认可回看状态。
- 验证 `bash` 在不可用环境中返回明确错误，而不是无响应或错误 shell。
- 验证 `plan` 模式下 `run_command` / `stop_command` 被拒绝。

## 非目标

- 不实现完整终端 UI 或交互式 TTY。
- 不实现复杂流式日志订阅 WebSocket。
- 不实现命令持久化到磁盘后的跨重启恢复。
- 不实现资源配额、沙箱或命令审计系统。
- 不实现细粒度 shell profile 管理页面。
