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

### Setup

From the repo root:

```bash
python -m venv .venv
.venv\Scripts\activate
pip install -r backend/requirements.txt
pip install -r backend/requirements-dev.txt   # only needed for `backend/build_app.py`

cd frontend
npm install
cd ..
```

### Run

```bash
launch.bat
```

Opens two Windows Terminal tabs: FastAPI backend + Tauri dev shell.

On first invocation `launch.ps1` stages a zero-byte placeholder at `frontend/src-tauri/binaries/entropiaorme-backend-x86_64-pc-windows-msvc.exe`. This is intentional dev affordance, not cruft: `tauri.conf.json` declares the sidecar via `bundle.externalBin`, which causes Tauri's build script to check the platform-triple-suffixed binary's existence at compile time even under `tauri dev`. The dev path runs the Python backend directly in a sibling terminal and never invokes the sidecar; the placeholder just satisfies the existence check. `backend/build_app.py` overwrites this stub with the real PyInstaller-frozen exe at release-build time, and the launcher leaves any existing file at that path untouched.

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
