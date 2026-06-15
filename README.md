# Agent Cowork

An agent system with a native desktop IDE, Rust agent backend, and document editing workspace.

## Architecture

```
moonlit-agent-ide (GPUI) ──HTTP/WS/SSE──▶ gateway-go (:8002) ──HTTP──▶ agentd (:8003)
                                              │
                                              └── workspace / sessions / tools / MCP
```

| Component | Path | Role |
| --- | --- | --- |
| **Agent IDE** | `rust/apps/agent-ide` | Native GPUI desktop shell: chat, workbench, settings, embedded DocForge |
| **DocForge** | `rust/apps/docforge` | Word / PPT editor with CRDT sync, compile & preview |
| **agentd** | `backend-rs` | Rust agent core: sessions, plan/todo, tools, MCP, memory, LLM providers |
| **gateway-go** | `gateway-go` | Public edge: CORS, auth, reverse proxy, WS→SSE bridge |
| **Python backend** | `backend` | Legacy reference implementation (not the default stack) |

## Quick start (Windows)

### 1. Backend

```powershell
powershell -File scripts/start-stack.ps1
```

This builds and starts:

- `agentd` on http://127.0.0.1:8003
- `gateway-go` on http://127.0.0.1:8002 (public API)

Health check:

```bash
curl http://127.0.0.1:8002/health
```

Or use Docker from the repo root:

```bash
docker compose up --build
```

### 2. Frontend IDE

```powershell
cd rust
$env:MOONLIT_SKIP_LOGIN = "1"
cargo run -p moonlit-agent-ide
```

The IDE connects to `http://127.0.0.1:8002` by default.

### 3. LLM credentials (optional)

Without API keys, agentd falls back to the mock provider. To use a real model, set environment variables before starting agentd:

```powershell
$env:OPENAI_API_KEY = "your-key"
$env:OPENAI_BASE_URL = "https://api.deepseek.com/v1"   # example
```

### 4. MCP (optional)

Copy the example config and point `command` at your built `mcp-demo-server`:

```powershell
Copy-Item mcp.json.example mcp.json
```

## Project layout

```
agent-cowork/
├── rust/                 # GPUI IDE + DocForge + moonlit-* crates
├── backend-rs/           # agentd (Rust agent core)
├── gateway-go/           # Go edge gateway
├── backend/              # Python reference backend
├── docforge/             # Legacy TS monorepo (reference)
├── scripts/              # Dev scripts (start-stack.ps1, smoke tests)
├── skills/               # Agent skill definitions
└── docker-compose.yml
```

## Docs

- [backend-rs/README.md](backend-rs/README.md) — agentd architecture, profiles, memory API
- [gateway-go/README.md](gateway-go/README.md) — gateway env vars & auth
- [backend/README.md](backend/README.md) — Python reference backend
- [docforge/README.md](docforge/README.md) — document engine (legacy TS path)

## License

MIT — see [LICENSE](LICENSE).
