# 修复 Build 模式看不到搜索工具计划

## Summary

- 目标：修复当前 Tavily 已配置、会话已开启 `联网搜索`、聊天实际处于 `build` 模式时，Agent 仍看不到 `web_search` / `web_fetch` 工具的问题。
- 本次排查后的明确根因：
  - 不是 `Ask` 模式误判。
  - 不是会话 `webSearchEnabled` 开关失效。
  - 不是 Tavily 配置未生效。
  - 而是 `build/debug/multitask/plan` 共用的 `_ACTION_MODE_TOOLS` 中根本没有 `web_search` / `web_fetch`，导致后端在模式级 allowlist 这一层就把搜索工具过滤掉了。

## Current State Analysis

### 1. Build 模式的工具集合缺少 web_search / web_fetch

- 文件：`backend/src/agent_debug/prompts/composer_mode_prompts.py`
  - `_ACTION_MODE_TOOLS` 当前包含：
    - `read_file`
    - `list_dir`
    - `grep`
    - `write_file`
    - `create_document`
    - `delete_file`
    - `run_command`
    - `check_command_status`
    - `stop_command`
    - `write_todos`
    - `Task`
  - 这里没有：
    - `web_search`
    - `web_fetch`

- 同文件：
  - `_BUILD_PROFILE.allowed_tools = _ACTION_MODE_TOOLS`
  - `_DEBUG_PROFILE.allowed_tools = _ACTION_MODE_TOOLS`
  - `_MULTITASK_PROFILE.allowed_tools = _ACTION_MODE_TOOLS`
  - `_PLAN_PROFILE.allowed_tools = _ACTION_MODE_TOOLS`
  - 所以这四类模式都会丢失搜索工具，不只是 `build`。

### 2. 后端最终工具列表是“模式 allowlist”与“会话 allowlist”的交集

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `_ask_composer_message()` 中：
    - `profile = resolve_composer_profile(runtime_mode)`
    - `session_allowed_tools = self.allowed_tool_names_for_session(session_id)`
    - `allowed_tools = [name for name in profile.allowed_tools if name in session_allowed_tools]`
  - 说明只要 `profile.allowed_tools` 不包含 `web_search`，即使会话允许，也不会进入最终工具列表。

### 3. 会话开关逻辑本身是正常的

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `allowed_tool_names_for_session()` 已确认：
    - `session.web_search_enabled == True` 时返回全部工具
    - 否则过滤掉 `web_search` / `web_fetch`
  - 这意味着：
    - 当前问题不是会话开关没打开
    - 而是模式级 allowlist 在更前面就漏掉了搜索工具

### 4. 前端发送的确实是当前 composerMode

- 文件：`apps/agent-ide/public/interactions.jsx`
  - 发送消息时：
    - `const backendMode = composerMode || "build";`
    - 然后调用 `askExecute(..., backendMode)`
  - 用户已明确说明当前就是 `build`，这与代码链路一致。

### 5. 现有测试没有覆盖 Build 模式下的搜索工具可见性

- 文件：`backend/tests/agent_debug/test_web_search_tools.py`
  - 当前 `test_rest_gateway_build_mode_passes_write_tools_to_runtime` 只断言：
    - `write_file`
    - `create_document`
    - `delete_file`
    - `run_command`
    - `check_command_status`
    - `stop_command`
    - `write_todos`
  - 但没有断言：
    - `web_search`
    - `web_fetch`
  - 所以这个回归一直没有被测试捕获。

## Assumptions & Decisions

- 决策 1：把 `web_search` / `web_fetch` 加入 `_ACTION_MODE_TOOLS`。
  - 原因：`build`、`debug`、`multitask`、`plan` 都属于行动型模式，联网搜索应作为可选工具存在。

- 决策 2：保留会话级 `webSearchEnabled` 作为最终开关。
  - 即使把搜索工具加入 `_ACTION_MODE_TOOLS`，仍由 `allowed_tool_names_for_session()` 决定当前会话是否真的暴露它们。
  - 这样不会破坏现有“未开启联网搜索时不暴露搜索工具”的行为边界。

- 决策 3：本次只修工具 allowlist，不改提示词文案。
  - 先确保模型能看到工具。
  - 若后续发现模型仍不主动调用，再考虑补系统提示词引导。

- 决策 4：本次不处理 `Ask` 模式工具策略。
  - 用户已确认实际场景是 `build`，因此先修真正命中的路径，避免扩大变更面。

## Proposed Changes

### 1. 扩展行动型模式工具集合

- 文件：`backend/src/agent_debug/prompts/composer_mode_prompts.py`
  - 在 `_ACTION_MODE_TOOLS` 中新增：
    - `web_search`
    - `web_fetch`
  - 影响范围：
    - `build`
    - `debug`
    - `multitask`
    - `plan`
  - 预期效果：
    - 只要会话开启了 `webSearchEnabled`，这些模式下模型就能看到搜索工具。

### 2. 保持 rest_gateway 的会话过滤逻辑不变

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `_ask_composer_message()` 与 `allowed_tool_names_for_session()` 不需要结构性改动。
  - 现有逻辑已经能保证：
    - 模式允许搜索工具
    - 但未开启会话联网搜索时，依然会自动过滤 `web_search` / `web_fetch`

### 3. 补齐 Build 模式测试覆盖

- 文件：`backend/tests/agent_debug/test_web_search_tools.py`
  - 更新 `test_rest_gateway_build_mode_passes_write_tools_to_runtime`
  - 增加断言：
    - 当会话 `webSearchEnabled=True` 时，`allowed_tools_override` 中包含：
      - `web_search`
      - `web_fetch`
  - 同时保留原有写工具断言，确保 build 模式功能不退化。

- 同文件新增一个回归测试：
  - 例如 `test_rest_gateway_build_mode_hides_web_tools_when_session_search_disabled`
  - 验证：
    - build 模式下，若当前会话 `webSearchEnabled=False`
    - 即使 `_ACTION_MODE_TOOLS` 已包含 `web_search/web_fetch`
    - 最终传给 runtime 的 `allowed_tools_override` 仍不包含它们

### 4. 可选补一个 profile 级单测

- 文件：`backend/tests/agent_debug/` 现有测试文件中就地增加即可
  - 直接验证 `resolve_composer_profile("build").allowed_tools` 包含：
    - `web_search`
    - `web_fetch`
  - 作用：当以后修改 `_ACTION_MODE_TOOLS` 时，能更早发现回归

## Verification Steps

1. 后端单测
   - 运行：
     - `backend/tests/agent_debug/test_web_search_tools.py`
   - 重点验证：
     - build 模式 + 会话开启搜索时，`allowed_tools_override` 包含 `web_search/web_fetch`
     - build 模式 + 会话关闭搜索时，`web_search/web_fetch` 仍被过滤

2. 手工联调
   - 配好 Tavily API Key
   - 打开当前会话的 `联网搜索`
   - 保持 composer mode 为 `build`
   - 再问一次“你现在能看到搜索工具了吗”
   - 预期：
     - 模型不再声称自己没有独立的联网搜索工具
     - 能看到 `web_search` / `web_fetch` 被纳入可用工具

3. 回归验证
   - 在未开启 `联网搜索` 的会话中，用 `build` 模式提问
   - 预期：
     - 仍保留本地读写和命令工具
     - 但不暴露 `web_search/web_fetch`

## Out Of Scope

- 不修改 `Ask` 模式的工具策略。
- 不修复前端顶部 `Hybrid` 文案与实际 composerMode 的展示关系。
- 不改 Tavily 配置页面。
- 不新增提示词级的“优先使用搜索工具”引导。
