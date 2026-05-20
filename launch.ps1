$ErrorActionPreference = "Stop"

# Backward-compatibility shim. Daily-driver dev launch is `just dev`; this
# file exists so existing muscle memory (`./launch.ps1`, `launch.bat` →
# `launch.ps1`) keeps working through the dev-orchestration lane's
# transition. Both routes produce identical behaviour. Scheduled for
# retirement at dev-orchestration R5 once direnv adoption is the canonical
# activation surface and cross-platform parity is verified at R4.

Set-Location $PSScriptRoot

if (-not (Get-Command just -ErrorAction SilentlyContinue)) {
    Write-Error "just is not on PATH. Install it (Windows: ``scoop install just``) or invoke the dev launch directly via .\scripts\dev-launch.ps1."
}

& just dev
