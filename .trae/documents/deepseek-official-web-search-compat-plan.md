# 为 Agent 适配 DeepSeek 联网搜索兼容方案计划

## Summary

- 目标不是接入 DeepSeek 网页端的私有“联网搜索”开关，而是在继续使用 DeepSeek 官方 `chat/completions` 模型能力的前提下，为当前 Agent 增加独立的 `web_search` / `web_fetch` 能力，并提供会话级开关。
- 方案选择基于两点现实约束：
  - 当前仓库里的 DeepSeek 已通过 OpenAI 兼容协议接入，核心落点在 `backend/src/agent_debug/provider/openai_compat_adapter.py`。
  - DeepSeek 官方公开文档已明确说明“联网搜索已上线网页端，但 API 暂不支持搜索功能”；因此不能把方案建立在不存在的公开 API 字段上。
- 本次规划采用“兼容方案 + 会话开关 + 可配置搜索 API”：
  - DeepSeek 继续负责推理、工具调用和回答整合。
  - Agent 后端新增联网搜索工具，由模型自主调用。
  - 是否允许联网搜索由当前会话控制，而不是全局强制开启。
  - 实际搜索后端不写死某一家，走后端可配置搜索 API，避免前端承担密钥管理。

## Current State Analysis

### 模型与 Provider 现状

- `backend/src/agent_debug/provider/channels.py`
  - `deepseek` 当前被定义为 `openai` 协议，默认地址是 `https://api.deepseek.com/v1`。
- `backend/src/agent_debug/provider/channel_store.py`
  - DeepSeek 渠道最终会构造成 `OpenAICompatibleProvider`。
- `backend/src/agent_debug/provider/openai_compat_adapter.py`
  - 已支持 `messages`、`tools`、`tool_choice`、`thinking`、流式 delta、tool_calls 归一化。
  - 这里没有任何 DeepSeek “联网搜索”参数注入逻辑，且官方文档也没有公开对应请求字段。

### Agent 工具与运行时现状

- `backend/src/agent_debug/domain/tools/base.py`
  - 已有标准 `AgentTool`、`WorkspaceToolRegistry`、`ToolResult` 抽象，可直接扩展新工具。
- `backend/src/agent_debug/domain/tools/workspace_tools.py`
  - 当前内置工具只有 `read_file`、`list_dir`、`grep`。
  - 工具注册入口是 `build_default_workspace_tools()`。
- `backend/src/agent_debug/domain/runtime.py`
  - ReAct 循环会自动把注册表转成 `tools` schema 发给模型。
  - 模型返回 tool_calls 后，运行时会串行执行工具，并通过事件总线发布 `agent.tool.invoked/completed/failed`。
- `backend/src/agent_debug/domain/permission_service.py`
  - 名义上已经把 `web_search`、`web_fetch` 视作只读安全工具，但仓库里没有对应实现，说明这是“权限预留”而非真正功能。

### 会话与前端现状

- `backend/src/agent_debug/domain/models.py`
  - `DebugSession` 当前只有标题、状态、模式、选中模型、plan/run 等字段，没有“会话级联网搜索开关”。
- `backend/src/agent_debug/domain/session_service.py`
  - 会话会持久化到 `backend/agent_sessions.json`，说明新增会话字段具备真实落盘路径。
- `backend/src/agent_debug/api/rest_gateway.py`
  - `patch_session()` 已存在，适合承载会话级布尔开关更新。
  - `get_design_snapshot()` 会返回 `activeSession`，前端刷新后能自然拿到新增字段。
- `apps/agent-ide/public/api-client.jsx`
  - 已有 `patchSession()`、`askExecute()` 等前端 API 封装，适合扩展请求参数和会话更新调用。
- `apps/agent-ide/public/interactions.jsx`
  - 发送消息时统一走 `MoonlitAgentApi.askExecute(...)`。
- `apps/agent-ide/public/main.jsx`
  - 当前会话、聊天和状态栏都围绕 `backend.activeSession` 运转，适合在聊天列或状态栏附近补一个轻量会话开关。
