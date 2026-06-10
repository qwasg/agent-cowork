# 前端添加“联网搜索”开关计划

## Summary

- 目标：在前端输入框（Composer）工具栏内添加一个明显可见的“联网搜索”开关，以便用户可以直接在聊天界面控制 `webSearchEnabled` 会话属性，从而允许或禁止模型使用 `web_search` 和 `web_fetch` 工具。
- 当前状态：`webSearchEnabled` 只能在创建会话时（或部分隐藏的侧边栏中）设置，Composer 周边没有快速切换入口。
- 方案：在 `components.jsx` 中，向 `Composer` 组件注入会话的 `webSearchEnabled` 状态及切换回调，并在模型选择器（`cmb-model-wrap`）左侧渲染一个状态清晰的开关按钮。

## Proposed Changes

### 1. 更新 `Composer` 组件接收和渲染开关

- 文件：`apps/agent-ide/public/components.jsx`
- 更改：
  - 给 `Composer` 增加 `webSearchEnabled` 和 `onWebSearchToggle` 两个 prop。
  - 在渲染底部工具栏（`cmb-model-wrap` 左侧或之前），添加一个按钮。
  - 按钮图标可以使用 `globe`。
  - 当 `webSearchEnabled` 为 true 时，赋予它一个高亮样式（如 `is-active` 或通过内联样式改变颜色），并调整 tooltip 或文本提示为“联网搜索已开启”。
  - 当点击按钮时，调用 `onWebSearchToggle()`，并调用 `keepComposerFocus` 保持焦点。

### 2. 在 `ChatColumn` 中传递状态和方法

- 文件：`apps/agent-ide/public/components.jsx`
- 更改：
  - 在 `ChatColumn` 内，通过 `const webSearchEnabled = activeSession?.webSearchEnabled === true;` 获取状态。
  - 增加一个 `toggleWebSearch` 函数：
    ```javascript
    const toggleWebSearch = async () => {
      if (!activeSession?.id) return;
      try {
        await window.MoonlitAgentApi?.patchSession(activeSession.id, { webSearchEnabled: !webSearchEnabled });
        await app?.refreshBackend?.(activeSession.id);
        app?.toast?.(`联网搜索已${!webSearchEnabled ? "开启" : "关闭"}`, "ok");
      } catch (err) {
        app?.toast?.(`切换失败: ${err.message || err}`, "err");
      }
    };
    ```
  - 把 `webSearchEnabled={webSearchEnabled}` 和 `onWebSearchToggle={toggleWebSearch}` 传给渲染的 `Composer` 组件。

## Assumptions & Decisions

- **图标与位置**：放置在模型选择器左侧是最合理的，因为它们都属于“本次对话的运行时配置”。图标选用 `globe` 符合联网直觉。
- **状态刷新**：直接复用现有的 `app.refreshBackend(activeSession.id)` 来确保 UI 上的 `activeSession` 数据能够及时反映后端 patch 后的新状态。

## Verification Steps

1. 修改并保存 `components.jsx`。
2. 因为是 Vite 环境，前端会自动热更新。
3. 检查页面中输入框下方是否出现了“联网搜索”开关。
4. 点击该开关，观察是否弹出 toast 提示，并且图标状态发生变化。
5. （可选）发一条需要联网的消息进行端到端确认。