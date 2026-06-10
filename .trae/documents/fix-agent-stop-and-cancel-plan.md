# 修复 Agent 运行中突然停掉问题计划

## 摘要
- 目标：修复 Agent 在运行一段时间后“突然停掉”的主要问题，范围包含后端主因和前端中止链路。
- 成功标准：
  - `build` / `debug` / `ask` / `multitask` 等对话模式不再因过短超时而频繁提前结束。
  - `plan` 执行链路里的子任务模型调用不再使用当前的硬编码短超时。
  - 当后端最终没有可提取文本时，不再发布“`agent.completed` + 空文本”，而是明确作为失败处理并向前端返回可见错误。
  - Composer 的“中止”按钮不再只清本地 UI，而是会尝试真正取消后端 run。

## 当前状态分析
- 后端对话模式超时偏短：
  - `backend/src/agent_debug/prompts/composer_mode_prompts.py`
  - 现状是 `build=15000`、`debug=20000`、`ask=12000`、`multitask=20000`、`plan=18000`。
  - 这些值会经 `backend/src/agent_debug/api/rest_gateway.py` 传入 `run_composer_chat()`。
- 后端 plan 子任务仍有单独的硬编码短超时：
  - `backend/src/agent_debug/domain/runtime.py`
  - `run_plan()` -> `_execute_task()` 中创建 `ModelRequestContext` 时写死 `timeout_ms=15000`。
- 后端允许“完成但无文本”：
  - `backend/src/agent_debug/domain/runtime.py`
  - `run_composer_chat()` 在 `extract_text_output(provider_output)` 失败时把 `text` 置为空字符串，随后仍写入 `run.status = "completed"` 并发布 `agent.completed`。
  - 历史 JSONL 已出现真实案例：`agent.completed` 的 `text` 为空。
- 前端中止链路只停本地展示：
  - `apps/agent-ide/public/interactions.jsx`
  - `useChatStore.send()` 中 `cancelRef.current` 只做 `setStreaming(null)` / `setLiveRun(null)` / toast，没有调用 `MoonlitAgentApi.cancelRun()`。
- 现有测试基础：
  - `backend/tests/agent_debug/test_composer_chat_streaming.py` 已覆盖 `run_composer_chat()` 正常完成、工具调用、历史恢复。
  - `backend/tests/agent_debug/test_rest_gateway.py` 已覆盖 `pause_run()` / `resume_run()` / `cancel_run()` 网关接口。
  - 前端没有针对 `interactions.jsx` 的现成单元测试入口，现有前端脚本以静态检查和 smoke test 为主，入口在 `apps/agent-ide/package.json`。

## 方案与改动

### 1. 放宽后端对话超时
- 文件：`backend/src/agent_debug/prompts/composer_mode_prompts.py`
- 修改内容：
  - 将模式超时调整为更适合真实代码排查的范围：
    - `build` -> `60000`
    - `debug` -> `90000`
    - `ask` -> `45000`
    - `multitask` -> `120000`
    - `plan` -> 保持定义但同步提升到 `120000`，避免后续复用该 profile 时仍带旧值
- 原因：
  - 当前 12s 到 20s 的预算过短，复杂推理和多次工具调用非常容易触发 provider 超时。
- 实现方式：
  - 仅修改 profile 常量，不改网关调用方式，保持 `rest_gateway.py` 现有接口和数据流不变。

### 2. 放宽 plan 子任务的模型超时
- 文件：`backend/src/agent_debug/domain/runtime.py`
- 修改内容：
  - 将 `_execute_task()` 里创建 `provider_ctx` 的 `timeout_ms=15000` 改为统一的较高值 `60000`。
  - 抽成明确常量，例如 `_PLAN_TASK_TIMEOUT_MS = 60_000`，避免再次出现散落硬编码。
- 原因：
  - 当前即使对话模式超时放大，plan 执行仍会被这条 15s 短超时截断。
- 实现方式：
  - 只改计划执行链路的 provider 超时，不改变 todo / plan 的状态机语义。

### 3. 把“空文本完成”改成明确失败
- 文件：`backend/src/agent_debug/domain/runtime.py`
- 修改内容：
  - 在 `run_composer_chat()` 里，`extract_text_output(provider_output)` 失败或返回空文本时，不再：
    - 写 `run.status = "completed"`
    - 发布 `agent.completed`
    - `_remember_turn(session_id, user_message, text)` 写入空 assistant 轮次
  - 改为：
    - 将 run 状态记为 `failed`
    - 发布 `agent.failed`
    - 错误 payload 使用固定错误码/消息，例如 `EMPTY_ASSISTANT_OUTPUT`
    - HTTP 返回显式错误文案，例如“模型已结束，但未生成可展示文本，请重试或检查 Provider 输出协议”