- `apps/agent-ide/public/panels.jsx`
  - “Agent”设置页已经有本地 `联网搜索工具` / `网页抓取工具` 开关，但只是浏览器本地 `useStickyState` 状态，没有接后端，不应误认为真实能力。

### 依赖与实现约束

- `backend/requirements.txt` 与 `backend/pyproject.toml`
  - 已有 `httpx`，因此后端可以直接实现外部搜索 API 调用和网页抓取，无需新增依赖。
- 当前仓库没有通用外网搜索服务接入层，也没有浏览器端联网搜索 SDK；因此最稳妥路径是后端直连配置好的搜索 API。

## Assumptions & Decisions

- 决策 1：不尝试伪造或逆向 DeepSeek 网页端“联网搜索”私有协议。
  - 原因：目标是稳定维护的仓库内能力，而不是依赖官网私有实现。
- 决策 2：不等待 DeepSeek 官方公开“原生联网搜索 API”再做。
  - 当前目标已改为兼容方案，必须能在现有官方 API 上落地。
- 决策 3：搜索能力以 Agent 工具形式接入，而不是 Provider 私有参数。
  - 这样对 DeepSeek、OpenAI、Anthropic 兼容路径都一致，且复用现有 tool loop。
- 决策 4：搜索后端采用“可配置搜索 API”。
  - 不在代码中硬编码某家供应商。
  - 初版通过环境变量读取搜索配置，不新增前端密钥管理界面。
- 决策 5：联网搜索开关是会话级字段。
  - 开启后，该会话的工具注册表包含 `web_search` / `web_fetch`。
  - 关闭后，这两个工具不暴露给模型，从根源避免误调用。
- 决策 6：网页抓取能力与搜索能力分层。
  - `web_search` 只负责返回搜索结果摘要和候选 URL。
  - `web_fetch` 负责抓取某个 URL 的正文摘要，供模型二次阅读。
- 决策 7：初版只做只读能力，不做搜索引用富展示。
  - 聊天区先复用现有工具卡片；引用来源先以工具返回文本/结构化结果体现。

## Proposed Changes

### 1. 扩展会话模型，增加联网搜索开关

- 文件：`backend/src/agent_debug/domain/models.py`
  - 在 `DebugSession` 增加 `web_search_enabled: bool = False`。
  - 目的：把联网搜索能力从“全局设置幻觉”变成真实的会话状态。

- 文件：`backend/src/agent_debug/domain/session_service.py`
  - 新增或扩展会话更新方法，使其支持修改 `web_search_enabled` 并持久化。
  - 确保旧 `agent_sessions.json` 反序列化时对缺失字段兼容。

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 扩展 `patch_session()`，接受 `webSearchEnabled`。
  - `get_design_snapshot()` 继续透传 `activeSession`，无需新增快照接口。
  - 目的：前端可直接通过现有会话 patch 接口切换状态。

### 2. 新增通用联网搜索配置层

- 文件：`backend/src/agent_debug/domain/` 下新增一个轻量服务模块，例如 `web_search_service.py`
  - 职责：
    - 读取搜索配置。
    - 发起搜索 API 请求。
    - 统一解析搜索结果。
    - 为 `web_fetch` 提供网页正文抓取与裁剪。
  - 设计：
    - 使用现有 `httpx`。
    - 通过环境变量提供配置，例如：
      - `AGENT_DEBUG_WEB_SEARCH_BASE_URL`
      - `AGENT_DEBUG_WEB_SEARCH_API_KEY`
      - `AGENT_DEBUG_WEB_SEARCH_TIMEOUT_MS`
      - `AGENT_DEBUG_WEB_FETCH_TIMEOUT_MS`
    - 如果缺失必要配置，服务抛出稳定错误码，工具层再转成 `ToolExecutionError("TOOL_NOT_CONFIGURED", ...)`。

- 为什么单独抽服务层
  - 让工具只关注参数校验与结果格式。
  - 后续若切换 Brave / SerpAPI / SearXNG / 自建搜索网关，只需替换服务层解析逻辑。

