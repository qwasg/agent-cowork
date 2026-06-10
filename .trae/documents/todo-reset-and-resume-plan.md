# 规划：todo 完成后默认重置隐藏，且中断任务可继续

## Summary

- 目标：
  - 任务成功完成后，agent 生成的 todo 不再继续污染默认 todo 表现与下一轮 `write_todos` 规划。
  - 任务中断时（按你的选择：暂停 / 失败 / 取消），当前 todo 仍然保留，后续可以继续承接。
- 成功标准：
  - 成功完成的 agent todo 默认不再出现在会话快照 / Todo 看板的默认列表里。
  - 新一轮 agent 规划不会再按标题误复用上一轮“已完成”的 todo。
  - 中断态 todo 仍会作为当前可继续任务保留并显示，下一轮可继续复用。
  - 不需要把 todo 注入模型 prompt；问题修复聚焦在 todo 生命周期、快照过滤和 run/session 绑定。

## Current State Analysis

### 1. 旧 todo 会长期留在 session 默认视图中

- `backend/src/agent_debug/api/rest_gateway.py`
  - [get_design_snapshot](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L325-L369) 当前直接把 `self.todo_engine.list_by_session(active_session.id)` 的全部结果放进返回值 `todos`。
  - [get_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L780-L782) 同样直接返回整个 session 下的 todo 列表。
- 结果：
  - Todo 看板、顶部指标、TodoStrip 都基于“会话内所有历史 todo”计算，成功完成后的 agent todo 不会默认消失。

### 2. 新一轮 `write_todos` 会和旧 todo 共用索引，存在被历史结果干扰的风险

