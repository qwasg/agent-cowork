# 启动默认后端栈：Rust agentd (:8003) + Go gateway (:8002)。
# 用法：powershell -File scripts/start-stack.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

Write-Host "[1/3] 构建 agentd（含 mcp-demo-server）..."
Push-Location (Join-Path $root "backend-rs")
cargo build --release --bins
Pop-Location

Write-Host "[2/3] 构建 gateway-go..."
Push-Location (Join-Path $root "gateway-go")
go build -o agent-debug-gateway.exe .
Pop-Location

Write-Host "[3/3] 启动进程（后台，不弹窗）..."
# -WindowStyle Hidden：不弹出独立控制台窗口
$env:AGENT_DEBUG_HOST = "127.0.0.1"
$env:AGENT_CORE_PORT  = "8003"
$agentd = Start-Process -PassThru -WindowStyle Hidden -WorkingDirectory $root `
    -FilePath (Join-Path $root "backend-rs\target\release\agentd.exe")
$env:AGENT_DEBUG_HTTP_PORT = "8002"
$env:AGENT_CORE_URL        = "http://127.0.0.1:8003"
$gateway = Start-Process -PassThru -WindowStyle Hidden -WorkingDirectory $root `
    -FilePath (Join-Path $root "gateway-go\agent-debug-gateway.exe")

Write-Host "agentd  PID=$($agentd.Id)  -> http://127.0.0.1:8003"
Write-Host "gateway PID=$($gateway.Id) -> http://127.0.0.1:8002 (公开入口)"
Write-Host "健康检查: curl http://127.0.0.1:8002/health"