### 3. 新增 `web_search` 与 `web_fetch` 工具

- 文件：`backend/src/agent_debug/domain/tools/` 下新增，例如 `web_tools.py`
  - 定义两个工具：
    - `web_search`
      - 输入：`query`, `limit`, 可选 `freshness`。
      - 输出：结构化结果列表，至少包含标题、URL、摘要。
      - `text` 输出保持简洁，利于模型继续决策下一步。
    - `web_fetch`
      - 输入：`url`。
      - 输出：抓取后的标题、最终 URL、正文摘要、裁剪文本。
  - 错误码约定：
    - `TOOL_INVALID_ARGS`
    - `TOOL_NOT_CONFIGURED`
    - `TOOL_FAILED`
    - `URL_NOT_ALLOWED` 或 `UNSUPPORTED_URL`

- 文件：`backend/src/agent_debug/domain/tools/workspace_tools.py`
  - 保持工作区工具实现不变，但把默认工具注册入口扩展为同时可接收联网工具。
  - 推荐方式：
    - 要么在本文件内注册 `web_tools`。
    - 要么保留工作区工具注册函数，再在 gateway 初始化阶段追加注册联网工具。

- 决策细节
  - `web_search` 返回搜索结果，不直接抓取全部正文，避免一次 tool call 带来巨大延迟和上下文膨胀。
  - `web_fetch` 对 HTML 做基础文本提取和长度裁剪，默认保留首屏高密度正文。

### 4. 按会话开关动态暴露联网工具

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 当前 `self.tool_registry = build_default_workspace_tools(self.workspace_tree)` 是全局固定注册。
  - 需要改成“基础工具注册表 + 按会话可见性过滤”方案。

- 文件：`backend/src/agent_debug/domain/runtime.py`
  - 在 `_run_react_loop()` 生成 `request["tools"]` 时，不再直接使用全量 `tool_registry.json_schemas()`。
  - 改为根据当前会话 `DebugSession.web_search_enabled` 过滤工具 schema。
  - `_dispatch_tool()` 也需要在运行前做同样的可见性校验，防止模型通过旧上下文残留调用被禁用工具。

- 推荐实现方式
  - 给 `WorkspaceToolRegistry` 增加按名称筛选 schema 的方法，或在 runtime 内部根据 allowlist 过滤。
  - allowlist 规则：
    - 默认总是允许：`read_file`、`list_dir`、`grep`、MCP demo 工具。
    - 仅当会话开启时允许：`web_search`、`web_fetch`。

- 为什么不直接全局注册再依赖 PermissionService 拒绝
  - 因为用户要求的是“会话开关”，最合理的行为应是关闭时模型根本看不到这两个工具。

### 5. 将会话开关注入发送链路与前端 UI

- 文件：`apps/agent-ide/public/api-client.jsx`
  - 保持 `patchSession()` 不变。
  - `askExecute()` 可保持现状，不强制新增参数；会话状态由后端根据 `sessionId` 自己读取。
  - 如需增强调试可选地透传 `webSearchEnabled`，但不是必需。

- 文件：`apps/agent-ide/public/interactions.jsx`
  - 发送逻辑无需知道搜索实现细节。
  - 只要当前 session 已更新，后端就会决定工具可见性。

- 文件：`apps/agent-ide/public/main.jsx`
  - 在与当前会话强绑定的位置增加“联网搜索”开关，推荐放在聊天列头部或状态栏附近，而不是设置页。
  - 行为：
    - 读取 `backend.activeSession.webSearchEnabled`。
    - 点击后调用 `MoonlitAgentApi.patchSession(sessionId, { webSearchEnabled: next })`。
    - 成功后 `refreshBackend(sessionId, { preserveChat: true, preserveEvents: true })`。
  - UI 文案要明确：
    - 这是“当前会话允许 Agent 使用联网搜索工具”。
    - 不是 DeepSeek 官方 API 原生搜索。

