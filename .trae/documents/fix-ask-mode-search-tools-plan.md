# 修复 Ask 模式看不到搜索工具计划

## Summary

- 目标：修复当前已配置 Tavily 且会话已开启联网搜索后，Agent 在 `Ask` 模式下仍看不到 `web_search` / `web_fetch` 工具的问题。
- 已确认的根因：
  - 后端 `ask` 模式的 `allowed_tools` 当前被配置为空集合。
  - 聊天发送链路会把前端 `composerMode` 直接透传给后端，后端再按该模式裁剪工具列表。
  - 会话级 `webSearchEnabled` 开关与 Tavily 配置本身已经正常接入，不是本次核心故障点。
- 本次修复方向已确认：
  - 保持 `Ask` 模式，但让它支持“搜索类只读工具”。
  - 至少让 `Ask` 可见并可调用 `web_search`、`web_fetch`，并与当前只读代码查询工具保持一致。

## Current State Analysis

### 1. Ask 模式在后端被硬编码为无工具

- 文件：`backend/src/agent_debug/prompts/composer_mode_prompts.py`
  - `ComposerModeProfile.allowed_tools` 决定某个 composer mode 最终允许暴露哪些工具。
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
  - `_ASK_PROFILE` 当前定义为：
    - `allowed_tools=frozenset()`
  - 这意味着后端无论会话是否开启联网搜索，`Ask` 模式默认都不会给模型任何工具。

### 2. 聊天发送链路会直接使用 composerMode

- 文件：`apps/agent-ide/public/interactions.jsx`
  - 发送消息时：
    - `const backendMode = composerMode || "build";`
    - 然后调用 `window.MoonlitAgentApi.askExecute(sessionId, text, contextWindow, backendMode)`
  - 说明只要前端当前 `composerMode` 是 `ask`，后端就一定会按 `ask` 配置裁剪工具。

- 文件：`apps/agent-ide/public/api-client.jsx`
  - `askExecute()` 会把 `composerMode` 原样发到后端 `/ask:execute`。

### 3. 后端确实会按模式和会话双重过滤工具

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `_ask_composer_message()` 里会：
    - `profile = resolve_composer_profile(runtime_mode)`
    - `session_allowed_tools = self.allowed_tool_names_for_session(session_id)`
    - `allowed_tools = [name for name in profile.allowed_tools if name in session_allowed_tools]`
  - 说明最终可见工具 = “模式允许的工具” 与 “当前会话允许的工具” 的交集。

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `allowed_tool_names_for_session()` 已确认：
    - 会话 `web_search_enabled=True` 时，返回全部工具名
    - 否则会过滤掉 `web_search` / `web_fetch`
  - 所以联网搜索配置正常时，只差 `Ask` 模式把这些工具纳入 allowlist。

### 4. 现有测试显式要求 Ask 模式工具为空

- 文件：`backend/tests/agent_debug/test_web_search_tools.py`
  - 现有测试 `test_rest_gateway_ask_mode_keeps_tools_empty` 明确断言：
    - `captured["allowed_tools_override"] == []`
  - 这是本次修复后必然需要更新的测试。

## Assumptions & Decisions

- 决策 1：`Ask` 模式改为“只读工具模式”，而不是“无工具模式”。
  - 原因：用户明确希望在当前问答场景直接使用搜索工具，而不是切去其它模式。

- 决策 2：`Ask` 模式只开放只读工具，不开放写文件/命令执行/任务写入类工具。
  - 推荐纳入：
    - `read_file`
    - `list_dir`
    - `grep`
    - `web_search`
    - `web_fetch`
  - 明确保留禁用：
    - `write_file`
    - `create_document`
    - `delete_file`
    - `run_command`
    - `check_command_status`
    - `stop_command`
    - `write_todos`
    - `Task`
  - 原因：这样既满足问答与联网检索，又不破坏 `Ask` 模式“偏解释、偏只读”的边界。

- 决策 3：本次不修顶部 `Hybrid` 文案与真实 `composerMode` 的显示脱节。
  - 原因：用户本次选择的是“Ask 也允许搜索”，而不是修 UI 展示。
  - 这可以作为后续单独问题处理，但不纳入此次计划。

