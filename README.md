![EntropiaOrme](assets/entropiaorme-github-banner.png)

[![CI](https://github.com/entropiaorme/entropiaorme/actions/workflows/ci.yml/badge.svg)](https://github.com/entropiaorme/entropiaorme/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/entropiaorme/entropiaorme/badges/coverage.json)](https://github.com/entropiaorme/entropiaorme/actions/workflows/ci.yml)
[![Mutation score](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/entropiaorme/entropiaorme/badges/mutation.json)](https://github.com/entropiaorme/entropiaorme/actions/workflows/nightly.yml)

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

pre-commit install   # local hooks mirroring the CI gates (see TESTING.md)
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

Installer chrome assets (header / sidebar BMPs + plain-text MIT licence) live under `frontend/src-tauri/entropia-orme/installer/` and are wired through `bundle.windows.nsis` in `frontend/src-tauri/entropia-orme/tauri.conf.json`.

## Optional dev environment

Beyond the core `just dev` flow, two optional capabilities are available: stable `https://entropiaorme.localhost` URLs via a reverse-proxy + DNS layer (useful for browser bookmarks, DevTools, screenshots, and running multiple checkouts of this repo on the same machine), and per-checkout env-var activation in ad-hoc shells. Skip this section to keep the basic flow unchanged.

- [`caddy`](https://caddyserver.com/): reverse proxy fronting the dev stack on a stable `https://entropiaorme.localhost` hostname instead of a port number. Run `caddy trust` once after install (elevated on Windows, `sudo` on macOS/Linux) to install Caddy's local CA root into the OS trust store. Start via `just proxy-up`. (Windows: `winget install CaddyServer.Caddy`.)
- [`coredns`](https://coredns.io/) (pairs with `caddy`): local DNS resolver answering `*.localhost` → `127.0.0.1`. CoreDNS only answers queries that reach it, and Windows does not route `.localhost` lookups to a configured resolver by default, so the hostname path also needs a one-time Name Resolution Policy rule (below) to direct the `.localhost` namespace at CoreDNS. Start via `just dns-up`. (Windows: `scoop install coredns`.)
- [`direnv`](https://direnv.net/): activates env vars from `.env.local` on `cd`-in so ad-hoc shell commands (`python -m backend.main`, `pytest`, `npm run ...`) honour the local env. (Windows: `scoop install direnv`; run `direnv allow .` once per checkout to whitelist the `.envrc`.)

Once per machine, route the `.localhost` namespace to CoreDNS with a Name Resolution Policy Table (NRPT) rule (elevated PowerShell). This is namespace-scoped: only `.localhost` names resolve through CoreDNS, so all other resolution stays on your normal adapter resolvers.

```powershell
Add-DnsClientNrptRule -Namespace ".localhost" -NameServers "127.0.0.1"
```

Revert with `Get-DnsClientNrptRule | Where-Object { $_.Namespace -contains '.localhost' } | Remove-DnsClientNrptRule -Force`. Without this rule the OS returns NXDOMAIN for `entropiaorme.localhost`, and `just dev` serves the stack on `http://localhost:<port>` instead (a working plain-HTTP session; the rule is what unlocks the HTTPS hostname).

## Further documentation

- [`backend/architecture/README.md`](backend/architecture/README.md): how the backend is put together (the event spine, the hydration HTTP surface, the service and worker conventions, and the tests that enforce them), with a companion [`PORT-READINESS.md`](backend/architecture/PORT-READINESS.md) on how those shapes map onto a contemplated native port.
- [`TESTING.md`](TESTING.md): the test suite, runtime tiers, and CI gates.

## License

[MIT](LICENSE). Third-party components and their licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
