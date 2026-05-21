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
- [`caddy`](https://caddyserver.com/) (optional): reverse proxy fronting the dev stack on a stable `https://entropiaorme.localhost` hostname instead of a port number. Run `caddy trust` once after install (elevated on Windows, `sudo` on macOS/Linux) to install Caddy's local CA root into the OS trust store. Start via `just proxy-up`. Skip to keep the port-based `just dev` flow. (Windows: `winget install CaddyServer.Caddy`.)
- [`coredns`](https://coredns.io/) (optional, pairs with `caddy`): local DNS resolver answering `*.localhost` → `127.0.0.1` so the dev hostname above resolves through every OS resolver path; needed because Windows Winsock doesn't honour RFC 6761 for `.localhost` subdomains. Start via `just dns-up`; configure the primary network adapter's DNS once per machine (snippet below). (Windows: `scoop install coredns`.)

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

If you installed `coredns`, configure your primary network adapter's DNS once per machine (elevated PowerShell — the secondary upstream keeps non-`.localhost` resolution working when CoreDNS is down):

```powershell
$iface = (Get-NetAdapter | Where-Object Status -eq 'Up' | Select-Object -First 1).Name
Set-DnsClientServerAddress -InterfaceAlias $iface -ServerAddresses '127.0.0.1','1.1.1.1'
```

Revert with `Set-DnsClientServerAddress -InterfaceAlias $iface -ResetServerAddresses`.

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
