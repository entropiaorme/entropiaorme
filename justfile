# entropiaorme dev recipes. `just dev` launches the dev stack; `just
# check` runs the frontend type-check + build; `just test-rust` runs the
# Rust workspace's test suite. Run `just --list` to see every recipe.
#
# Env vars from .env.local (if present) are loaded automatically before
# each recipe via `set dotenv-load` below. Recognised keys:
# ENTROPIAORME_FRONTEND_PORT, ENTROPIAORME_DATA_DIR. Absence of the file
# falls through to runtime defaults. Parallel checkouts of this repo
# coexist by giving each its own values in its own .env.local.

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

# On macOS / Linux the same single Tauri dev process launches directly; env
# vars from .env.local are inherited the same way. Ctrl+C tears the stack down.
[unix]
dev:
    npm --prefix app run tauri:dev

# Run the Rust workspace's test suite: every member, the Tauri
# shell included (it needs the Tauri build chain's system libraries, as
# `just dev` does). Invoked from the workspace so
# app/src-tauri/.cargo/config.toml is discovered: it redirects test temp
# into target/, keeping an interrupted run from accumulating scratch dirs in
# the OS temp directory. Reclaim any leftovers from a prior interrupted run
# with `cargo clean`.
[windows]
test-rust:
    cd app/src-tauri; cargo nextest run --workspace

[unix]
test-rust:
    cd app/src-tauri && cargo nextest run --workspace

# Each step is its own recipe line (just stops on the first non-zero exit)
# rather than an `&&` chain, so the body runs under any shell, including
# Windows PowerShell, which does not support `&&`. `npm --prefix` runs each
# script from the frontend package without a shell-specific `cd`.
# Frontend type-check + production build (matches the CI `Frontend (build + check)` job).
check:
    npm --prefix app run check
    npm --prefix app run build

# Build the bespoke WiX Burn installer end to end (per-user MSI -> native x86
# bafunctions helper -> themed Burn bundle). Windows-only: needs WiX 6, the MSVC
# x86 toolset, and the Tauri build chain.
[windows]
installer:
    powershell -NoProfile -ExecutionPolicy RemoteSigned -File "{{justfile_directory()}}\scripts\build-installer.ps1"

[unix]
installer:
    @echo "just installer: the Windows installer build requires Windows (WiX + MSVC x86)."
    @exit 1

# Build the Linux packages (.deb + AppImage) end to end via the Tauri
# bundler, applying the Linux config overlay (deb/appimage targets, the
# Linux ONNX Runtime resource). Needs the Tauri build chain plus the
# WebKitGTK / GTK dev libraries.
[linux]
bundle:
    npm --prefix app run tauri -- build --config src-tauri/entropia-orme/tauri.linux.conf.json --bundles deb,appimage

[windows, macos]
bundle:
    @echo "just bundle: the Linux package build requires Linux (deb + AppImage)."
    @exit 1

# Headless smoke verification of the dev launch. Not yet implemented.
smoke:
    @echo "just smoke: headless smoke verification is not yet implemented."
    @exit 1

# Regenerate the TypeScript bindings for the typed IPC commands from the
# eo-api command manifest.
gen-ts:
    cd app/src-tauri && cargo run -q -p xtask -- gen-ts

# Fail when the committed typed-command bindings drift from the manifest
# (the CI drift gate, runnable locally).
gen-ts-check:
    cd app/src-tauri && cargo run -q -p xtask -- gen-ts --check
