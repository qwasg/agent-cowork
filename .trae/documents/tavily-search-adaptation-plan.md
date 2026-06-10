# Tavily 搜索适配计划

## Summary

- 目标：把当前项目的联网搜索能力从“通用外部搜索 API 适配层”收敛为“面向 Tavily 的专用实现”，并保持现有会话级 `联网搜索` 开关、工具名 `web_search` / `web_fetch`、前后端主交互路径不变。
- 范围已确认：
  - 只替换现有搜索链路，不引入 `crawl` / `map` / `research` 新工具。
  - 配置界面改成 Tavily 专用，而不是继续暴露 `Base URL + Path` 这类通用网关配置。
- 实现策略：
  - 后端用现有 `httpx` 直连 Tavily REST API，不新增 Python 依赖。
  - `web_search` 走 Tavily `/search`。
  - `web_fetch` 优先走 Tavily `/extract`，保留当前本地 HTML 提取逻辑作为失败回退，降低回归风险。
  - `/api/agent-debug/search-config` 接口保留，但其返回/保存字段调整为 Tavily 语义。

## Current State Analysis

### 后端现状

- `backend/src/agent_debug/domain/web_search_service.py`
  - 当前实现是“通用搜索 API 适配层”。
  - 通过 `SearchApiConfig` 或环境变量读取 `base_url`、`path`、`api_key`，发起 `GET` 请求，再从 `items/results/data/webPages.value` 中做归一化。
  - `fetch()` 是本地 `httpx + HTMLParser` 文本抽取，不依赖外部内容提取 API。

- `backend/src/agent_debug/domain/tools/web_tools.py`
  - 已注册 `web_search` 和 `web_fetch` 两个工具。
  - `web_search` 目前仅接受 `query`、`limit`、`freshness`。
  - `web_fetch` 仅接受 `url`。
  - 运行时和工具名已经稳定，适合“底层替换而非接口重做”。

- `backend/src/agent_debug/domain/search_config.py`
  - 当前配置模型只有：
    - `enabled`
    - `base_url`
    - `path`
    - `api_key`
    - `created_at`
    - `updated_at`
  - 这与 Tavily 的固定基地址和专用参数模型不匹配。

- `backend/src/agent_debug/infra/search_config_store.py`
  - 配置持久化到 `agent_search_config.json`，支持加密存储。
  - 现有读写逻辑支持 snake_case / camelCase 兼容，适合做一次平滑字段迁移。

- `backend/src/agent_debug/api/rest_gateway.py`
  - `get_search_config()` / `set_search_config()` 已存在，当前仅转发 `enabled/baseUrl/path/apiKey`。
  - `allowed_tool_names_for_session()` 已根据会话 `webSearchEnabled` 决定是否向模型暴露 `web_search` / `web_fetch`。
  - 说明会话级能力控制已就绪，本次不需要改会话模型或 runtime 工具可见性逻辑。

### 前端现状

- `apps/agent-ide/public/panels.jsx`
  - `SearchApiSection` 已经是独立设置区块，当前 UI 字段为：
    - 启用搜索 API
    - Base URL
    - Path
    - API Key
  - 保存调用 `window.MoonlitAgentApi.setSearchConfig(...)`，不需要改 API 调用入口，只需改展示字段和校验逻辑。

- `apps/agent-ide/public/api-client.jsx`
  - 已有 `getSearchConfig()` / `setSearchConfig()` 封装。
  - 可以保持接口路径不变，仅调整请求/响应体字段。

- `apps/agent-ide/public/main.jsx`
  - 状态栏里的 `联网搜索 开/关` 直接切换当前会话 `webSearchEnabled`。
  - 这部分与具体搜索供应商解耦，本次不需要改交互。

- `apps/agent-ide/package.json` 与 `scripts/check-static.mjs`
  - 源码入口明确指向 `public/*.jsx`，不是 `dist/*.jsx`。
  - 计划中以前端源码文件为准，不把 `dist` 当作手工维护目标；若实现阶段需要产物同步，交给构建流程处理。