- 文件：`apps/agent-ide/public/panels.jsx`
  - 现有“联网搜索工具 / 网页抓取工具”是本地 sticky state，与后端脱节。
  - 初版建议只补一段说明或暂不接线，避免与新的会话级真实开关混淆。
  - 若后续需要，可把这里转成“默认偏好”，但本次不作为主实现路径。

### 6. 提示词与子代理工具描述同步

- 文件：`backend/src/agent_debug/prompts/builtin_subagents.py`
  - 当前 `researcher` 已声明 `web_search`、`web_fetch`。
  - 需要确认文字描述与真实能力一致，例如明确其基于外部搜索 API 和网页抓取，而非 Provider 原生搜索。

- 文件：如有工具说明聚合逻辑的 prompt 模块，也需同步检查
  - 目标是避免模型把该能力误解为 DeepSeek 内建联网模式。

### 7. 测试补齐

- 文件：`backend/tests/agent_debug/` 下新增或扩展测试
  - `test_session_web_search_toggle`
    - 验证 `patch_session` 能持久化 `web_search_enabled`。
  - `test_runtime_hides_web_tools_when_disabled`
    - 会话关闭时，请求发给 provider 的 `tools` 列表中不包含 `web_search` / `web_fetch`。
  - `test_runtime_exposes_web_tools_when_enabled`
    - 会话开启时，这两个工具出现在 schema 中。
  - `test_web_search_tool_config_error`
    - 未配置搜索 API 时返回稳定错误，而不是崩溃。
  - `test_web_fetch_tool_success`
    - 用伪造 HTTP 响应验证正文提取与裁剪。

- 文件：`backend/tests/agent_debug/test_provider_execution.py`
  - 可补一个最小链路测试，证明在 DeepSeek/OpenAI-compatible provider 下，tool_calls + 新工具仍能走通。

## Implementation Notes

- 搜索 API 适配接口建议统一为内部返回结构：
  - `{"items": [{"title": str, "url": str, "snippet": str}], "query": str, "provider": str}`
- `web_search` 工具对模型暴露的 JSON Schema：
  - `query: string` 必填
  - `limit: integer` 可选，默认 5，硬上限 10
- `web_fetch` 工具对模型暴露的 JSON Schema：
  - `url: string` 必填
- 正文提取策略：
  - 初版使用标准库 `html.parser` 或正则/最小 HTML 清洗实现，避免新依赖。
  - 移除 `script/style/noscript` 内容。
  - 保留标题与正文前若干千字符，防止上下文爆炸。
- 安全边界：
  - 仅允许 `http/https` URL。
  - 默认跟随跳转，但限制最大跳转次数和超时。
  - 默认 user-agent 固定，避免被部分服务直接拒绝空白客户端。

## Verification Steps

1. 后端单测
   - 运行新增的 session/tool/runtime 测试，确认开关、工具可见性、错误处理都稳定。

2. 手工后端验证
   - 配置搜索 API 环境变量并启动后端。
   - 创建新会话，默认确认 `webSearchEnabled=false`。
   - 打开会话级开关后，再次查看 `list_tools` 或 provider 请求体，确认工具 schema 已出现。
   - 关闭后再次确认 schema 消失。

3. 前端联调
   - 刷新 IDE，确认聊天区域能看到会话级“联网搜索”开关。
   - 切换开关后刷新当前会话，不丢失聊天记录。
   - 触发一个需要最新信息的问题，确认工具卡片能展示 `web_search` / `web_fetch`。

4. 回归验证
   - 会话关闭联网搜索时，本地 `read_file/list_dir/grep` 与 MCP demo 不受影响。
   - DeepSeek 渠道的普通对话、tool call、streaming 不回退。
   - 未配置搜索 API 时，系统给出清晰错误提示，不影响普通会话。

## Out Of Scope

- 不接入 DeepSeek 官网私有网页端协议。
- 不实现搜索结果引用富卡片、网页快照预览、来源高亮。
- 不在本次规划中增加搜索 API 的前端密钥管理页面。
- 不把设置页里现有本地 sticky-state “联网搜索工具”改造成完整配置中心，只避免其与真实会话开关混淆。
