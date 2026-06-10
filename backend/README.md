# Moonlit Document Compiler Backend

轻量后端，提供文档处理与 AI 辅助编译能力（FastAPI + WebSocket）。

## 核心能力

- 会话管理（Session / Replay）
- 计划与待办编排（Plan / Todo）
- 文档与代码编辑提案（Proposal Apply/Discard）
- 工作区文件树与读写接口
- AI Provider 适配（OpenAI-compatible / mock）

## 依赖策略（轻量化）

已移除旧引擎相关重依赖，仅保留服务所需组件：

- `fastapi`
- `uvicorn`
- `openai`
- `httpx`
- `websockets`

## 本地运行

```bash
pip install -r requirements.txt
python -m src.agent_debug.server
```

默认监听（Document Compiler 副本专用，**非**原项目 `8001`）：

- `http://127.0.0.1:8002`

健康检查：

```bash
curl http://127.0.0.1:8002/health
```

CORS 默认允许前端 `http://localhost:8030`（见 `server.py`）。配套 IDE 勿使用原项目端口 `3000` / `8001`。

## 入口脚本

- `agent-debug-server`
- `moonlit-doc-compiler`