- 决策 4：系统提示词暂不改为强鼓励 `Ask` 模式使用工具。
  - 只修改允许工具集即可，先确保模型“能看到工具”。
  - 若后续发现模型仍不倾向调用，再考虑追加 ask-mode 的工具使用提示。

## Proposed Changes

### 1. 新增 Ask 模式专用只读工具集合

- 文件：`backend/src/agent_debug/prompts/composer_mode_prompts.py`
  - 新增一个只读工具集合，例如 `_ASK_MODE_TOOLS`，内容为：
    - `read_file`
    - `list_dir`
    - `grep`
    - `web_search`
    - `web_fetch`
  - 与 `_ACTION_MODE_TOOLS` 分开定义，避免误把写操作能力引入 `Ask`。

- 同文件：
  - 把 `_ASK_PROFILE.allowed_tools` 从空集合改成 `_ASK_MODE_TOOLS`。

### 2. 保持后端会话级搜索开关逻辑不变

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `_ask_composer_message()` 与 `allowed_tool_names_for_session()` 不需要结构性改动。
  - 现有逻辑已经能保证：
    - `Ask` 模式允许的只读工具先进入候选集合
    - 若当前会话 `webSearchEnabled=false`，仍会自动过滤掉 `web_search` / `web_fetch`
  - 这样可以保持“Ask 可支持搜索，但仍受会话级开关控制”的行为边界。

### 3. 更新后端测试，验证 Ask 模式暴露只读搜索工具

- 文件：`backend/tests/agent_debug/test_web_search_tools.py`
  - 替换现有 `test_rest_gateway_ask_mode_keeps_tools_empty`
  - 改成新的断言，例如：
    - `Ask` 模式下 `allowed_tools_override` 至少包含：
      - `read_file`
      - `list_dir`
      - `grep`
      - `web_search`
      - `web_fetch`
    - 且不包含：
      - `write_file`
      - `create_document`
      - `delete_file`
      - `run_command`
      - `write_todos`
      - `Task`

- 如果当前测试更适合拆分为两条：
  - `test_rest_gateway_ask_mode_exposes_readonly_tools`
  - `test_rest_gateway_ask_mode_excludes_write_tools`
  - 则按现有测试风格拆开也可以。

### 4. 补一条会话开关回归验证

- 文件：`backend/tests/agent_debug/test_web_search_tools.py`
  - 追加一个针对 `Ask` 模式 + 会话禁用联网搜索的回归测试，验证：
    - `Ask` 模式下仍保留 `read_file/list_dir/grep`
    - 但会话 `webSearchEnabled=false` 时不出现 `web_search/web_fetch`
  - 目的：确保本次修复不会绕过已有的会话级联网开关。

## Verification Steps

1. 后端单测
   - 运行：
     - `backend/tests/agent_debug/test_web_search_tools.py`
   - 核对：
     - `Ask` 模式现在能拿到只读工具集合
     - `web_search/web_fetch` 仍受 `webSearchEnabled` 控制

2. 手工联调
   - 在前端把 Tavily API Key 配好
   - 打开当前会话状态栏的 `联网搜索`
   - 保持 composer mode 为 `Ask`
   - 发送“你现在能看到搜索工具了吗”这类问题
   - 预期：
     - 模型不再回复“没有独立的联网搜索工具”
     - 能看到 `web_search` / `web_fetch` 出现在工具可见集合里，并可被调用

3. 回归验证
   - 切回未开启 `联网搜索` 的会话
   - 在 `Ask` 模式下再次提问
   - 预期：
     - 仍然只有本地只读工具
     - 不会错误暴露 `web_search/web_fetch`

## Out Of Scope

- 不修复顶部 `Hybrid` 标签与实际 `composerMode` 的显示不一致问题。
- 不改 `Debug` / `Plan` / `Multitask` 模式的工具策略。
- 不新增 `Ask` 模式的额外系统提示词引导。
- 不改前端 `composerMode` 的持久化机制 `moonlit:mode`。
