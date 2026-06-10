# 修复 Agent 仍然没有写入工具计划

## 摘要

当前仓库里已经实现了 `write_file` 和 `create_document`，也完成了注册、权限和基础测试，但主 Agent 在实际对话时仍然可能“看不到”这些工具。已确认根因不在工具实现本身，而在“Composer 模式工具可见性链路”没有同步更新。

本次修复目标：

- 让主 Agent 在应支持写入的模式下，实际收到 `write_file` / `create_document` 工具 schema。
- 保持 `ask` 模式默认无写入工具，避免把解释型问答变成默认可修改模式。
- 为运行时工具暴露行为补充回归测试，防止再次出现“代码里有工具，但模型上下文里没有”的问题。

## 当前状态分析

### 已确认正常的部分

- `backend/src/agent_debug/domain/tools/workspace_tools.py`
  - 已实现 `WriteFileTool` 与 `CreateDocumentTool`。
  - 已在 `build_default_workspace_tools()` 中注册。
- `backend/src/agent_debug/domain/permission_service.py`
  - 已将 `create_document` 视为写入类工具。
  - `plan` 模式下已限制为仅允许 `.md`。
- `backend/src/agent_debug/api/rest_gateway.py`
  - `list_tools()` 会直接枚举 registry 中的工具，因此 REST 工具列表理论上能包含写入工具。
- `backend/src/agent_debug/prompts/builtin_subagents.py`
  - 通用子 Agent 的 `DEFAULT_WRITE_TOOLS` 已包含 `write_file` / `create_document`。
- 测试中已有：
  - 工具注册测试。
  - 工具执行测试。
  - 权限测试。

### 已确认的问题

- `backend/src/agent_debug/prompts/composer_mode_prompts.py`
  - `build` / `debug` / `multitask` / `plan` 的 `allowed_tools` 仍然只包含：
    - `read_file`
    - `list_dir`
    - `grep`
    - `write_todos`
  - 没有包含 `write_file` / `create_document`。
- `backend/src/agent_debug/api/rest_gateway.py`
  - `_ask_composer_message()` 虽然读取了 `resolve_composer_profile()` 返回的 `profile`，但当前仅使用了 `timeout_ms`。
  - `profile.allowed_tools` 没有传给 `runtime.run_composer_chat()`。
- `backend/src/agent_debug/domain/runtime.py`
  - `run_composer_chat()` 当前也没有接收“模式级工具覆盖”的参数。
  - 运行时最终默认使用的是会话级 allowlist，而不是 Composer 模式定义的 allowlist。

### 结论

当前“Agent 没有写入工具”的真实原因，不是写入工具未实现，而是主 Agent 对话链路没有把这些工具作为可用工具暴露给模型。

## 假设与决策

- 本次修复范围聚焦“主 Agent 工具可见性”。
- 保持 `ask` 模式为空工具集，不新增写入能力。
- `build`、`debug`、`multitask` 应允许看到写入工具。
- `plan` 配置文件中的 allowlist 也同步更新，保持模式定义一致；即便 `plan` 当前主流程走 `generate_plan + execute_plan`，也不保留过期配置。
- 不修改已有 `write_file` / `create_document` 的参数设计与权限规则。
- 不扩展新的写入工具种类，不引入 MCP 或前端配置页的额外功能。

## 方案设计

### 1. 修正 Composer 模式工具白名单

修改 `backend/src/agent_debug/prompts/composer_mode_prompts.py`：

- 为 action-oriented 模式提取一个共享工具集合常量，避免后续再次遗漏。
- 建议新增类似：
  - `read_file`
  - `list_dir`
  - `grep`
  - `write_file`
  - `create_document`
  - `write_todos`
- 将以下 profile 更新为该集合：
  - `_BUILD_PROFILE`
  - `_DEBUG_PROFILE`
  - `_MULTITASK_PROFILE`
  - `_PLAN_PROFILE`
- 保持 `_ASK_PROFILE.allowed_tools = frozenset()` 不变。

目的：

