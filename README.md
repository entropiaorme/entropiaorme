![EntropiaOrme](assets/entropiaorme-github-banner.png)

[![CI](https://github.com/MikelWL/entropiaorme/actions/workflows/ci.yml/badge.svg)](https://github.com/MikelWL/entropiaorme/actions/workflows/ci.yml)

An analytical desktop tool for Entropia Universe.

For an overview, installer downloads, and usage guides → **[entropiaorme.com](https://entropiaorme.com)**.

The rest of this README is for developers building from source. Windows-only for now; other platforms may follow.

---

## Build from source (Windows)

### Prerequisites

- Python 3.11+
- Node.js ≥ 20.19
- Rust (`rustup`): for the Tauri shell
- Visual Studio Build Tools (MSVC C++ workload): required by Tauri on Windows
- Windows Terminal (`wt.exe`): used by the launcher
- [`just`](https://just.systems/) ≥ 1.34: task runner driving `just dev` etc. (Windows: `scoop install just`).
- [`direnv`](https://direnv.net/): activates env vars from `.env.local` on `cd`-in so ad-hoc shell commands (`python -m backend.main`, `pytest`, `npm run ...`) honour the local env. (Windows: `scoop install direnv`; run `direnv allow .` once per checkout to whitelist the `.envrc`.)
- [`caddy`](https://caddyserver.com/) (optional): reverse proxy that fronts the dev stack on a stable `http://entropiaorme.localhost` hostname so URLs in the browser, DevTools, and screenshots read as a hostname rather than a port number. Start via `just proxy-up`; skip the install to keep the existing port-based `just dev` flow. (Windows: `winget install CaddyServer.Caddy`.)

### Setup

From the repo root:

```bash
python -m venv .venv
.venv\Scripts\activate
pip install -r backend/requirements.txt
pip install -r backend/requirements-dev.txt   # pytest (`just test-backend`) and pyinstaller (`backend/build_app.py`)

cd frontend
npm install
cd ..
```

### Run

```bash
just dev
```

Opens two Windows Terminal tabs: FastAPI backend + Tauri dev shell.

Run `just --list` to see other recipes (`just check` for frontend type-check + build, `just test-backend` for the pytest suite).

### Build installer

```bash
.venv\Scripts\python.exe backend\build_app.py
```

Produces (at `frontend/src-tauri/target/release/`):

- `bundle/nsis/entropiaorme-<version>-x64-setup.exe`: NSIS installer (sidecar bundled inside).
- `entropiaorme-<version>-x64-portable.zip`: portable bundle (Tauri shell + sidecar + README.txt).
- Matching `.sha256` sidecar files for both artefacts (single-line `<hash>  <filename>`, `sha256sum -c` compatible).

Installer chrome assets (header / sidebar BMPs + plain-text MIT license) live under `frontend/src-tauri/installer/` and are wired through `bundle.windows.nsis` in `frontend/src-tauri/tauri.conf.json`.

## License

[MIT](LICENSE). Third-party components and their licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
