# entropiaorme dev recipes. `just dev` launches the dev stack; `just
# check` runs the frontend type-check + build; `just test-backend` runs
# the pytest suite. Run `just --list` to see every recipe.
#
# Env vars from .env.local (if present) are loaded automatically before
# each recipe via `set dotenv-load` below. Recognised keys:
# ENTROPIAORME_BACKEND_PORT, ENTROPIAORME_FRONTEND_PORT,
# ENTROPIAORME_DATA_DIR. Absence of the file falls through to runtime
# defaults.

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
