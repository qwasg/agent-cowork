# 个人工作区净化收尾计划

## 摘要

目标是完成“删除初始占位展示内容，确保用户看到简洁个人工作区”的最后收尾。基于当前仓库现状，`apps/agent-ide/public/main.jsx`、`components.jsx`、`interactions.jsx`、`plan-views.jsx`、`panels.jsx`、`menu-schema.jsx` 中的大部分演示数据和 demo 文案已经完成替换；剩余工作集中在补齐新的空工作区样式、检查是否仍有遗漏的演示语义，并对本轮改动做诊断验证，确保首次进入时呈现真实、克制、可继续操作的个人工作区体验。

## 当前状态分析

### 1. 主要逻辑清理已落地

- `apps/agent-ide/public/main.jsx`
  - 已存在 `workspace-empty` 空态结构。
  - `termHistory` 已改为空数组。
  - `moonlit:groups` 已改为单组空标签默认值。
  - `Subagents` 无数据时已显示“暂无运行中的子代理”。
  - `DIFF_PLACEHOLDER` 已去掉伪代码说明内容。
- `apps/agent-ide/public/components.jsx`
  - 标题、About、用户卡默认名已改为通用工作区表述。
- `apps/agent-ide/public/interactions.jsx`
  - 无后端时已不再使用离线草稿回复，改为明确错误提示。
- `apps/agent-ide/public/plan-views.jsx`
  - DAG / Timeline 已改为真实空态。
  - Diff History 已只读取 `backend?.diffs || []`。
- `apps/agent-ide/public/panels.jsx`
  - 套餐、规则、技能、MCP、通知、资料默认信息已大幅去演示化。
- `apps/agent-ide/public/menu-schema.jsx`
  - About 菜单已同步为“关于 Moonlit 工作区”。

### 2. 样式层仍未和新空态完全对齐

- `apps/agent-ide/public/styles.css`
  - 当前已能看到大量基础 token 和组件样式，但还未确认存在 `workspace-empty`、`workspace-empty__card`、`workspace-empty__tips`、`workspace-empty--panel` 的样式定义。
  - 这意味着 JSX 虽然已经切到新空态，但最终视觉可能仍缺少结构、间距和弱引导层次。

### 3. 仍需做一次一致性排查

- `apps/agent-ide/public/interactions.jsx`
  - 仍存在“使用离线演示”的错误提示分支，需要确认这是否属于用户要求清理的演示语义残留。
- `apps/agent-ide/public/main.jsx`
  - 计划页头部和局部统计仍有 `live`、`Plan` 等英文混用，需要在执行时顺手核查是否与“简洁个人工作区”目标冲突。
- `apps/agent-ide/public/styles.css`
  - 新旧空态样式共存的情况下，需要确认没有残留仅服务于旧 demo 大卡片的样式引用。

## 拟议变更

### A. 补齐新的个人工作区空态样式

- 文件：`apps/agent-ide/public/styles.css`
- 变更：
  - 新增 `workspace-empty` 基础容器样式，使无标签主区和面板级空态都能稳定居中显示。
  - 新增 `workspace-empty__card`，提供轻量边框、背景、圆角和适度留白，但避免营销卡片感。
  - 新增 `workspace-empty__tips`，把“新建会话 / 打开工作区 / 创建文件”渲染成低干扰提示标签。
  - 新增 `workspace-empty--panel`，兼容右侧或局部面板中的空态尺寸和边距。
- 原因：
  - 现有 JSX 已切换到新空态，缺少 CSS 会导致首屏视觉散、层次弱，影响“简洁个人工作区”的完成度。
- 实现方式：
  - 复用现有 `--bg-panel`、`--line`、`--text-3`、`--accent-bg` 等设计 token。
  - 不新增依赖，不重做整体视觉体系，只做最小样式补齐。

### B. 清理剩余演示语义和不一致文案

- 文件：`apps/agent-ide/public/interactions.jsx`
- 变更：
  - 检查并移除“使用离线演示”这类错误提示中的演示措辞，统一改为真实失败态提示。
- 文件：`apps/agent-ide/public/main.jsx`
- 变更：
  - 复查工作区统计、页签标题、空态说明，确保无“演示运行中”“默认已执行过”的暗示。
  - 若存在无真实数据时仍显示固定计数、固定标签或过强教程语气，统一收口为中性空态。
- 文件：`apps/agent-ide/public/components.jsx`、`apps/agent-ide/public/panels.jsx`
- 变更：
  - 再做一次全文案复核，只处理仍明显偏 demo 的残留表述，不扩展需求。
- 原因：
  - 当前代码主体已清理，但少量残留措辞仍可能破坏整体感知。
- 实现方式：
  - 以“真实状态优先、无数据则空态、失败则直说失败”为统一原则。

### C. 做一次只覆盖本轮改动范围的诊断验证

- 文件：`apps/agent-ide/public/main.jsx`
- 文件：`apps/agent-ide/public/components.jsx`
- 文件：`apps/agent-ide/public/interactions.jsx`
- 文件：`apps/agent-ide/public/plan-views.jsx`
- 文件：`apps/agent-ide/public/panels.jsx`
- 文件：`apps/agent-ide/public/menu-schema.jsx`
- 文件：`apps/agent-ide/public/styles.css`
- 变更：
  - 对上述文件运行诊断，确认没有新增语法错误、未定义类名、JSX 结构问题或简单可修复的告警。
  - 若项目内已有便捷静态检查脚本，可补充一次针对前端静态资源的快速检查。
- 原因：
  - 这轮改动分散在多个静态前端文件中，最容易出现的是漏改 class、文案分支不一致和简单 JSX 问题。
- 实现方式：
  - 先用编辑器诊断确认文件级错误。
  - 再视仓库已有脚本情况决定是否执行轻量验证，不做额外工程化扩展。

## 假设与决策

- 决策：范围仍以“全应用演示内容”清理为准，但执行阶段只处理当前代码中确实残留的 demo 内容，不重新设计信息架构。
- 决策：空状态继续采用“保留少量引导”，不会做成完全空白，也不会恢复大块教程式卡片。
- 决策：不新增后端接口，不引入新依赖，只在现有前端静态文件中收尾。
- 决策：以 `apps/agent-ide/public` 为主修改目标，不同步处理 `dist` 目录手写副本，避免在源码确认前造成重复修改。
- 假设：`dist` 产物会由既有构建流程再生成，因此本次应优先保证 `public` 源文件正确。
- 假设：用户当前更关注首屏体验与默认空态，而不是把所有英文标签彻底本地化。

## 验证步骤

1. 首次进入工作区时，确认主内容区不再自动打开演示标签组合，只出现简洁空态与少量引导。
2. 在无真实会话、无 diff、无 swarm、无通知、无 todo 的情况下，确认界面不显示假日志、假通知、假子代理、假历史。
3. 断开后端后发送消息，确认不会生成伪助手回复，也不会出现“离线演示”类措辞。
4. 打开 `Plan`、`Diff History`、`Notifications`、`Profile`、`Settings` 等区域，确认只显示真实数据或中性空态。
5. 检查 `workspace-empty` 相关空态在主区和面板区的视觉效果，确认层次清晰但不过度装饰。
6. 对本轮涉及文件运行诊断，确保无新增错误；如有轻量静态检查脚本，再补充一次快速校验。
