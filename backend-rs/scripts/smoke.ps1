# End-to-end smoke test: builds agentd (Rust core) + gateway-go (edge), boots
# both against a throwaway data dir, then walks the critical user journey
# through the *gateway* port:
#   health -> register/login -> create session -> ask:execute -> SSE stream ->
#   steer (error envelope) -> cancel -> fork -> replay -> openapi ->
#   document session (agentKind) -> memory create/list/delete
# Exits non-zero on the first failed assertion. Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\smoke.ps1
$ErrorActionPreference = "Stop"

$backendRs = Split-Path -Parent $PSScriptRoot
$repoRoot = Split-Path -Parent $backendRs
$gatewayDir = Join-Path $repoRoot "gateway-go"

$gatewayPort = 18002
$corePort = 18003
$base = "http://127.0.0.1:$gatewayPort"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("agentd-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

$failures = 0
function Assert-True([bool]$cond, [string]$what) {
    if ($cond) {
        Write-Host "  PASS  $what" -ForegroundColor Green
    } else {
        Write-Host "  FAIL  $what" -ForegroundColor Red
        $script:failures++
    }
}

Write-Host "== building agentd + gateway =="
Push-Location $backendRs
# cmd /c keeps cargo/go stderr progress output from tripping ErrorActionPreference=Stop.
cmd /c "cargo build -p agentd --bin agentd 2>&1" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
Pop-Location
Push-Location $gatewayDir
cmd /c "go build -o `"$(Join-Path $tmp 'gateway.exe')`" . 2>&1" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "go build failed" }
Pop-Location

Write-Host "== starting services (gateway :$gatewayPort -> core :$corePort, data: $tmp) =="
$env:AGENT_DEBUG_HOST = "127.0.0.1"
$env:AGENT_DEBUG_HTTP_PORT = "$gatewayPort"
$env:AGENT_DEBUG_DATA_DIR = "$tmp"
$env:AGENT_DEBUG_SESSION_DIR = (Join-Path $tmp "sessions")
$env:AGENT_DEBUG_WORKSPACE_ROOT = "$tmp"
$env:AGENT_DEBUG_REQUIRE_AUTH = "1"
$env:AGENT_CORE_URL = "http://127.0.0.1:$corePort"

$coreExe = Join-Path $backendRs "target\debug\agentd.exe"
$core = Start-Process -FilePath $coreExe -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $tmp "core.log") -RedirectStandardError (Join-Path $tmp "core.err.log")

# Wait for the core, then start the gateway (it must read the JWT secret file
# the core writes at startup).
$coreUp = $false
foreach ($i in 1..50) {
    Start-Sleep -Milliseconds 300
    try {
        $r = Invoke-RestMethod -Uri "http://127.0.0.1:$corePort/health" -TimeoutSec 2
        if ($r.ok) { $coreUp = $true; break }
    } catch {}
}
if (-not $coreUp) { Stop-Process -Id $core.Id -Force; throw "agentd core never became healthy" }

$gw = Start-Process -FilePath (Join-Path $tmp "gateway.exe") -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $tmp "gw.log") -RedirectStandardError (Join-Path $tmp "gw.err.log")
$gwUp = $false
foreach ($i in 1..50) {
    Start-Sleep -Milliseconds 200
    try {
        $r = Invoke-RestMethod -Uri "$base/health" -TimeoutSec 2
        if ($r.ok) { $gwUp = $true; break }
    } catch {}
}

try {
    Assert-True $gwUp "gateway + core healthy through edge /health"

    # Auth required for everything beyond the open paths.
    $denied = $false
    try { Invoke-RestMethod -Uri "$base/api/agent-debug/sessions" -TimeoutSec 5 | Out-Null }
    catch { $denied = $_.Exception.Response.StatusCode.value__ -eq 401 }
    Assert-True $denied "unauthenticated request rejected with 401"

    # Register + login -> JWT accepted by the Go gateway.
    $cred = @{ email = "smoke@test.dev"; password = "smoke-pass-123"; displayName = "Smoke"; workspace = "$tmp" } | ConvertTo-Json
    Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/auth/register" -Body $cred -ContentType "application/json" | Out-Null
    $login = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/auth/login" `
        -Body (@{ email = "smoke@test.dev"; password = "smoke-pass-123" } | ConvertTo-Json) -ContentType "application/json"
    $token = $login.token
    Assert-True ($null -ne $token -and $token.Length -gt 20) "login returns a JWT"
    $H = @{ Authorization = "Bearer $token" }

    # Session + chat turn (mock provider replies offline).
    $s = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/sessions" -Headers $H `
        -Body (@{ title = "smoke" } | ConvertTo-Json) -ContentType "application/json"
    $sid = $s.session.id
    Assert-True ($sid -like "sess*" -or $sid.Length -gt 8) "session created ($sid)"

    $ask = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/sessions/$sid/ask:execute" -Headers $H `
        -Body (@{ userInput = "冒烟测试：你好"; composerMode = "build" } | ConvertTo-Json) -ContentType "application/json" -TimeoutSec 120
    Assert-True ($ask.run.status -eq "completed") "ask:execute completes a run"
    Assert-True ($ask.message.text.Length -gt 0) "ask:execute returns assistant text"

    # SSE stream through the gateway delivers the replayed events.
    $sse = & curl.exe -s -N -m 4 -H "Authorization: Bearer $token" "$base/api/agent-debug/sessions/$sid/events/stream?fromSeq=0" 2>$null | Out-String
    Assert-True ($sse -match "composer.user.message") "SSE stream replays composer.user.message"
    Assert-True ($sse -match "agent.completed") "SSE stream replays agent.completed"

    # Steering a finished run yields the structured error envelope.
    $rid = $ask.run.id
    $steerCode = ""
    try {
        Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/runs/${rid}:steer" -Headers $H `
            -Body (@{ text = "too late" } | ConvertTo-Json) -ContentType "application/json" | Out-Null
    } catch {
        $steerCode = ($_.ErrorDetails.Message | ConvertFrom-Json).error.code
    }
    Assert-True ($steerCode -eq "RUN_NOT_ACTIVE") "steer on finished run -> RUN_NOT_ACTIVE envelope"

    # Cancel is idempotent on finished runs (200 + ok flag).
    $cancel = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/runs/${rid}:cancel" -Headers $H `
        -Body "{}" -ContentType "application/json"
    Assert-True ($null -ne $cancel.ok) "cancel endpoint responds with ok flag"

    # Fork copies history; replay returns the event log for both sessions.
    $fork = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/sessions/${sid}:fork" -Headers $H `
        -Body "{}" -ContentType "application/json"
    $fid = $fork.session.id
    Assert-True ($fid.Length -gt 8 -and $fid -ne $sid) "fork creates a new session ($fid)"
    $replay = Invoke-RestMethod -Uri "$base/api/agent-debug/replay/$fid" -Headers $H
    $completedEvents = @($replay.events | Where-Object { $_.type -eq "agent.completed" })
    Assert-True ($completedEvents.Count -ge 1) "forked session replay contains agent.completed"

    # OpenAPI document is served and lists the chat route.
    $doc = Invoke-RestMethod -Uri "$base/api/agent-debug/openapi.json" -Headers $H
    Assert-True ($null -ne $doc.paths."/api/agent-debug/sessions/{id}/ask:execute") "openapi.json describes ask:execute"

    # Run metrics carry the phase-9 fields.
    $metrics = Invoke-RestMethod -Uri "$base/api/agent-debug/runs/$rid/metrics" -Headers $H
    Assert-True ($null -ne $metrics.stepsTotal -and $null -ne $metrics.usage) "run metrics expose steps + usage"

    # Agent profiles: a document session reports agentKind=document.
    $docSess = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/sessions" -Headers $H `
        -Body (@{ title = "smoke-doc"; agentKind = "document" } | ConvertTo-Json) -ContentType "application/json"
    Assert-True ($docSess.session.agentKind -eq "document") "document session created with agentKind=document"

    # Long-term memory: create -> list -> delete roundtrip.
    $mem = Invoke-RestMethod -Method Post -Uri "$base/api/agent-debug/memories" -Headers $H `
        -Body (@{ content = "冒烟：用户偏好简体中文"; kind = "preference"; scope = "global" } | ConvertTo-Json) -ContentType "application/json"
    $memId = $mem.memory.id
    Assert-True ($memId.Length -gt 8) "memory created ($memId)"
    $memList = Invoke-RestMethod -Uri "$base/api/agent-debug/memories" -Headers $H
    $found = @($memList.memories | Where-Object { $_.id -eq $memId })
    Assert-True ($found.Count -eq 1) "memory appears in list"
    $delMem = Invoke-RestMethod -Method Delete -Uri "$base/api/agent-debug/memories/$memId" -Headers $H
    Assert-True ($null -ne $delMem.ok) "memory delete responds with ok flag"
} finally {
    Stop-Process -Id $gw.Id -Force -ErrorAction SilentlyContinue
    Stop-Process -Id $core.Id -Force -ErrorAction SilentlyContinue
}

Write-Host ""
if ($failures -eq 0) {
    Write-Host "SMOKE OK - all checks passed (data: $tmp)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "SMOKE FAILED - $failures check(s) failed (logs in $tmp)" -ForegroundColor Red
    exit 1
}
