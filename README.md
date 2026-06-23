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

- Python 3.11+: for the test suite and the cross-language equivalence oracle (the shipped application is a single Rust binary and bundles no Python)
- Node.js ≥ 20.19
- Rust (`rustup`): for the Tauri shell and the native backend
- Visual Studio Build Tools (MSVC C++ workload): required by Tauri on Windows
- Windows Terminal (`wt.exe`): used by the launcher
- [`just`](https://just.systems/) ≥ 1.34: task runner driving `just dev` etc. (Windows: `scoop install just`).

### Setup

From the repo root:

```bash
python -m venv .venv
.venv\Scripts\activate
pip install -r backend/requirements.txt
pip install -r backend/requirements-dev.txt   # pytest (`just test-backend`) and the cross-language equivalence oracle

cd frontend
npm install
cd ..

pre-commit install   # local hooks mirroring the CI gates (see TESTING.md)
```

### Run

```bash
just dev
```

Opens two Windows Terminal tabs: a Python backend and the Tauri dev shell. Release builds embed the backend in the shell process and run it in-process; the standalone Python backend is a development affordance and is not part of a shipped build.

Run `just --list` to see other recipes (`just check` for frontend type-check + build, `just test-backend` for the pytest suite).

### Build installer

```bash
just installer
```

Builds the bespoke WiX Burn installer end to end (the per-user MSI payload, the native x86 bootstrapper helper, and the themed Burn bundle) and writes `EntropiaOrme-<version>-x64-setup.exe` to `frontend/src-tauri/target/release/bundle/burn/`. This is equivalent to running `scripts/build-installer.ps1` directly, and needs WiX 6 (the `wix` dotnet tool), the MSVC x86 toolset, and the Tauri build chain. The installer wraps the single Rust binary together with its data, model, and ONNX Runtime assets; there is no separate backend process inside it.

Installer sources (the WiX template, the Burn bundle, the themed bootstrapper, and the native helper) live under `frontend/src-tauri/entropia-orme/installer/`; the branded art is generated from the app icon and design tokens by `installer/burn/compose-art.py`.

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

- The [architecture handbook](docs/): an mdBook covering the system overview, the service and crate map, the event taxonomy, the OCR pipeline, and the database schema reference, with the [architecture decision records](docs/src/adr/) alongside. It is published to GitHub Pages from `main`, tracking the latest landed state, together with the generated `cargo doc` API reference. Build it locally with `mdbook build docs`.
- [`backend/architecture/README.md`](backend/architecture/README.md): how the backend is put together (the event spine, the hydration HTTP surface, the service and worker conventions, and the tests that enforce them), with a companion [`PORT-READINESS.md`](backend/architecture/PORT-READINESS.md) analysing how those shapes mapped onto the native port.
- [`TESTING.md`](TESTING.md): the test suite, runtime tiers, and CI gates.
- [`SECURITY.md`](SECURITY.md): the security policy, including the supply-chain review gates over dependencies and bundled assets, and the SBOM, checksums, and build-provenance attestation the release pipeline attaches to each release.

## License

[MIT](LICENSE). Third-party components and their licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
