![EntropiaOrme](assets/entropiaorme-github-banner.png)

[![CI](https://github.com/entropiaorme/entropiaorme/actions/workflows/ci.yml/badge.svg)](https://github.com/entropiaorme/entropiaorme/actions/workflows/ci.yml)
[![Branch coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/entropiaorme/entropiaorme/badges/coverage.json)](https://github.com/entropiaorme/entropiaorme/actions/workflows/ci.yml)

An analytical desktop tool for Entropia Universe: a personal project, built for its maintainer's own play and published here as source for anyone who finds it useful or interesting.

A Tauri 2 shell hosting a Svelte 5 frontend over a pure-Rust in-process backend. It runs on Windows and, experimentally, on Linux (Ubuntu on Wayland/GNOME, via XWayland); the OS-touching seams (input observation, screen capture, window discovery) are implemented per platform behind shared traits, and the Linux build is still being validated against the live game. There are no installer downloads: the application is built from source, as described below.

## Repository map

- `app/`: the application (SvelteKit frontend + the `src-tauri` Rust workspace)
- `docs/`: the mdBook architecture handbook and ADRs
- `scripts/`: build and launch scripts driven by the `justfile`
- `assets/`: repository art

## How this repository is updated

Day-to-day development happens in a private working tree. This repository receives an occasional source snapshot of it, as a single commit on `main`, once the changes have been in use for a while ([ADR-0040](docs/src/adr/0040-periodic-source-snapshots.md)). The history up to that decision is the project's full development history; from then on, each snapshot is one commit.

## Build (Windows)

Prerequisites: Node.js ≥ 20.19, Rust (`rustup`), Visual Studio Build Tools (MSVC C++ workload), Windows Terminal, [`just`](https://just.systems/) ≥ 1.34.

```bash
cd app && npm install && cd ..
pre-commit install   # local hooks mirroring the CI gates (see TESTING.md)
just dev             # hot-reload dev shell
just installer       # WiX Burn installer -> app/src-tauri/target/release/bundle/burn/
```

`just --list` shows the remaining recipes (`just check`, `just test-rust`, `just gen-ts`). The installer build additionally needs WiX 6 (the `wix` dotnet tool) and the MSVC x86 toolset; installer sources live under `app/src-tauri/entropia-orme/installer/`.

## Parallel checkouts

Give each checkout its own `.env.local` with a distinct `ENTROPIAORME_FRONTEND_PORT` and `ENTROPIAORME_DATA_DIR`; `just` loads it before every recipe. Optionally, [`direnv`](https://direnv.net/) activates it for ad-hoc shells (`direnv allow .` once per checkout).

## Documentation

- [Architecture handbook](docs/) and [ADRs](docs/src/adr/): build with `mdbook build docs`; `cargo doc` generates the API reference.
- [TESTING.md](TESTING.md): the test suite, the replay corpus, and the CI gates.
- [SECURITY.md](SECURITY.md): the security policy and the supply-chain review gates.

## License

[MIT](LICENSE). Third-party components and their licenses: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
