# entropiaorme dev recipes. `just dev` launches the dev stack; `just
# check` runs the frontend type-check + build; `just test-backend` runs
# the pytest suite. Run `just --list` to see every recipe.
#
# Env vars from .env.local (if present) are loaded automatically before
# each recipe via `set dotenv-load` below. Recognised keys:
# ENTROPIAORME_BACKEND_PORT, ENTROPIAORME_FRONTEND_PORT,
# ENTROPIAORME_DATA_DIR, ENTROPIAORME_HOSTNAME. Absence of the file
# falls through to runtime defaults; absence of ENTROPIAORME_HOSTNAME
# specifically falls through to the port-based devUrl in
# build-dev-config.mjs (i.e. Caddy and CoreDNS are both optional).

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

# Boot the full dev stack: backend (Python) + Tauri webview (which spawns Vite).
[windows]
dev:
    powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\dev-launch.ps1"

[unix]
dev:
    @echo "just dev: macOS / Linux dev launch is not yet implemented; contributions welcome."
    @exit 1

# Run backend tests.
test-backend:
    .venv/Scripts/python.exe -m pytest backend/tests/

# Run the cross-language equivalence runner end to end: the Rust-native
# per-unit gate (Normalizer conformance, DB-snapshot + HTTP fingerprint emitter
# byte-equality, the .yml-family mirrors, the cost-engine numeric loop) PLUS the
# live differential fuzzes against the Python oracle (the `cross-language`
# feature) and the Python faithfulness legs. The virtualenv must be installed
# (the differentials shell out to it). No ignore list: every divergence fails.
test-equivalence:
    $env:EO_ORACLE_PYTHON = (Resolve-Path .venv/Scripts/python.exe).Path; cargo test --manifest-path frontend/src-tauri/Cargo.toml -p eo-wire -p eo-services --features cross-language
    .venv/Scripts/python.exe -m pytest backend/tests/test_normalizer_conformance.py backend/tests/test_equivalence_emitters.py backend/tests/test_equivalence_yml_family.py

# Each step is its own recipe line (just stops on the first non-zero exit)
# rather than an `&&` chain, so the body runs under any shell, including
# Windows PowerShell, which does not support `&&`. `npm --prefix` runs each
# script from the frontend package without a shell-specific `cd`.
# Frontend type-check + production build (matches the CI `Frontend (build + check)` job).
check:
    npm --prefix frontend run check
    npm --prefix frontend run build

# Headless smoke verification of the dev launch. Not yet implemented.
smoke:
    @echo "just smoke: headless smoke verification is not yet implemented."
    @exit 1

# Regenerate the typed frontend API client from the committed OpenAPI
# snapshot (backend/tests/expected/openapi.snapshot.json). Run after any
# backend change that regenerates the snapshot.
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
