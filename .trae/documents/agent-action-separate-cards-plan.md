# 规划：将当前 Agent 每次行动拆成独立呈现框

## Summary

- 目标：修正当前前端把同一轮 Agent 的思考、工具调用、Todo/Plan、最终输出都塞进同一个消息框的问题，改为“同一轮回复内按行动顺序拆成多个独立卡片”。
- 成功标准：
  - 实时执行时，思考、每次工具调用、Todo/Plan、最终输出各自拥有独立呈现框。
  - 历史回放/刷新后，消息结构与实时阶段一致，不再退化回单一大框。
  - 同一轮回复仍属于一个 agent turn，不拆成多条聊天消息；头像、头部信息只显示一次。
  - 不改后端事件协议，优先通过前端事件聚合与渲染层完成。

## Current State Analysis

### 现有数据聚合

- `apps/agent-ide/public/interactions.jsx`
  - `buildAgentBlocksFromEvents()` 当前会把整轮 `run` 的事件压成一个 `blocks` 数组：
    - 所有 `agent.reasoning.delta` 合并成一个 `reasoning` block。
    - 所有 `agent.tool.*` 合并成一个 `process` block，内部再挂多个 `tool`。
    - 所有文本类输出合并成一个 `text` block。
    - `todo`、`plan`、`code_edit` 作为附加 block 插入。
  - `messagesFromEvents()` 继续按 `correlationId` 把整轮事件折叠为一条 `agent` 消息，因此“一个 run = 一个消息框”。
  - `useChatStore.send()` 在请求完成后再次调用同一聚合器固化消息，因此实时态和历史态共用同一问题。

### 现有渲染

- `apps/agent-ide/public/components.jsx`
  - `ChatMessage` 和 `LiveAssistantBubble` 都把同一轮的 `blocks` 直接放进一个 `.msg-agent` 容器。
  - `AgentBlocks` 只是顺序渲染 block，不负责给每个行动提供统一的外层卡片语义。
  - `ReasoningBlock`、`ProcessGroup`、`TaskProgressCard`、`ToolUseCard` 已经各自有内部卡片，但它们仍属于同一个消息框，不满足“每次行动独立呈现框”的要求。

### 影响范围结论

- 根因不是单纯 CSS，而是“前端事件聚合模型 + 消息容器模型”共同决定的。
- 仅做视觉分隔无法满足需求；必须把 `blocks` 从“按类型合并”改成“按行动顺序生成 timeline items”。
- 需要同时覆盖：
  - 快照恢复：`messagesFromEvents()`
  - 实时展示：`LiveAssistantBubble`
  - 请求完成后的最终固化：`useChatStore.send()`

## Proposed Changes

### 1. 重构事件聚合器为“行动时间线”

- 文件：`apps/agent-ide/public/interactions.jsx`
- 变更内容：
  - 新增一个基于事件顺序的聚合函数，建议命名为 `buildAgentTimelineFromEvents()` 或等价名称。
  - 输出结构从当前“按类型归并的 blocks”改为“同一轮内多个 action cards”，每个 item 至少包含：
    - `id`
    - `kind` 或 `cardType`
    - `status`
    - `payload`
    - 可选的 `ts/seq/sourceEventIds`
  - 拆分规则按事件顺序执行，不再把整轮文本/整轮工具过程压成单一块。

- 具体拆分策略：
  - `agent.reasoning.delta`
    - 聚合为当前正在进行的 reasoning 卡片。
    - 一旦后续出现工具调用、文本输出、plan/todo 等不同类别事件，结束当前 reasoning 卡片并开启下一个行动卡片。
  - `agent.tool.invoked / completed / failed / denied`
    - 每个 `toolCallId` 生成独立工具卡片，而不是并入同一个 `process` group。
    - `invoked` 创建卡片，`completed/failed/denied` 更新同一张卡片状态与结果。
  - `todo.*`
    - 合并成独立 Todo 进度卡，但只在相邻 todo 事件之间合并，避免跨行动吞并其他内容。
  - `plan.created`
    - 生成独立 Plan 卡片，不再只向文本区写一条“已生成计划”。
  - `agent.token.stream.delta`
    - 聚合为输出卡片；若输出发生在工具后，则开启新的输出卡片，而不是与前序文本拼接。
  - `subagent.summary.generated`
    - 作为独立结果卡片或并入紧随其后的输出卡片，执行时固定一种规则，避免刷新后形态变化。
  - `agent.code_edit.proposed`
    - 保持独立卡片。

- 保留兼容性：
  - 可以保留 `buildAgentBlocksFromEvents()` 作为兼容包装层，但内部改为基于新的 timeline 结构派生。
  - 更直接的方案是把消息对象上的 `blocks` 升级为 `items/cards`，并同步更新消费方。

### 2. 调整消息模型，保留“同一轮单消息、内部多卡片”

