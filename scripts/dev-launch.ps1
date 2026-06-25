$ErrorActionPreference = "Stop"

# Resolve the repo root from this script's location (scripts/ subdir).
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$frontendDir = Join-Path $repoRoot "frontend"

# The app is a single binary: the backend runs in-process inside the Tauri
# shell in every mode, including `tauri dev`, and the webview reaches it over
# Tauri IPC (no localhost backend port, no proxy, no sidecar). So dev launch is
# just the one Tauri dev process, which spawns and tails Vite for the frontend.
#
# Env vars (ENTROPIAORME_FRONTEND_PORT / ENTROPIAORME_DATA_DIR /
# ENTROPIAORME_HOSTNAME) are sourced from .env.local by `just` itself via
# `set dotenv-load` in the justfile, and inherited here. FRONTEND_PORT drives
# Vite's port and the fallback dev URL; DATA_DIR points the in-process backend
# at a per-checkout data directory; HOSTNAME (optional) selects the HTTPS dev
# URL served through Caddy.

# Defensive CoreDNS start: the hostname-based devUrl resolves via OS DNS,
# which means CoreDNS must be running before the Tauri shell's tauri-cli
# resolves the devUrl during the dev launch below. Gated on
# ENTROPIAORME_HOSTNAME being set: a contributor on the plain-port flow
# does not need the DNS layer, so we do not start it for them (matching
# build-dev-config.mjs, which only does hostname work when the hostname is
# set). The lifecycle script is idempotent (already-running is a no-op),
# so this is safe to invoke on every hostname-mode dev launch. Skipped
# silently when coredns is not on PATH (the CoreDNS install is optional;
# the port-based devUrl fallback in build-dev-config.mjs keeps `just dev`
# working without it). The catch covers rare PowerShell-level invocation
# exceptions; non-zero exit from the lifecycle script surfaces its own
# message rather than throwing.
if ($env:ENTROPIAORME_HOSTNAME -and (Get-Command coredns -ErrorAction SilentlyContinue)) {
    try {
        & (Join-Path $PSScriptRoot "dns-lifecycle.ps1") -Action up
    } catch {
        Write-Warning "dns-lifecycle up threw a PowerShell exception (continuing without DNS layer): $($_.Exception.Message)"
    }
}

# Defensive Caddy ensure-up: guarantee a running, current-config Caddy
# before the Tauri tab resolves the HTTPS devUrl. Uses the idempotent
# `up` action (start if down, reload if already up) rather than a bare
# reload, which would silently no-op against a dead admin endpoint and
# leave the hostname unrouted. Routes through caddy-lifecycle.ps1 so the
# main worktree's Caddyfile (rather than this launching checkout's local
# copy) is the target; that preserves multi-checkout coexistence, since
# the main worktree's `.dev/Caddyfile.worktrees/` is the canonical home
# for every active checkout's per-checkout routing fragment. Gated on
# ENTROPIAORME_HOSTNAME being set so a contributor on the plain-port flow
# does not get Caddy force-started. Skipped silently when caddy is not on
# PATH (the Caddy install is optional; the port-based devUrl fallback in
# build-dev-config.mjs keeps `just dev` working without it).
if ($env:ENTROPIAORME_HOSTNAME -and (Get-Command caddy -ErrorAction SilentlyContinue)) {
    try {
        & (Join-Path $PSScriptRoot "caddy-lifecycle.ps1") -Action up
    } catch {
        Write-Warning "caddy ensure-up threw a PowerShell exception (continuing without proxy): $($_.Exception.Message)"
    }
}

if (-not (Test-Path $frontendDir)) {
    Write-Error "Missing frontend directory at $frontendDir."
}

# Launch the single dev process in the foreground. `tauri:dev` runs
# build-dev-config.mjs (which writes the env-driven devUrl overlay) and then
# `tauri dev`, which builds and runs the shell with the backend in-process and
# spawns Vite as a child, tailing both. Ctrl+C tears the whole stack down.
npm --prefix $frontendDir run tauri:dev