### 测试现状

- `backend/tests/agent_debug/test_web_search_tools.py`
  - 已覆盖：
    - 会话级工具暴露开关
    - 搜索配置保存/读取
    - `WebSearchService.search()`
    - `WebSearchService.fetch()`
  - 但断言仍围绕通用 `baseUrl/path` 配置与通用 GET 搜索接口。

- `backend/tests/agent_debug/test_server.py`
  - 已覆盖 `/api/agent-debug/search-config` 的 HTTP 层读写。
  - 也需要跟随配置字段模型同步。

## Assumptions & Decisions

- 决策 1：保留现有工具名与工具暴露策略。
  - `web_search` / `web_fetch` 不改名。
  - 当前会话 `webSearchEnabled` 的行为不变。
  - 原因：最大限度复用现有 prompt、runtime、UI 和测试结构。

- 决策 2：后端直接调用 Tavily REST API，不接 Tavily Python SDK。
  - 继续使用仓库里已有的 `httpx`。
  - 原因：减少依赖变更，贴合现有服务层风格，也更容易做单测替身。

- 决策 3：配置接口路径不改，但配置语义切换为 Tavily 专用。
  - 保留 `/api/agent-debug/search-config`。
  - 字段改为 Tavily 相关配置，而不是 `baseUrl/path`。

- 决策 4：`web_search` 的工具入参尽量保持兼容。
  - 保留 `query`、`limit`、`freshness`。
  - 内部映射关系：
    - `limit` -> `max_results`
    - `freshness` -> `time_range`
  - 这样运行时与模型侧无需额外迁移。

- 决策 5：`web_fetch` 对外接口不变，但实现优先切到 Tavily `/extract`。
  - 输入仍是单个 `url`。
  - 服务层内部把单 URL 包装为 `urls=[url]` 请求 Tavily。
  - 若 Tavily extract 失败或返回空内容，则回退到当前本地 HTML 文本提取逻辑。

- 决策 6：本次不新增 Tavily 的 `crawl` / `map` / `research` 工具。
  - 这些能力在教程中存在，但不属于本次确认范围。

- 决策 7：Tavily 配置以“必要最小集”为主。
  - 保存以下字段：
    - `enabled`
    - `provider`（固定为 `tavily`，主要用于前端展示和后续扩展兼容）
    - `apiKey`
    - `topic`
    - `searchDepth`
    - `timeRange`
    - `extractDepth`
    - `createdAt`
    - `updatedAt`
  - 不在本次暴露 `include_domains`、`exclude_domains`、`auto_parameters`、`project_id` 等高级项，避免 UI 过重。

## Proposed Changes

### 1. 重定义搜索配置模型为 Tavily 专用

- 文件：`backend/src/agent_debug/domain/search_config.py`
  - 将 `SearchApiConfig` 从通用网关字段改为 Tavily 专用字段。
  - 新模型建议包含：
    - `enabled: bool = False`
    - `provider: str = "tavily"`
    - `api_key: str = ""`
    - `topic: str = "general"`
    - `search_depth: str = "basic"`
    - `time_range: str = ""`
    - `extract_depth: str = "basic"`
    - `created_at: str = ""`
    - `updated_at: str = ""`
  - 目的：让后端服务和前端配置都围绕 Tavily 的真实参数建模。

- 文件：`backend/src/agent_debug/infra/search_config_store.py`
  - 更新 `_config_from_dict()`，支持从旧字段平滑迁移：
    - 旧 `apiKey` / `api_key` 继续兼容读取。
    - 旧 `baseUrl` / `path` 读取后忽略，不再写回。
    - 缺失新字段时填默认值。
  - 写回结构统一为新模型，完成一次静默升级。

### 2. 把 `WebSearchService` 改造成 Tavily 服务层

