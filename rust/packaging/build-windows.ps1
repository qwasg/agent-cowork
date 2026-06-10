<#
.SYNOPSIS
    Build the Moonlit native apps in release mode and assemble a portable
    Windows distribution (zip). No external tooling required.

.DESCRIPTION
    Compiles `moonlit-docforge` and `moonlit-agent-ide` (both with the GPUI
    backend), copies the executables into dist/moonlit-<version>/, and zips it.
    If WiX (candle/light) is on PATH it additionally builds an MSI.

.EXAMPLE
    pwsh ./packaging/build-windows.ps1
#>
param(
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $version = (Select-String -Path "Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
    if (-not $version) { $version = "0.1.0" }
    Write-Host "Building Moonlit $version ($Profile)…" -ForegroundColor Cyan

    cargo build --profile $Profile -p moonlit-docforge -p moonlit-agent-ide
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $targetDir = if ($Profile -eq "release") { "target/release" } else { "target/$Profile" }
    $stage = Join-Path $root "dist/moonlit-$version"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    foreach ($exe in @("moonlit-docforge.exe", "moonlit-agent-ide.exe")) {
        $src = Join-Path $targetDir $exe
        if (-not (Test-Path $src)) { throw "missing build artifact: $src" }
        Copy-Item $src $stage
    }

    $zip = Join-Path $root "dist/moonlit-$version-windows-x64.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path "$stage/*" -DestinationPath $zip
    Write-Host "Portable zip: $zip" -ForegroundColor Green

    $candle = Get-Command candle.exe -ErrorAction SilentlyContinue
    $light = Get-Command light.exe -ErrorAction SilentlyContinue
    if ($candle -and $light) {
        Write-Host "WiX detected — building MSI…" -ForegroundColor Cyan
        & candle.exe -nologo -arch x64 -out "$stage/installer.wixobj" (Join-Path $PSScriptRoot "moonlit.wxs") -dVersion="$version" -dStageDir="$stage"
        & light.exe -nologo -ext WixUIExtension -out (Join-Path $root "dist/moonlit-$version-windows-x64.msi") "$stage/installer.wixobj"
        Write-Host "MSI built." -ForegroundColor Green
    } else {
        Write-Host "WiX (candle/light) not found — skipping MSI. Portable zip is ready." -ForegroundColor Yellow
    }
}
finally {
    Pop-Location
}