- 文件：`apps/agent-ide/public/interactions.jsx`
- 变更内容：
  - `messagesFromEvents()` 仍按 `correlationId` 生成一条 agent 消息，但消息内容字段改为 timeline cards。
  - `useChatStore.send()` 在请求完成后使用同一套 timeline 聚合逻辑生成最终消息，确保实时态与固化态一致。
  - `hasRenderableBlocks()` 需要升级为时间线可渲染判断，例如 `hasRenderableCards()`。

- 设计决策：
  - 不拆成多条聊天消息，避免复制头像、时间、模型标签，且不破坏现有回放/Fork/Copy Messages 逻辑。
  - 仍保留 `runId` 作为这一轮消息的主键归属。

### 3. 新增“行动时间线卡片”渲染层

- 文件：`apps/agent-ide/public/components.jsx`
- 变更内容：
  - 新增统一容器组件，例如：
    - `AgentActionTimeline`
    - `AgentActionCard`
  - `ChatMessage` 和 `LiveAssistantBubble` 不再直接把原始 `blocks` 平铺到 `.msg-agent` 内，而是在头部下面渲染一个时间线容器。
  - 每个行动卡片共享统一外框、间距、状态样式，再在卡内挂载已有内容组件。

- 映射方式：
  - reasoning card -> 使用现有 `ReasoningBlock`
  - tool card -> 优先直接使用 `ToolUseCard`
  - todo card -> 使用 `TaskProgressCard`
  - plan card -> 使用 `InlinePlan`
  - text/output card -> 使用 `MarkdownText`
  - code edit card -> 使用现有工具卡或单独卡片包装

- 关键调整：
  - 弱化或移除 `ProcessGroup` 在主时间线中的职责；因为需求要求“每次行动独立”，工具不应继续以单个 group 汇总展示。
  - 若保留 `ProcessGroup`，只能用于某些连续、极短且同类的系统动作，但默认方案不依赖它。

### 4. 为每张行动卡片补充元信息与视觉边界

- 文件：`apps/agent-ide/public/components.jsx`
- 文件：`apps/agent-ide/public/styles.css`
- 变更内容：
  - 给每张卡片增加轻量头部元信息，例如：
    - 行动类型：思考 / 工具 / Todo / Plan / 输出
    - 状态：运行中 / 完成 / 失败
  - 增加统一类名，例如：
    - `.agent-action-list`
    - `.agent-action-card`
    - `.agent-action-card--reasoning`
    - `.agent-action-card--tool`
    - `.agent-action-card--output`
    - `.agent-action-card--running`
  - 通过边框、背景、间距、卡片阴影或分隔线，明确每次行动是独立框。

- 样式原则：
  - 保持当前主题变量体系，不新增样式文件。
  - 不把所有子组件重写一遍，而是在其外层卡片加视觉边界；必要时只补少量内部边距冲突修正。

### 5. 处理复制、空态和兼容逻辑

- 文件：`apps/agent-ide/public/components.jsx`
- 文件：`apps/agent-ide/public/interactions.jsx`
- 变更内容：
  - `handleCopyMessages()` 当前只拼 `b.text`，会漏掉工具/Todo/Plan 内容；改为基于新时间线结构串联每张卡片的可读文本摘要。
  - 历史消息的空态判断从 `blocks` 切换到 `cards/items`。
  - `LiveAssistantBubble` 在无事件时继续显示“正在连接 Agent…”，但一旦出现任一行动，就切到独立卡片时间线。

## Assumptions & Decisions

- 已确认用户要求：
  - 覆盖范围为“整轮全拆分”，不是只拆思考和最终输出。
  - 呈现方式为“同一轮内多卡片”，不是多条聊天消息。
- 本次规划不修改后端：
  - 后端事件已足够表达行动边界，问题主要在前端聚合方式。
  - 若后续发现 `agent.token.stream.delta` 与其他事件缺少稳定分段锚点，再追加前端启发式分段，不先改协议。
- 优先修改现有静态前端文件：
  - `apps/agent-ide/public/interactions.jsx`
  - `apps/agent-ide/public/components.jsx`
  - `apps/agent-ide/public/styles.css`
- 不新增依赖，沿用现有 React 组件与样式变量体系。

## Verification Steps

### 手工验证

- 启动前端后，发送一个会触发多步 ReAct 的请求，确认单轮回复内按顺序出现：
  - 思考卡
  - 工具卡 1
  - 工具卡 2
  - Todo/Plan 卡
  - 输出卡
- 确认每张卡片都有独立外框，而不是共享一个大消息体背景。
- 刷新页面或重新进入会话后，历史消息仍保持同样的多卡片结构。
- 在工具失败、权限拒绝、无文本输出、仅 Todo 更新等情况下，仍能渲染出正确的独立卡片。
- 复制消息后，文本内容包含每张卡片的摘要，不只剩最终输出。

### 代码校验

- 检查 `components.jsx` 与 `interactions.jsx` 的语法和运行时引用是否一致。
- 检查 `LiveAssistantBubble`、`ChatMessage`、`messagesFromEvents()`、`useChatStore.send()` 是否全部切到同一套时间线数据结构。
- 使用诊断工具检查被修改文件，确保无新增语法错误。