- 文件：`backend/src/agent_debug/domain/web_search_service.py`
  - 保留 `WebSearchService` 类名，减少调用方变更。
  - 调整职责：
    - `search()`：从“通用 GET 搜索 API”改为调用 Tavily `POST https://api.tavily.com/search`
    - `fetch()`：优先调用 Tavily `POST https://api.tavily.com/extract`
    - 本地 HTML 解析逻辑下沉为 extract 失败时的 fallback
  - 请求构造细节：
    - 通用请求头：
      - `Authorization: Bearer <api_key>`
      - `Content-Type: application/json`
      - `User-Agent: MoonlitAgentDebug/1.0 ...`
    - `search()` 请求体：
      - `query`
      - `max_results`
      - `search_depth`
      - `topic`
      - `time_range`（优先使用工具传入的 `freshness`，否则使用配置默认值）
      - `include_raw_content: false`
      - `include_images: false`
    - `fetch()` 请求体：
      - `urls: [url]`
      - `extract_depth`
      - `format: "text"`
  - 响应归一化：
    - `search()` 统一输出：
      - `query`
      - `provider: "tavily"`
      - `items: [{ title, url, snippet, score? }]`
      - `rawCount`
    - `fetch()` 统一输出：
      - `url`
      - `title`
      - `contentType`
      - `text`
      - 可选 `source: "tavily" | "http-fallback"`
  - 错误语义：
    - 没有 API Key 或配置不可用时抛 `WebSearchConfigError`
    - Tavily 非 2xx 或结构不合法时抛 `WebSearchRequestError`
    - fallback 仅用于 `fetch()`，不用于 `search()`

### 3. 保持工具层接口稳定，只替换内部行为

- 文件：`backend/src/agent_debug/domain/tools/web_tools.py`
  - `WebSearchTool.parameters` 继续保留当前 schema：
    - `query`
    - `limit`
    - `freshness`
  - `WebSearchTool.run()` 改为消费 Tavily 结果格式，并在文本输出中优先展示：
    - 标题
    - URL
    - 摘要
    - 如果有 `score`，可以选择性附带，便于模型排序

- 文件：`backend/src/agent_debug/domain/tools/web_tools.py`
  - `WebFetchTool.parameters` 保持不变。
  - `WebFetchTool.run()` 对外返回格式保持兼容，不让运行时或前端工具卡片感知供应商切换。

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 保持 `register_web_tools(self.tool_registry, service=self.web_search_service)` 现有接线方式不变。
  - 说明：本次不动工具注册和会话 allowlist，只替换服务层与配置输出。

### 4. 把搜索配置 API 改成 Tavily 专用语义

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 调整 `_search_config_to_dict()` 输出字段：
    - `enabled`
    - `provider`
    - `apiKeySet`
    - `topic`
    - `searchDepth`
    - `timeRange`
    - `extractDepth`
    - `updatedAt`
    - `createdAt`
  - 删除对 `baseUrl` / `path` 的对外暴露。

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 调整 `set_search_config()` 入参解析：
    - 接收 Tavily 专用字段
    - 对枚举值做轻量归一化：
      - `topic` 允许 `general` / `news`
      - `searchDepth` 允许 `basic` / `advanced`
      - `timeRange` 允许 `"" | day | week | month | year`
      - `extractDepth` 允许 `basic` / `advanced`
    - `apiKey` 为空字符串时沿用现有值，不强制覆盖为空
  - 目的：保证 UI 可以直接展示 Tavily 选项，避免提交非法配置。

### 5. 前端搜索配置面板改成 Tavily 专用

- 文件：`apps/agent-ide/public/panels.jsx`
  - 把 `SearchApiSection` 从“搜索 API 配置”改成“Tavily 搜索配置”。
  - 初始草稿字段改为：
    - `enabled`
    - `provider`
    - `apiKey`
    - `apiKeySet`
    - `topic`
    - `searchDepth`
    - `timeRange`
    - `extractDepth`
  - 界面调整为：
    - 启用 Tavily
    - Provider 只读文案或固定说明 `Tavily`
    - API Key
    - 默认搜索主题 `general/news`
    - 默认搜索深度 `basic/advanced`
    - 默认时间范围 `不限/day/week/month/year`
    - 默认抽取深度 `basic/advanced`
  - 删除：
    - Base URL 输入框
    - Path 输入框
    - 其对应的 URL 格式校验
  - 保存时继续调用现有 `setSearchConfig()`，但提交 নতুন字段。