- 让模式定义与当前真实工具能力保持一致。
- 避免未来新增工具时继续散落在多个 profile 常量中手写维护。

### 2. 把模式 allowlist 真正传入运行时

修改 `backend/src/agent_debug/domain/runtime.py` 与 `backend/src/agent_debug/api/rest_gateway.py`：

#### `backend/src/agent_debug/domain/runtime.py`

- 扩展 `run_composer_chat()` 签名，增加可选参数，例如：
  - `allowed_tools_override: Optional[Sequence[str]] = None`
- 在 `run_composer_chat()` 调用 `_run_react_loop()` 时，将该参数透传下去。
- 复用 `_run_react_loop()` 已存在的 `allowed_tools_override` 逻辑，不重新发明一套过滤机制。

#### `backend/src/agent_debug/api/rest_gateway.py`

- 在 `_ask_composer_message()` 中把：
  - `profile.allowed_tools`
  - 以及当前会话级 allowlist 结果
  做一次交集/受限合并，再传给 `runtime.run_composer_chat()`。

推荐合并策略：

- 基础集合使用 `profile.allowed_tools`。
- 再受 `allowed_tool_names_for_session(session_id)` 约束。
- 最终传入的是两者交集。

原因：

- 模式 should decide “理论允许哪些工具”。
- 会话级 allowlist should decide “当前 session 是否还要额外禁用某些工具”。
- 这样既不会绕过模式限制，也不会绕过现有 session 级过滤规则。

### 3. 保持前端不改或最小改动

当前前端工具卡片映射已具备：

- `write_file`
- `create_document`

因此本次主问题不在前端展示，不需要新增 UI 功能。

仅在执行阶段如发现前端仍有错误文案或空标签，再做最小修正；计划默认不改前端。

## 具体改动清单

### `backend/src/agent_debug/prompts/composer_mode_prompts.py`

- 提取 action 模式共享工具集合常量。
- 将 `write_file` / `create_document` 加入 `build/debug/multitask/plan` 的 `allowed_tools`。
- 保持 `ask` 模式无工具。

### `backend/src/agent_debug/domain/runtime.py`

- 为 `run_composer_chat()` 增加 `allowed_tools_override` 参数。
- 透传到 `_run_react_loop()`。

### `backend/src/agent_debug/api/rest_gateway.py`

- 在 `_ask_composer_message()` 中计算最终允许工具集合。
- 将最终工具集合传给 `runtime.run_composer_chat()`。

### `backend/tests/agent_debug/test_web_search_tools.py`

- 复用现有“运行时对模型暴露工具列表”的测试风格，新增或扩展以下断言：
  - 主 Agent 在支持动作的模式下，发送给 provider 的 `tools` 中包含 `write_file`。
  - 同时包含 `create_document`。
  - `ask` 模式仍然保持空工具或不包含写入工具。

如果该文件不适合承载 Composer 模式测试，也可新建专门测试文件，但优先放在已有 runtime tool-exposure 风格测试附近，减少分散。

## 验证步骤

### 自动验证

- 运行受影响测试，至少包括：
  - `backend/tests/agent_debug/test_web_search_tools.py`
  - 与 Composer 运行时相关的新/旧测试文件
  - 如签名改动波及运行时，也补跑相关 `test_composer_chat_streaming.py` 中的受影响用例
- 确认新增断言能覆盖：
  - `build/debug/multitask` 可见写入工具
  - `ask` 不可见写入工具

### 手动验证

- 调用 `/api/agent-debug/tools`，确认 `write_file` / `create_document` 仍在工具列表中。
- 在主 Agent 对话中发起“创建 markdown 文档”类请求，确认模型不再声称“没有写入工具”。
- 观察事件流，确认出现真实的 `agent.tool.invoked` 写入事件，而不是只输出文本内容。

## 非目标

- 不重新设计 `write_file` / `create_document` 的参数。
- 不修改文件写入底层 `WorkspaceTreeService.write_text()`。
- 不新增新的文档格式或差量编辑工具。
- 不实现 MCP 写入能力。
- 不调整前端设置页的工具配置交互。
