# agentd — Rust Agent 核心

Agent Debug 后端的 Rust 实现（默认运行栈），与 `gateway-go` 边缘网关配套使用。
Python 版本（`../backend`）仅作为参考实现保留。

## 架构

```
agent-ide (GPUI) ──HTTP/WS/SSE──▶ gateway-go (:8002) ──HTTP──▶ agentd (:8003)
```

- **agentd**（本目录）：会话 / Plan DAG / Todo / Agent 回合循环 / 工具系统 /
  Proposal / Checkpoint / Swarm / MCP / 长期记忆 / 多厂商 LLM Provider。
- **gateway-go**：CORS、JWT 鉴权、反向代理、WS→SSE 桥接。

## 三类 Agent Profile

平台在同一套运行时上提供三类定位不同的 Agent，由会话的 `agentKind` 决定
（创建会话时传入，旧会话缺省为 `coding`）。提示词工程、工具白名单与可用的
composer 模式按 profile 区分，定义见 [`crates/agent-core/src/profile.rs`](crates/agent-core/src/profile.rs)
与提示词包 [`crates/agent-core/src/prompts/`](crates/agent-core/src/prompts/)。

| agentKind | 定位 | 工具白名单 | composer 模式 |
| --- | --- | --- | --- |
| `coding` | Vibe coding 编码代理（默认） | 全部工具 + 结构化编辑 + todo + task | build / plan / debug / ask / multitask |
| `document` | 文档撰写与整理 | 只读 + web + `write_file` / `create_document` + todo + task（无 `run_command`、无代码补丁） | ask / build |
| `general` | 通用对话助手 | 只读 + web + task（不写文件、不执行命令） | ask / build |

非 `coding` 会话传入不支持的模式（如 `plan`/`debug`/`multitask`）会自动降级为
`build`，且不会走 Plan 引擎，直接进入对话回合循环。

## 长期记忆

跨会话的结构化记忆存于 redb（`T_MEMORIES`，按 `scope` 建立二级索引），服务层见
[`crates/agent-core/src/memory.rs`](crates/agent-core/src/memory.rs)：

- **scope**：`global` / `workspace:{root}` / `session:{id}`。
- **kind**：`preference` / `fact` / `convention`。
- 检索为零外部依赖的关键词打分（ASCII 词元 + 中文字符 bigram），离线可用；
  命中计数 + 新近度参与排序与容量淘汰（每 scope 上限 200 条）。
- Agent 通过运行时工具 `memory_write` / `memory_search` / `memory_delete` 读写；
  每轮会用用户输入检索 Top-N 注入到系统提示的「相关记忆」区块。
- 管理 API：`GET/POST /api/agent-debug/memories`、`PATCH/DELETE /api/agent-debug/memories/{id}`。

## Workspace 布局

```
backend-rs/
├── crates/
│   ├── agent-config/     # 环境变量配置
│   ├── agent-protocol/   # wire 契约：DTO、事件信封、错误码
│   ├── agent-store/      # redb KV、JSONL 事件日志、事件总线、加密
│   ├── agent-providers/  # LLM provider 抽象 + 多厂商适配器
│   ├── agent-mcp/        # MCP 客户端
│   ├── agent-tools/      # Agent 工具注册表（fs/command/web/skill）
│   └── agent-core/       # 领域层：会话、plan/todo 引擎、Agent 运行时
└── bins/
    └── agentd/           # HTTP + SSE 服务（axum）、API handlers、集成测试
```

## 本地运行

```powershell
# 1. Agent 核心（默认监听 127.0.0.1:8003 = 公开端口 + 1）
cargo run --bin agentd

# 2.（可选）Go 边缘网关
cd ../gateway-go
$env:AGENT_CORE_URL = "http://127.0.0.1:8003"; go run .
```

Docker 方式（推荐，公开端口为网关的 8002）。须在**仓库根目录**执行（构建上下文需包含 `rust/crates/moonlit-*`）：

```bash
docker compose up --build
```

## 常用环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `AGENT_DEBUG_HOST` / `AGENT_DEBUG_HTTP_PORT` | `127.0.0.1` / `8002` | 监听地址 |
| `AGENT_DEBUG_DATA_DIR` | `.` | redb + JSONL 数据目录 |
| `AGENT_DEBUG_WORKSPACE_ROOT` | 当前目录 | 工具操作的工作区根 |
| `OPENAI_API_KEY` / `OPENAI_BASE_URL` / `OPENAI_MODEL` | — | 默认 LLM 渠道（缺省回退 mock provider） |
| `ANTHROPIC_API_KEY` | — | Anthropic 渠道 |
| `TAVILY_API_KEY` | — | Web 搜索 |
| `AGENT_DEBUG_STREAM` | `1` | LLM 流式输出 |
| `AGENT_DEBUG_ALLOW_LOCAL_FS` | off | 允许 `/local-file`、`/workspace/browse` 越出工作区 |

完整列表见 `crates/agent-config/src/lib.rs`。

## MCP demo server

`bins/agentd/src/bin/mcp_demo_server.rs` 是 stdio MCP 演示服务器（`add` / `echo` / `reverse`
三个工具），由 agentd 自动发现并以 `mcp__demo__*` 名称注入 Agent 工具循环：

```bash
cargo build --bin mcp-demo-server
```

## 测试

```bash
cargo test
```
