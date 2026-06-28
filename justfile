# entropiaorme dev recipes. `just dev` launches the dev stack; `just
# check` runs the frontend type-check + build; `just test-rust` runs the
# native backend test suite. Run `just --list` to see every recipe.
#
# Env vars from .env.local (if present) are loaded automatically before
# each recipe via `set dotenv-load` below. Recognised keys:
# ENTROPIAORME_FRONTEND_PORT, ENTROPIAORME_DATA_DIR,
# ENTROPIAORME_HOSTNAME. Absence of the file falls through to runtime
# defaults; absence of ENTROPIAORME_HOSTNAME specifically falls through to
# the port-based devUrl in build-dev-config.mjs (i.e. Caddy and CoreDNS are
# both optional).

set dotenv-load
set dotenv-filename := ".env.local"

# just defaults the recipe-body shell to `sh` on every platform, including
# Windows, where a stock machine has no sh.exe on PATH. Route recipe bodies
# through PowerShell on Windows so the recipes run without Git Bash or WSL
# installed. RemoteSigned matches the execution policy the Windows recipes
# already pass to `powershell -File`.
set windows-shell := ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "RemoteSigned", "-Command"]

# Default: list available recipes.
default:
    @just --list

# Boot the dev stack: the single Tauri dev process (backend in-process) which
# spawns and tails Vite for the frontend.
[windows]
dev:
    powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\dev-launch.ps1"

[unix]
dev:
    @echo "just dev: macOS / Linux dev launch is not yet implemented; contributions welcome."
    @exit 1

# Run the native backend (Rust) test suite. Invoked from the workspace so
# frontend/src-tauri/.cargo/config.toml is discovered: it redirects test temp
# into target/, keeping an interrupted run from accumulating scratch dirs in
# the OS temp directory. Reclaim any leftovers from a prior interrupted run
# with `cargo clean`.
[windows]
test-rust:
    cd frontend/src-tauri; cargo nextest run -p eo-wire -p eo-http -p eo-services

[unix]
test-rust:
    cd frontend/src-tauri && cargo nextest run -p eo-wire -p eo-http -p eo-services

# Each step is its own recipe line (just stops on the first non-zero exit)
# rather than an `&&` chain, so the body runs under any shell, including
# Windows PowerShell, which does not support `&&`. `npm --prefix` runs each
# script from the frontend package without a shell-specific `cd`.
# Frontend type-check + production build (matches the CI `Frontend (build + check)` job).
check:
    npm --prefix frontend run check
    npm --prefix frontend run build

# Build the bespoke WiX Burn installer end to end (per-user MSI -> native x86
# bafunctions helper -> themed Burn bundle). Windows-only: needs WiX 6, the MSVC
# x86 toolset, and the Tauri build chain. Mirrors the release pipeline's build.
[windows]
installer:
    powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\build-installer.ps1"

[unix]
installer:
    @echo "just installer: the Windows installer build requires Windows (WiX + MSVC x86)."
    @exit 1

# Headless smoke verification of the dev launch. Not yet implemented.
smoke:
    @echo "just smoke: headless smoke verification is not yet implemented."
    @exit 1

# Regenerate the typed frontend API client from the committed OpenAPI
# snapshot (frontend/src-tauri/contracts/openapi.snapshot.json). Run after a
# change that regenerates the snapshot.
gen-api:
    npm --prefix frontend run gen:api

# Verify the committed generated client matches the OpenAPI snapshot
# (matches the CI freshness step).
gen-api-check:
    npm --prefix frontend run gen:api:check

# Start Caddy in the background using the main worktree's Caddyfile.
# Routes through caddy-lifecycle.ps1 so the main worktree (resolved via
# `git worktree list`) is the canonical config home, regardless of
# which checkout this recipe is invoked from. Caddy runs as a
# long-lived process; subsequent invocations are no-ops once it is
# running. See `Caddyfile` for the routed hostname.
[windows]
proxy-up:
    @powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\caddy-lifecycle.ps1" -Action up

[unix]
proxy-up:
    @echo "just proxy-up: macOS / Linux Caddy launch is not yet implemented; contributions welcome."
    @exit 1

# Stop the background Caddy via its admin endpoint (localhost:2019).
[windows]
proxy-down:
    @powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\caddy-lifecycle.ps1" -Action down

[unix]
proxy-down:
    @echo "just proxy-down: macOS / Linux Caddy launch is not yet implemented; contributions welcome."
    @exit 1

# Cheap liveness check via Caddy's admin endpoint. Prints `caddy running`
# or `caddy not running`.
[windows]
proxy-status:
    @powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\caddy-lifecycle.ps1" -Action status

[unix]
proxy-status:
    @echo "just proxy-status: macOS / Linux Caddy launch is not yet implemented; contributions welcome."
    @exit 1

# Start CoreDNS in the background using the Corefile at the repo root.
# Binds 127.0.0.1:53 for *.localhost resolution. Idempotent: a second
# invocation reports `coredns already running` rather than spawning a
# duplicate. See `Corefile` for the resolved zones.
[windows]
dns-up:
    @powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\dns-lifecycle.ps1" -Action up

[unix]
dns-up:
    @echo "just dns-up: macOS / Linux DNS launch is not yet implemented; contributions welcome."
    @exit 1

# Stop the background CoreDNS by process name.
[windows]
dns-down:
    @powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\dns-lifecycle.ps1" -Action down

[unix]
dns-down:
    @echo "just dns-down: macOS / Linux DNS launch is not yet implemented; contributions welcome."
    @exit 1

# Cheap liveness check by process presence. Prints `coredns running`
# or `coredns not running`.
[windows]
dns-status:
    @powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\dns-lifecycle.ps1" -Action status

[unix]
dns-status:
    @echo "just dns-status: macOS / Linux DNS launch is not yet implemented; contributions welcome."
    @exit 1