- 原因：
  - 用户明确要求“空文本时明确报错”，且这是当前“看起来突然停掉”的关键表现之一。
- 实现方式：
  - 只在最终无可展示文本时切换为失败；正常有文本的完成路径保持不变。
  - 不新增接口字段；继续复用现有 `{ message: { text }, run: { id } }` 返回形状，保证前端兼容。

### 4. 补充后端回归测试
- 文件：`backend/tests/agent_debug/test_composer_chat_streaming.py`
- 修改内容：
  - 新增用例：provider 最终只产生工具调用或无文本输出时，`run_composer_chat()` 返回明确错误文案，事件流包含 `agent.failed`，且不出现空文本 `agent.completed`。
  - 新增用例：若 provider 正常返回文本，既有完成语义保持不变，防止回归。
- 原因：
  - 当前测试只覆盖“正常有文本完成”，没有锁住“空文本必须失败”的新语义。
- 实现方式：
  - 复用该文件中已有的假 provider 模式，新增一个“最终无文本”的 provider stub。

### 5. 让前端中止真正调用后端取消
- 文件：`apps/agent-ide/public/interactions.jsx`
- 修改内容：
  - 在 `useChatStore.send()` 内部为本轮请求维护可解析的 `runId`。
  - `cancelRef.current` 改为异步中止流程：
    - 优先从当前轮次的 WS 事件缓存里查找 `agent.started` / `correlationId` 对应的 `runId`
    - 若已拿到 `runId`，调用 `window.MoonlitAgentApi.cancelRun(runId)`
    - 无论取消请求是否成功，都清理 `streaming` / `liveRun`
    - toast 明确区分“已请求中止后端运行”和“仅停止本地展示，未拿到运行 ID”
  - 在 `askExecute` resolve 后，如果请求早已被用户中止，不再继续渲染最终消息。
- 原因：
  - 当前前端的“中止”只是隐藏 UI，无法真正停止后端。
- 实现方式：
  - 不改 API 形状，不新增后端接口。
  - 运行 ID 来源采用现有 WS 事件中的 `correlationId` / `agent.started`，避免改动 `ask_execute` 的返回时序。
  - 若用户点击极快、WS 尚未收到 `agent.started`，允许进入“本地已停止展示，但后端取消未确认”的兜底提示；不为此新增轮询或后台重试逻辑，保持实现最小化。

### 6. 前端失败展示保持兼容但更明确
- 文件：`apps/agent-ide/public/interactions.jsx`
- 修改内容：
  - 保持现有 `catch` 路径的错误渲染方式。
  - 由于后端现在会在“空文本”时返回明确错误文案，前端无需额外引入新分支，只需确保该文案按现有错误块显示即可。
- 原因：
  - 用户要求空文本明确报错，后端返回文案后，前端现有错误展示已经足够承接。
- 实现方式：
  - 不改 `buildAgentBlocksFromEvents()` 的数据结构，不扩展新的 block 类型。

## 假设与决策
- 决策：本次按“后端主因 + 前端中止”执行，不扩大到更多监控、日志面板或额外 UX 重构。
- 决策：空文本最终结果按失败处理，不再伪装成成功完成。
- 决策：前端中止采用最小接线方案，依赖现有事件流获取 `runId`，不引入新的后端立即返回 run-id 协议。
- 假设：`agent.started` 事件会在大多数真实交互中早于用户中止操作到达前端；极早点击中止时允许进入“未确认后端取消”的兜底提示。
- 假设：现有 `run.status`、`agent.failed` 事件语义已被前端和测试接受，不需要新增专门的 `agent.cancelled` 或 `agent.empty_output` 事件类型。

## 验证步骤
- 后端测试：
  - 运行 `py -3 -m pytest backend/tests/agent_debug/test_composer_chat_streaming.py -q`
  - 运行 `py -3 -m pytest backend/tests/agent_debug/test_rest_gateway.py -q`
- 前端静态检查：
  - 运行 `npm --prefix apps/agent-ide test`
  - 运行 `npm --prefix apps/agent-ide lint`
- 手工验收：
  - 在 `build` / `debug` 模式发起一个需要多次工具调用的请求，确认不会在 12s 到 20s 左右无故结束。
  - 触发一个“最终无文本”的后端路径，确认前端展示明确错误，不再出现空白回复。
  - 发送请求后点击 Composer 中止，确认前端会请求后端取消；若能拿到 `runId`，当前运行面板中的 run 状态应变为取消态。
  - 再次发送新请求，确认正常完成路径与历史消息恢复逻辑未回归。
