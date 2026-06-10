# 规划：修复 agent 完成后未自动标记 todo 完成

## Summary

- 目标：修复 composer 对话模式下，agent 已经产出最终回答并结束 run，但本轮通过 `write_todos` 创建/更新的 todo 仍停留在 `running` / 非完成态的问题。
- 成功标准：
  - 当 `run_composer_chat()` 成功结束并发布 `agent.completed` 时，本轮仍未完成的 agent-authored todo 会被自动收尾为完成态。
  - UI 无需额外改动即可显示正确的 `TODO x / y` 进度，因为前端已经消费 `todo.completed` 和后端 `snapshot.todos`。
  - 不影响 plan 执行链路 `run_plan()` 的既有 todo 生命周期。
  - 为该行为补充后端回归测试，覆盖事件发布和状态持久化。

## Current State Analysis

### 问题定位

- `backend/src/agent_debug/domain/runtime.py`
  - [run_composer_chat](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L980-L1091) 在 `_run_react_loop()` 返回最终文本后，直接把 run 置为 `completed` 并发布 `agent.completed`。
  - 该流程没有任何“todo 收尾”逻辑，因此只有模型再次主动调用 `write_todos`，todo 才会变成 `completed`。
  - 这与截图现象一致：agent 已答复结束，但 TodoStrip 仍显示 `0 / 1`。

- `backend/src/agent_debug/domain/runtime.py`
  - [_handle_write_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L866-L946) 只负责处理模型显式发出的 `write_todos` 调用，并按 `created/updated/running/completed` 发布事件。
  - 也就是说，当前系统把 todo 生命周期完全托付给模型，没有成功结束时的兜底机制。

- `backend/src/agent_debug/domain/todo_engine.py`
  - [sync_agent_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/todo_engine.py#L188-L286) 会把模型状态映射到内部状态，并在创建 agent todo 时写入 `related_agent_run_id=run_id`。
  - 但对已存在 todo，只在 `related_agent_run_id` 为空时才补写 run id；复用旧 todo 时，当前 run 与 todo 的关联并不稳定，不利于 run 结束时精确收尾。

### 前端现状

- `apps/agent-ide/public/components.jsx`
  - [TodoStrip](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/components.jsx#L1421-L1465) 仅根据 `todo.status === "completed"` 统计已完成数。
  - 所以只要后端状态和事件正确，前端现有逻辑就会自动恢复正常，无需单独修 UI。

- `apps/agent-ide/public/interactions.jsx`
  - [todo 事件聚合](file:///h:/agent-debug-frontend-backend-copy-20260530/apps/agent-ide/public/interactions.jsx#L508-L524) 已消费 `todo.completed` / `todo.updated`。
  - 说明本问题不是事件展示链路缺失，而是后端没有在 run 成功结束时发出对应 todo 完成事件。

## Proposed Changes

### 1. 为 agent-authored todo 增加“按 run 收尾”的后端能力

- 文件：`backend/src/agent_debug/domain/todo_engine.py`
- 变更内容：
  - 新增一个面向 composer chat 的收尾辅助方法，例如“列出或完成某个 `run_id` 下仍未终态的 agent todo”。
  - 仅处理 `source == "agent"` 且状态不在终态集合内的 todo，避免误动用户手工 todo 或 plan 驱动 todo。
  - 返回被自动完成的 todo 列表，供 runtime 逐条发布 `todo.completed` 事件。

- 设计决策：
  - 终态仍沿用现有 `_TERMINAL_STATUSES`。
  - 自动完成后统一落到内部状态 `completed`，不引入新状态。

### 2. 强化 todo 与当前 run 的关联，确保自动收尾可精确命中

- 文件：`backend/src/agent_debug/domain/todo_engine.py`
- 变更内容：
  - 调整 [sync_agent_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/todo_engine.py#L188-L286) 中对已存在 agent todo 的 run 关联逻辑。
  - 当本次 `write_todos` 由某个 `run_id` 发起时，即使 todo 是按标题或 client id 复用的，也要把 `related_agent_run_id` 更新为当前 run。

- 原因：
  - 当前实现只在字段为空时设置 `related_agent_run_id`，会导致复用旧 todo 时，当前 run 无法稳定回收自己这轮维护的 todo。
  - 更新到“最后一次维护该 todo 的 run”更符合本次自动收尾场景，且仓库内暂未发现其他依赖旧 run id 不变的逻辑。

### 3. 在 composer 对话成功结束时自动完成剩余 todo

- 文件：`backend/src/agent_debug/domain/runtime.py`
- 变更内容：
  - 在 [run_composer_chat 成功分支](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L1078-L1091) 中，`agent.completed` 发布前增加一个 todo 收尾步骤。
  - 调用第 1 步新增的 todo engine 辅助方法，收集本 run 仍未完成的 agent todo。
  - 对每个被收尾的 todo：
    - 更新状态为 `completed`
    - 同步写入 `run.completed_todo_ids`
    - 必要时从 `run.active_todo_ids` 中移除
    - 发布 `todo.completed` 事件，payload 仍使用完整 `TodoItem`

- 行为边界：
  - 只在成功产出最终文本并进入 `agent.completed` 分支时执行。
  - `agent.failed`、`cancelled`、工具循环耗尽等异常分支先不自动改 todo，保留现场，避免把中断任务误判为已完成。

### 4. 补充回归测试，锁定自动收尾行为

- 文件：`backend/tests/agent_debug/test_todo_engine.py`
- 变更内容：
  - 新增针对 todo engine 收尾辅助方法的单元测试。
  - 覆盖：
    - 仅自动完成 `source == "agent"` 的 todo
    - 仅命中指定 `related_agent_run_id`
    - 已是 `completed/failed/cancelled/...` 的 todo 不重复处理

- 文件：`backend/tests/agent_debug/test_composer_chat_streaming.py`
- 变更内容：
  - 新增一个 composer chat 集成测试：
    - provider 先调用 `write_todos` 写入至少 1 个 `in_progress` todo
    - 随后直接返回最终文本，不再显式调用 `write_todos` 标记完成
    - 断言最终事件流中出现 `agent.completed` 和自动补发的 `todo.completed`
    - 断言 `todo_engine.list_by_session()` 中对应 todo 已持久化为 `completed`
    - 断言 TodoStrip 依赖的数据面，即后端 `todos` 状态，已满足 `completed`

## Assumptions & Decisions

- 本次修复范围限定在 composer 对话链路，不改 `run_plan()` 的计划执行逻辑。
- 自动完成的对象限定为“当前 run 最后一次维护的 agent todo”，不波及用户手工创建 todo。
- 成功结束时自动补齐 todo 完成态，失败或取消时不做兜底自动完成。
- 前端已有正确的消费逻辑，因此优先做最小后端修复，不引入不必要的 UI 改动。

## Verification Steps

1. 运行 `backend/tests/agent_debug/test_todo_engine.py`，确认新增 todo 收尾单测通过。
2. 运行 `backend/tests/agent_debug/test_composer_chat_streaming.py` 中新增场景，确认：
   - agent 结束后自动发出 `todo.completed`
   - todo 最终状态为 `completed`
3. 回归已有 `write_todos` 相关测试，确保：
   - 显式 `write_todos` 完成态仍正常
   - 超过旧 step cap、上下文压缩等场景不受影响
4. 如需手工验证，在 UI 中复现“单个 todo 进入 `in_progress` 后 agent 直接回答完成”的流程，确认 TodoStrip 从 `0 / 1` 变为 `1 / 1`。
