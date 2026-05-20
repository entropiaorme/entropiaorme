$ErrorActionPreference = "Stop"

# Backward-compatibility shim. The canonical dev launch is `just dev`;
# this file exists so existing `./launch.ps1` invocations keep working.
# Both routes produce identical behaviour.

Set-Location $PSScriptRoot

if (-not (Get-Command just -ErrorAction SilentlyContinue)) {
    Write-Error "just is not on PATH. Install it (Windows: ``scoop install just``) or invoke the dev launch directly via .\scripts\dev-launch.ps1."
}

& just dev