- 文件：`apps/agent-ide/public/api-client.jsx`
  - 方法名与接口路径保持不变。
  - 如果当前代码对返回结构做了字段假设，则同步改成 Tavily 字段名；否则无需结构性改动。

### 6. 测试同步到 Tavily 语义

- 文件：`backend/tests/agent_debug/test_web_search_tools.py`
  - 更新 `test_rest_gateway_sets_and_gets_search_config()`：
    - 改为断言 `provider/topic/searchDepth/timeRange/extractDepth/apiKeySet`
  - 重写 `test_web_search_service_prefers_saved_config()`：
    - 断言请求 URL 为 `https://api.tavily.com/search`
    - 断言请求方法语义对应 POST JSON
    - 断言配置字段被映射到 Tavily 请求体
  - 新增或调整 `test_web_search_service_uses_tool_freshness_over_default_time_range()`：
    - 验证工具传入 `freshness` 时优先覆盖配置默认 `timeRange`
  - 新增或调整 `test_web_fetch_prefers_tavily_extract_and_falls_back_to_http()`：
    - Tavily extract 成功时，使用其 `raw_content`
    - Tavily extract 失败时，回退到现有 HTML 抽取逻辑

- 文件：`backend/tests/agent_debug/test_server.py`
  - 更新 `/api/agent-debug/search-config` 的 HTTP 断言，覆盖新的字段结构与兼容保存逻辑。

### 7. 验证现有会话开关链路无需回归修改

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - `allowed_tool_names_for_session()` 逻辑保持不变，但实现后需通过现有测试确认：
    - 会话关闭时不暴露 `web_search` / `web_fetch`
    - 会话开启时照常暴露

- 文件：`apps/agent-ide/public/main.jsx`
  - 不改会话开关交互，但在联调时要确认：
    - 开启搜索后，工具实际走 Tavily
    - 关闭搜索后，模型不可见这两个工具

## Verification Steps

1. 后端单测
   - 运行 `backend/tests/agent_debug/test_web_search_tools.py`
   - 运行 `backend/tests/agent_debug/test_server.py`
   - 重点确认配置迁移、Tavily 请求映射、fallback 行为、会话工具可见性。

2. 前端静态检查
   - 运行 `apps/agent-ide` 下的 `npm run lint`
   - 运行 `npm run typecheck`
   - 确认 `public/panels.jsx` 的字段调整未破坏静态入口与 smoke test。

3. 手工后端验证
   - 保存一份 Tavily 配置后，确认 `agent_search_config.json` 已按新字段写入。
   - 创建或切换到开启 `联网搜索` 的会话。
   - 发送需要最新信息的问题，确认模型可调用 `web_search`。
   - 在需要阅读具体网页时，确认 `web_fetch` 优先返回 Tavily extract 内容。

4. 手工前端验证
   - “模型”页中的搜索配置区已变为 Tavily 专用文案。
   - 不再出现 `Base URL` 和 `Path` 输入框。
   - API Key 覆盖、保留旧 Key、枚举项切换都能正确保存与回显。

5. 回归验证
   - 未开启当前会话的 `联网搜索` 时，普通聊天与本地工具不受影响。
   - 未配置 Tavily API Key 时，普通会话仍可工作；只有联网搜索工具在被调用时返回明确配置错误。
   - 已有旧配置文件不会导致后端启动或读取失败。

## Out Of Scope

- 不新增 `crawl`、`map`、`research` 等 Tavily 工具。
- 不改状态栏 `联网搜索` 会话开关的交互位置与行为。
- 不把搜索结果做成富引用卡片、来源分组、图片展示等 UI 增强。
- 不引入 Tavily SDK、LangChain、LangGraph 等新依赖。
