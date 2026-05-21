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

# Frontend type-check + production build (matches the CI `Frontend (build + check)` job).
check:
    cd frontend && npm run check && npm run build

# Headless smoke verification of the dev launch. Not yet implemented.
smoke:
    @echo "just smoke: headless smoke verification is not yet implemented."
    @exit 1

# Start Caddy in the background using the Caddyfile at the repo root.
# Caddy runs as a long-lived process; subsequent invocations are no-ops
# once it is running. See `Caddyfile` for the routed hostname.
proxy-up:
    caddy start --config Caddyfile

# Stop the background Caddy via its admin endpoint (localhost:2019).
proxy-down:
    caddy stop

# Cheap liveness check via Caddy's admin endpoint. Prints `caddy running`
# or `caddy not running`.
proxy-status:
    @curl -fsS -o /dev/null http://localhost:2019/config/ && echo "caddy running" || echo "caddy not running"

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
