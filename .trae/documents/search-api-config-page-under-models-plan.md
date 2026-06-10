# 在模型页下新增搜索 API 配置页面计划

## Summary

- 目标是在现有“模型”页面中、`模型配置（渠道）` 区块下方，新增一个独立的“搜索 API 配置”页面/section。
- 该页面采用“简版配置”：
  - `启用开关`
  - `Base URL`
  - `API Key`
  - `Path`
- 配置需要由后端加密持久化，而不是仅保存在浏览器本地。
- 配置生效后，现有 `backend/src/agent_debug/domain/web_search_service.py` 不再只依赖环境变量，而是优先读取后端持久化配置；若 UI 未配置，则继续回退到环境变量，保持兼容。

## Current State Analysis

### 前端现状

- `apps/agent-ide/public/panels.jsx`
  - `ModelsPage` 当前只有一个 `ChannelsSection`，见 [panels.jsx](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/panels.jsx#L871-L875)。
  - `ChannelsSection` 已负责“模型配置（渠道）”的列表、表单、保存和刷新，见 [panels.jsx](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/panels.jsx#L707-L869)。
  - Agent 页面里虽然有“联网搜索工具 / 网页抓取工具”的本地 sticky state，但那只是浏览器偏好，不是后端真实配置，见 [panels.jsx](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/panels.jsx#L457-L477)。

- `apps/agent-ide/public/api-client.jsx`
  - 当前已有渠道相关接口：`listChannels`、`createChannel`、`updateChannel`、`deleteChannel`、`fetchChannelModels`，见 [api-client.jsx](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/api-client.jsx#L283-L300)。
  - 当前没有任何“搜索 API 配置”的前端 API 封装。

### 后端现状

- `backend/src/agent_debug/domain/web_search_service.py`
  - 当前搜索配置只从环境变量读取：
    - `AGENT_DEBUG_WEB_SEARCH_BASE_URL`
    - `AGENT_DEBUG_WEB_SEARCH_PATH`
    - `AGENT_DEBUG_WEB_SEARCH_API_KEY`
    - 以及其它高级参数
  - 见 [web_search_service.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/web_search_service.py#L100-L117)。

- `backend/src/agent_debug/api/rest_gateway.py`
  - 当前已包含会话、模型、渠道等配置入口，但没有“搜索 API 配置”的读取/保存方法。
  - 现有渠道配置保存后会触发 provider registry 重建，见 [rest_gateway.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L523-L538)。

- `backend/src/agent_debug/server.py`
  - 当前仅暴露渠道类 REST 路由，没有搜索配置路由，见 [server.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/server.py#L352-L378)。

### 持久化现状

- `backend/src/agent_debug/provider/channel_store.py`
  - 渠道配置通过独立文件 `agent_channels.json` 加密落盘，使用 `CryptoStore` Fernet 加密，见 [channel_store.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/provider/channel_store.py#L64-L173)。

- `backend/src/agent_debug/infra/crypto_store.py`
  - 已有通用加密能力，可用于新的搜索配置存储，见 [crypto_store.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/infra/crypto_store.py#L13-L58)。

## Assumptions & Decisions

- 决策 1：页面位置按用户要求，直接加在 `ModelsPage` 下面，不新增新的左侧一级导航。
- 决策 2：UI 采用“简版配置”，只暴露 `enabled / baseUrl / apiKey / path` 四类输入。
- 决策 3：配置保存到后端，并加密持久化；不使用 `localStorage` 作为真实来源。
- 决策 4：后端为搜索配置新增独立存储文件和接口，不把它硬塞进现有渠道配置数据结构。
- 决策 5：`WebSearchService` 保持向后兼容：
  - 优先读取持久化配置。
  - 未配置时回退环境变量。
- 决策 6：初版不增加高级字段编辑能力，如 `authHeader`、`queryParam`、`timeout`、`fetchTimeout`、`maxFetchChars`、`userAgent`。
  - 这些仍用现有默认值或环境变量。
- 决策 7：`API Key` 的前端表现沿用渠道配置风格：
  - 支持“已配置”状态展示。
  - 编辑时留空表示不修改已有 Key。

## Proposed Changes

### 1. 新增搜索配置数据模型与加密存储

- 文件：`backend/src/agent_debug/domain/` 下新增一个轻量模型文件，例如 `search_config.py`
  - 定义搜索配置结构，建议字段：
    - `enabled: bool`
    - `base_url: str`
    - `path: str`
    - `api_key: str`
    - `created_at: str`
    - `updated_at: str`
  - 目的：让后端配置有明确 schema，而不是散落在 dict 中。

- 文件：`backend/src/agent_debug/infra/` 下新增存储模块，例如 `search_config_store.py`
  - 持久化文件建议独立为 `agent_search_config.json`。
  - 加密方式复用 `CryptoStore`，实现风格参考 [channel_store.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/provider/channel_store.py)。
  - 读写行为：
    - 读取失败时返回默认空配置。
    - 写入优先加密，失败时按当前仓库已有策略降级明文并告警。
  - 原因：搜索配置与模型渠道配置职责不同，分文件更清晰，也避免后续扩展互相耦合。

### 2. 在 Rest Gateway 中新增搜索配置读写能力

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 在 gateway 初始化时创建 `SearchConfigStore` 实例。
  - 新增方法：
    - `get_search_config()`
    - `set_search_config(payload)`
  - 返回结构建议：
    - `config.enabled`
    - `config.baseUrl`
    - `config.path`
    - `config.apiKeySet`
    - `config.updatedAt`
  - 写入规则：
    - `apiKey` 为空字符串时保留旧值。
    - `baseUrl`、`path` 支持显式保存。
    - `enabled` 默认 `false`。
  - 目的：前端通过统一 gateway 读写，不直接碰存储实现。

### 3. 在 FastAPI 层新增搜索配置路由

- 文件：`backend/src/agent_debug/server.py`
  - 新增 REST 路由：
    - `GET /api/agent-debug/search-config`
    - `PUT /api/agent-debug/search-config`
  - 路由风格对齐现有渠道接口和会话接口。

- 可选同步文件：若仓库内 bridge handler 需要覆盖到同类能力，则同步补充
  - `backend/src/agent_debug/api/bridge_handlers.py`
  - 只有在当前 IDE 其他入口确实依赖 bridge handler 访问该接口时才需要加；执行时再根据现有模式确认。

### 4. 让 WebSearchService 优先读取持久化配置

- 文件：`backend/src/agent_debug/domain/web_search_service.py`
  - 当前 `WebSearchService` 在 `__init__()` 中直接读环境变量，见 [web_search_service.py](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/web_search_service.py#L100-L117)。
  - 需要改为支持注入配置来源，推荐方案：
    - 给 `WebSearchService` 增加可选配置对象/配置解析器参数。
    - 若注入了后端搜索配置，则优先使用该配置。
    - 未注入或字段为空时，再回退到环境变量。
  - 目的：避免 UI 保存后还必须重启或改环境变量。

- 文件：`backend/src/agent_debug/domain/tools/web_tools.py`
  - 注册 `WebSearchTool` / `WebFetchTool` 时，给它们注入共享的 `WebSearchService` 实例。
  - 该实例需要绑定新的搜索配置来源，确保工具调用能拿到页面里保存的真实配置。

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
  - 由于当前 registry 初始化时会调用 `register_web_tools(self.tool_registry)`，这里要改成把带配置来源的 service 传进去。

### 5. 在前端 ModelsPage 下新增 Search API 配置 Section

- 文件：`apps/agent-ide/public/panels.jsx`
  - 在 `ChannelsSection` 后新增一个独立 section，例如 `SearchApiSection`。
  - `ModelsPage` 结构改为：
    - 标题 `模型`
    - `ChannelsSection`
    - `SearchApiSection`
  - 新 section UI 采用当前设置页现成风格：
    - 外层 `Card`
    - 每项使用 `Row`
    - 顶部用和 `ChannelsSection` 一致的标题行样式

- `SearchApiSection` 建议字段
  - `启用搜索 API`
  - `Base URL`
  - `Path`
  - `API Key`
  - 保存按钮

- 行为细节
  - 首次加载时调用后端获取配置。
  - `API Key` 文案参考 `ChannelForm`：
    - 已配置时显示“已配置（输入新 Key 以覆盖）”
    - 留空表示不修改已保存的 Key
  - 保存成功后显示 toast。
  - 如有需要，保存后调用 `refreshBackend(..., { preserveChat: true })`，保证页面状态及时更新。

### 6. 扩展前端 API Client

- 文件：`apps/agent-ide/public/api-client.jsx`
  - 新增：
    - `getSearchConfig()`
    - `setSearchConfig(config)`
  - 路由分别对应：
    - `GET /api/agent-debug/search-config`
    - `PUT /api/agent-debug/search-config`

### 7. 校验与默认值策略

- 后端校验
  - `baseUrl` 允许为空，但当 `enabled=true` 且实际要执行 `web_search` 时，如仍为空则延续当前 `TOOL_NOT_CONFIGURED` 逻辑。
  - `path` 默认为 `/search`。
  - `apiKey` 可以为空，适配无需密钥的网关。

- 前端校验
  - 简版仅做轻量校验：
    - `Base URL` 输入为非空时可做基本 URL 格式校验。
    - `Path` 若为空则自动回写 `/search`。
  - 不在页面里做复杂供应商适配逻辑。

## Verification Steps

1. 后端接口验证
   - 调用 `GET /api/agent-debug/search-config`，确认能返回默认配置。
   - 调用 `PUT /api/agent-debug/search-config` 保存后，再次读取确认字段落盘。
   - 重启后端后再次读取，确认加密持久化生效。

2. 前端页面验证
   - 打开“模型”页，确认在 `模型配置（渠道）` 下方出现新的“搜索 API 配置”区块。
   - 首次加载能正确显示当前配置状态。
   - 保存后出现成功提示，刷新页面后仍保留。

3. 功能联调
   - 打开会话级“联网搜索”开关。
   - 在搜索 API 配置页填写可用的 `Base URL / API Key / Path` 并保存。
   - 让 Agent 触发 `web_search`，确认不再报“missing AGENT_DEBUG_WEB_SEARCH_BASE_URL”。

4. 回归验证
   - 现有渠道配置的增删改查不受影响。
   - 未配置搜索 API 页面时，环境变量配置仍可继续工作。
   - 仅 `web_fetch` 使用时不应依赖搜索 API 配置。

## Out Of Scope

- 不新增左侧导航页签。
- 不实现高级搜索配置项编辑器。
- 不实现“测试连接”按钮。
- 不改造 Agent 页面里的本地 sticky-state 偏好为真实配置源。
- 不在本次计划里增加搜索供应商模板或预设列表。