- `backend/src/agent_debug/domain/todo_engine.py`
  - [sync_agent_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/todo_engine.py#L205-L297) 会先遍历当前 session 下全部 `source == "agent"` 的 todo，构建标题索引 `title_index`。
  - 这意味着只要标题接近，新一轮任务就可能按标题复用旧 todo，即使旧 todo 已属于上一轮成功任务。
- 结果：
  - “确保 AI 每次制定时不受影响”当前并不成立，干扰点不在 prompt，而在 todo 对账逻辑。

### 3. 成功完成会自动补全 todo，但不会把它们从默认工作集里移走

- `backend/src/agent_debug/domain/runtime.py`
  - [run_composer_chat](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L1149-L1178) 成功结束时会调用 `complete_agent_todos_for_run(run.id)`，把当前 run 遗留的 agent todo 自动收尾为 `completed`。
- 结果：
  - 当前已经解决了“任务结束后还停留在进行中”的问题，但没有解决“成功完成后默认重置隐藏”的问题。

### 4. 中断任务要“继续”还缺一层 session/run 生命周期绑定

- `backend/src/agent_debug/api/rest_gateway.py`
  - [execute_plan](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L710-L756) 会调用 `self.sessions.update_active_run(...)`，但 chat/composer 链路 [\_ask_composer_message](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L1200-L1234) 只是直接调用 `runtime.run_composer_chat()`，没有把 run 最终状态回写到 session。
- `backend/src/agent_debug/domain/session_service.py`
  - [update_active_run](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/session_service.py#L131-L141) 已有能力更新 `active_run_id`，但 composer 路径目前没有系统使用。
- 结果：
  - 如果要让“暂停 / 失败 / 取消”的 todo 在默认视图中作为“可继续任务”存在，仅靠 todo 表本身不够，session 还需要知道当前可继续的 run。

### 5. 这不是 prompt 污染问题

- `backend/src/agent_debug/prompts/system_prompt_assembly.py`
  - [assemble_composer_system_message](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/prompts/system_prompt_assembly.py#L116-L149) 只组装工作区、AGENT.md、context window 等动态上下文，没有把 todo 列表塞进 prompt。
- 结论：
  - 本次修复不需要改 prompt，重点是 todo 的“默认工作集”与“历史归档集”分离。

## Proposed Changes

### 1. 为 agent todo 引入“可继续分组”和“默认隐藏历史”元数据

- 文件：`backend/src/agent_debug/domain/models.py`
- 文件：`backend/src/agent_debug/domain/todo_engine.py`
- 变更内容：
  - 在 `TodoItem` 上新增面向 agent todo 生命周期的元数据，建议最小化增加两类字段：
    - 一个稳定分组字段，例如 `agent_todo_group_id`
      - 用于标识同一批可继续的 todo 工作集。
      - 成功完成前后、以及中断后继续时，组内 todo 可以跨 run 延续。
    - 一个默认隐藏标记，例如 `archived_at` 或 `hidden_from_default_views`
      - 用于区分“历史已完成工作集”和“当前/可继续工作集”。
  - 在 `TodoEngine` 内新增辅助方法，至少包括：
    - 解析 session 当前“可继续 agent todo 组”
    - 列出默认可见 todo
    - 成功完成后归档某个 agent todo 组

- 设计规则：
  - 成功完成的 agent todo 组：进入“历史已完成组”，默认隐藏。
  - 暂停 / 失败 / 取消的 agent todo 组：保留为“可继续组”，默认可见。
  - 用户手工 todo / plan 执行 todo：不套用本次“成功即默认隐藏”的规则，避免误伤其他来源。

### 2. 收紧 `write_todos` 的复用范围，只允许复用当前可继续工作集

- 文件：`backend/src/agent_debug/domain/todo_engine.py`
- 变更内容：
  - 调整 [sync_agent_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/todo_engine.py#L205-L297) 的 client-id / title 复用逻辑：
    - 不再从整个 session 的全部 agent todo 建标题索引。
    - 只在“当前可继续的 agent todo 组”内做 id/标题匹配。
  - 当 session 中存在未归档的中断组时，新一轮 `write_todos` 继续绑定到这个组。
  - 当不存在可继续组时，创建新的组，避免误复用上一个成功任务。

- 预期效果：
  - 成功任务结束后，下一轮 AI 会从全新的工作集开始制定。
  - 中断任务再次继续时，旧 todo 会被正确续写，而不是另起一套或错误匹配历史完成项。

### 3. 成功完成时归档 todo 组；中断时保留 todo 组

- 文件：`backend/src/agent_debug/domain/runtime.py`
- 变更内容：
  - 保留现有 [自动完成当前 run todo](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/domain/runtime.py#L1152-L1169) 的逻辑。
  - 在 composer run 进入成功完成分支后，增加“归档当前 agent todo 组”的步骤：
    - 当前 run 关联组全部完成后，标记该组为默认隐藏。
  - 对 `paused` / `failed` / `cancelled` 分支不做归档，保持默认可见。

- 关键原因：
  - 成功态要“重置默认工作集”。
  - 中断态要“保留恢复入口”。

### 4. 让 composer run 的 session 指针与默认 todo 工作集保持一致

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
- 文件：`backend/src/agent_debug/domain/session_service.py`
- 变更内容：
  - 在 [\_ask_composer_message](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L1200-L1234) 调用 `runtime.run_composer_chat()` 返回后，读取 `run.id` / `run.status` 并回写 session：
    - 成功完成：清空 `active_run_id`，session 状态回到空闲或完成态。
    - 暂停 / 失败 / 取消：保留 `active_run_id = run.id`，使 session 能指向当前可继续 run。
  - 这样刷新快照后，系统仍能知道当前默认展示的是哪个可继续任务上下文。

- 说明：
  - 本次计划不依赖真正的“暂停后进程内继续执行”语义，而是确保“中断后的 todo 工作集和 run 指针被正确保留”，供后续继续承接。

### 5. 默认快照和 Todo API 只返回“当前工作集”，不再把历史成功 todo 混进默认界面

- 文件：`backend/src/agent_debug/api/rest_gateway.py`
- 变更内容：
  - 修改 [get_design_snapshot](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L325-L369) 和 [get_todos](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L780-L782)：
    - 改为返回 `TodoEngine` 计算出的默认可见 todo，而不是整个 session 的原始 todo。
  - 同步让 [\_build_design_metrics](file:///h:/agent-debug-frontend-backend-copy-20260530/backend/src/agent_debug/api/rest_gateway.py#L1303-L1341) 基于默认可见 todo 统计，这样 `Todos x/y`、TodoStrip、看板列数都会自动恢复正确含义。

- 前端影响：
  - `apps/agent-ide/public/main.jsx` 和 `apps/agent-ide/public/components.jsx` 已经消费 `backend.todos` / `metrics.todos`，只要后端默认返回值变干净，前端通常无需额外改动。
  - 本次优先不改前端结构，只修正后端默认数据面。

### 6. 补回归测试，锁定“成功隐藏、失败保留、规划不串台”三个关键行为

- 文件：`backend/tests/agent_debug/test_todo_engine.py`
- 变更内容：
  - 新增单测覆盖：
    - 成功归档后的 todo 组不再参与下一轮标题匹配。
    - 中断组会被识别为当前可继续组，并在后续 `write_todos` 中被复用。
    - 默认可见 todo 只包含未归档 / 可继续的工作集。

- 文件：`backend/tests/agent_debug/test_composer_chat_streaming.py`
- 变更内容：
  - 新增或扩展测试覆盖：
    - 成功结束后，run 关联的 todo 会被自动完成并转入默认隐藏状态。
    - 失败 / 取消时，todo 不被归档，仍保留为可继续状态。

- 文件：`backend/tests/agent_debug/test_rest_gateway.py`
- 变更内容：
  - 新增 gateway 级测试覆盖：
    - `get_design_snapshot()` 在成功 run 后默认不再返回该批完成 todo。
    - 中断 run 后，`get_design_snapshot()` 仍返回该批 todo，且 session `active_run_id` 指向可继续 run。
    - 新一轮 composer 任务不会误复用已归档成功组的标题。

## Assumptions & Decisions

- 已确认 composer prompt 本身不注入 todo，因此本次不改 prompt，只改 todo 生命周期与默认快照逻辑。
- 你已明确选择：
  - 成功任务：旧 agent todo 默认隐藏历史，不干扰后续任务。
  - 中断任务：暂停 / 失败 / 取消都保留，以便继续。
- 本次“继续”优先落在“保留并复用 todo 工作集 + session 指针恢复”上，而不是扩展新的交互协议。
- 默认视图隐藏历史，不等于彻底删除历史；历史仍可保留在后端记录 / 事件流中，满足后续追溯需要。

## Verification Steps

1. 成功完成场景：
   - 发起一轮 composer 任务，让 agent 写入并完成若干 todo。
   - 确认默认 `snapshot.todos` / TodoStrip / 看板不再显示这批已完成 agent todo。
   - 再发起一轮新任务，确认 `write_todos` 从新的工作集开始，不会复用上一轮同名完成项。

2. 中断保留场景：
   - 构造 `paused` / `failed` / `cancelled` 的 composer run。
   - 刷新快照后确认：
     - 对应 todo 仍然可见；
     - session 仍保留 `active_run_id`；
     - 下一轮继续时，旧 todo 会被复用而不是丢失。

3. 指标一致性：
   - 验证 `metrics.todos.completed/total`、TodoStrip 和看板列数只基于默认可见 todo 统计。

4. 回归检查：
   - `test_todo_engine.py`
   - `test_composer_chat_streaming.py`
   - `test_rest_gateway.py`
   - 重点确认不会破坏现有 plan 执行链路与用户手工 todo。
