$ErrorActionPreference = "Stop"

# Resolve the repo root from this script's location (scripts/ subdir).
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$frontendDir = Join-Path $repoRoot "frontend"
$pythonExe = Join-Path $repoRoot ".venv\Scripts\python.exe"

# Env vars (ENTROPIAORME_BACKEND_PORT / ENTROPIAORME_FRONTEND_PORT /
# ENTROPIAORME_DATA_DIR) are sourced from .env.local by `just` itself via
# `set dotenv-load` in the justfile. This script inherits them from its
# parent (just → powershell → Start-Process wt.exe → cmd /k child).

if (-not (Get-Command wt.exe -ErrorAction SilentlyContinue)) {
    Write-Error "Windows Terminal (wt.exe) is required for dev-launch.ps1."
}

if (-not (Test-Path $pythonExe)) {
    Write-Error "Missing backend interpreter at $pythonExe. Create the Windows virtualenv first."
}

if (-not (Test-Path $frontendDir)) {
    Write-Error "Missing frontend directory at $frontendDir."
}

# Dev affordance: tauri.conf.json declares the backend sidecar via
# `bundle.externalBin`, which causes Tauri's build-script-build to check
# the platform-triple-suffixed binary's existence at compile time even
# under `tauri dev`. The dev path runs the Python backend directly in a
# sibling terminal (via `python -m backend.main` below) and never invokes
# the sidecar at runtime, so a zero-byte placeholder satisfies the check.
# Release builds via `backend/build_app.py` replace this stub with the
# real PyInstaller-frozen exe at the same path. The guard is strictly
# "create if missing" so a frozen sidecar from a prior release build
# survives launch re-invocations untouched.
$sidecarDir = Join-Path $frontendDir "src-tauri\binaries"
$sidecarStub = Join-Path $sidecarDir "entropiaorme-backend-x86_64-pc-windows-msvc.exe"
if (-not (Test-Path $sidecarStub)) {
    New-Item -ItemType Directory -Force -Path $sidecarDir | Out-Null
    New-Item -ItemType File -Path $sidecarStub | Out-Null
    Write-Host "Staged dev sidecar placeholder at $sidecarStub"
}

$backendCmd = "`"$pythonExe`" -X utf8 -m backend.main"
# Use the tauri:dev npm script so the dev-mode CSP overlay (broader
# connect-src allowing alternate localhost backend ports) is applied
# automatically.
$tauriCmd = "npm run tauri:dev"

Start-Process wt.exe -ArgumentList @(
    "-w", "0",
    "new-tab",
    "--title", "Backend",
    "-d", $repoRoot,
    "cmd", "/k", $backendCmd
)

Start-Sleep -Milliseconds 700

Start-Process wt.exe -ArgumentList @(
    "-w", "0",
    "new-tab",
    "--title", "Tauri",
    "-d", $frontendDir,
    "cmd", "/k", $tauriCmd
)
