# gateway-go — Go 边缘网关

Agent Debug 后端的公开入口（默认 `:8002`），把请求反代到 Rust 核心
`agentd`（默认 `:8003`），并承担：

- CORS（`AGENT_DEBUG_ALLOW_ORIGINS`）
- JWT / 静态 token 鉴权（`AGENT_DEBUG_AUTH_TOKEN`，REST + WS）
- WebSocket → SSE 桥接与事件回放

## 运行

```bash
AGENT_CORE_URL=http://127.0.0.1:8003 go run .
```

或随整套栈启动：

```bash
docker compose up --build   # 仓库根目录
```

## 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `AGENT_DEBUG_HOST` / `AGENT_DEBUG_HTTP_PORT` | `127.0.0.1` / `8002` | 监听地址 |
| `AGENT_CORE_URL` | `http://127.0.0.1:8003` | agentd 上游地址 |
| `AGENT_DEBUG_AUTH_TOKEN` | 空（关闭鉴权） | 开启鉴权 |
| `AGENT_DEBUG_ALLOW_ORIGINS` | `http://localhost:8030` | 允许的前端来源 |

## 测试

```bash
go test ./...
```
