$ErrorActionPreference = "Stop"

# Resolve the repo root from this script's location (scripts/ subdir).
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$frontendDir = Join-Path $repoRoot "frontend"
$pythonExe = Join-Path $repoRoot ".venv\Scripts\python.exe"

# Env vars (ENTROPIAORME_BACKEND_PORT / ENTROPIAORME_FRONTEND_PORT /
# ENTROPIAORME_DATA_DIR / ENTROPIAORME_HOSTNAME) are sourced from
# .env.local by `just` itself via `set dotenv-load` in the justfile.
# This script inherits them from its parent (just -> powershell ->
# Start-Process wt.exe -> cmd /k child).

# Strangler topology (dev): the shell binds the public backend port and
# reverse-proxies not-yet-ported routes to the Python backend, which is
# relocated onto a private port. The private port defaults to public+1000,
# well clear of the ports neighbouring dev setups typically claim;
# override with ENTROPIAORME_SIDECAR_PORT in .env.local. The backend tab
# gets the private port as its bind port; the Tauri tab gets
# ENTROPIAORME_SIDECAR_PORT so the shell knows where to proxy. Vite keeps
# the public port, so the webview keeps dialling the address the shell
# now owns.
$publicPort = 8421
if ($env:ENTROPIAORME_BACKEND_PORT) { $publicPort = [int]$env:ENTROPIAORME_BACKEND_PORT }
$sidecarPort = $publicPort + 1000
if ($env:ENTROPIAORME_SIDECAR_PORT) { $sidecarPort = [int]$env:ENTROPIAORME_SIDECAR_PORT }

# Defensive CoreDNS start: the hostname-based devUrl resolves via OS DNS,
# which means CoreDNS must be running before the Tauri shell's tauri-cli
# resolves the devUrl during the tab launches below. The lifecycle
# script is idempotent (already-running is a no-op), so this is safe
# to invoke unconditionally on every dev launch. Skipped silently when
# coredns is not on PATH (the CoreDNS install is optional; the
# port-based devUrl fallback in build-dev-config.mjs keeps `just dev`
# working without it). The catch covers rare PowerShell-level invocation
# exceptions; non-zero exit from the lifecycle script surfaces its own
# message rather than throwing.
if (Get-Command coredns -ErrorAction SilentlyContinue) {
    try {
        & (Join-Path $PSScriptRoot "dns-lifecycle.ps1") -Action up
    } catch {
        Write-Warning "dns-lifecycle up threw a PowerShell exception (continuing without DNS layer): $($_.Exception.Message)"
    }
}

# Defensive Caddy reload: re-read the on-disk Caddyfile so any manual
# edits or newly-allocated per-checkout fragments propagate without a
# restart. Routes through caddy-lifecycle.ps1 so the main worktree's
# Caddyfile (rather than this launching checkout's local copy) is the
# reload target; that preserves multi-checkout coexistence, since the
# main worktree's `.dev/Caddyfile.worktrees/` is the canonical home for
# every active checkout's per-checkout routing fragment. No-op when
# Caddy is reachable on its admin endpoint and the config is unchanged;
# the lifecycle script surfaces caddy's own "admin endpoint
# unreachable" stderr if Caddy is not running, but does not block dev
# launch. Skipped silently when caddy is not on PATH (the Caddy install
# is optional; the port-based devUrl fallback in build-dev-config.mjs
# keeps `just dev` working without it).
if (Get-Command caddy -ErrorAction SilentlyContinue) {
    try {
        & (Join-Path $PSScriptRoot "caddy-lifecycle.ps1") -Action reload
    } catch {
        Write-Warning "caddy reload threw a PowerShell exception (continuing without reload): $($_.Exception.Message)"
    }
}

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
$sidecarDir = Join-Path $frontendDir "src-tauri\entropia-orme\binaries"
$sidecarStub = Join-Path $sidecarDir "entropiaorme-backend-x86_64-pc-windows-msvc.exe"
if (-not (Test-Path $sidecarStub)) {
    New-Item -ItemType Directory -Force -Path $sidecarDir | Out-Null
    New-Item -ItemType File -Path $sidecarStub | Out-Null
    Write-Host "Staged dev sidecar placeholder at $sidecarStub"
}

# The backend binds the private port (its own env view drives its bind
# address and Host-header guard); the shell learns the same port through
# ENTROPIAORME_SIDECAR_PORT and proxies to it. `set X=...&&` carries the
# per-tab value through cmd without a trailing space in the value.
$backendCmd = "set ENTROPIAORME_BACKEND_PORT=$sidecarPort&& `"$pythonExe`" -X utf8 -m backend.main"
# Use the tauri:dev npm script so the dev-mode CSP overlay (broader
# connect-src allowing alternate localhost backend ports) is applied
# automatically.
$tauriCmd = "set ENTROPIAORME_SIDECAR_PORT=$sidecarPort&& npm run tauri:dev"

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
