# entropiaorme dev recipes.
#
# `just dev` is the canonical dev-launch entry point. `launch.ps1` and
# `launch.bat` at the repo root remain as thin backward-compatibility shims
# that invoke `just dev`; both routes produce identical behaviour. See
# .planning/lanes/dev-orchestration.md for the lane context (R5 retires the
# shims once direnv adoption is the canonical activation surface and
# cross-platform parity is verified at R4).
#
# Per-worktree env vars in `.env.local` (ENTROPIAORME_BACKEND_PORT /
# ENTROPIAORME_FRONTEND_PORT / ENTROPIAORME_DATA_DIR) are loaded by just
# itself via `set dotenv-load` below, so every recipe honours them without
# external sourcing. The tier dir's daily-driver invocation (no .env.local
# present by convention for public) is a no-op load; runtime defaults bind
# on the documented anchor ports per CLAUDE.md "Lane environment".

set dotenv-load
set dotenv-filename := ".env.local"

# Default recipe: list available recipes.
default:
    @just --list

# Boot the full dev stack: backend (Python) + Tauri webview (which spawns Vite).
[windows]
dev:
    powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\dev-launch.ps1"

[unix]
dev:
    @echo "just dev: macOS / Linux cross-platform path lands under dev-orchestration R4 — see .planning/lanes/dev-orchestration.md"
    @exit 1

# Run backend tests.
test-backend:
    .venv/Scripts/python.exe -m pytest backend/tests/

# Frontend type-check + production build (CI parity with the `Frontend (build + check)` job).
check:
    cd frontend && npm run check && npm run build

# Headless smoke verification of the dev launch.
# R4 territory; placeholder recipe shape for the cross-platform smoke beat.
smoke:
    @echo "just smoke: headless smoke verification lands under dev-orchestration R4"
    @exit 1
