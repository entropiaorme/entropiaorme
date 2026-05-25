"""One-command release bundle for EntropiaOrme.

Freezes the FastAPI backend with PyInstaller, stages the resulting exe
at the Tauri externalBin path with the platform-triple suffix that
`bundle.externalBin: ["binaries/entropiaorme-backend"]` expects, runs
`npm run tauri:build` to produce the Tauri shell + NSIS installer, and
post-processes those into the brand-canonical artefact names with
matching SHA256 sidecars: a renamed NSIS installer and a portable zip
containing the Tauri shell, the sidecar exe, and a short README.

Canonical invocation (from the repo root, with the dev venv active or
via the venv python directly):

    .venv/Scripts/python.exe backend/build_app.py

Output (at `frontend/src-tauri/target/release/`):

    entropiaorme-<version>-x64-setup.exe              NSIS installer (~148 MB; sidecar bundled inside)
    entropiaorme-<version>-x64-setup.exe.sha256       SHA256 sidecar
    entropiaorme-<version>-x64-portable.zip           Portable bundle (Tauri shell + sidecar + README.txt)
    entropiaorme-<version>-x64-portable.zip.sha256    SHA256 sidecar

The portable zip extracts to a folder containing both exes alongside the
README; running `entropia-orme.exe` from the extracted folder boots the
sidecar that sits next to it. The SHA256 sidecar files are single-line
`<hash>  <filename>` (two-space separator, sha256sum -c compatible).

For Tauri-shell-only iterations (sidecar unchanged) skip this script
and run `npm run tauri:build` from `frontend/` directly. The freeze
step dominates total build time.
"""

import hashlib
import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SPEC = PROJECT_ROOT / "backend" / "build_sidecar.spec"
FROZEN_EXE = PROJECT_ROOT / "dist" / "entropiaorme-backend.exe"
FRONTEND = PROJECT_ROOT / "frontend"
TAURI_CONF = FRONTEND / "src-tauri" / "tauri.conf.json"
TAURI_BIN_DIR = FRONTEND / "src-tauri" / "binaries"
TAURI_TARGET = TAURI_BIN_DIR / "entropiaorme-backend-x86_64-pc-windows-msvc.exe"
TAURI_RELEASE_DIR = FRONTEND / "src-tauri" / "target" / "release"
TAURI_PORTABLE_EXE = TAURI_RELEASE_DIR / "entropia-orme.exe"
NSIS_DIR = TAURI_RELEASE_DIR / "bundle" / "nsis"

PORTABLE_README = (
    "EntropiaOrme - Portable\n"
    "\n"
    "Extract this folder anywhere, then run entropia-orme.exe. The\n"
    "entropiaorme-backend.exe sidecar must stay next to entropia-orme.exe.\n"
    "\n"
    "Settings persist to %APPDATA%\\Roaming\\EntropiaOrme\\ on the host\n"
    "machine; truly-portable mode is not in scope for the 0.x window.\n"
    "\n"
    "More info: https://entropiaorme.com\n"
)


def step(label: str) -> None:
    print(f"\n=== {label} ===", flush=True)


def read_version() -> str:
    return json.loads(TAURI_CONF.read_text(encoding="utf-8"))["version"]


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def write_sha256_sidecar(artefact: Path) -> Path:
    sidecar = artefact.with_name(artefact.name + ".sha256")
    sidecar.write_text(f"{sha256_of(artefact)}  {artefact.name}\n", encoding="utf-8")
    return sidecar


def main() -> int:
    if sys.platform == "win32":
        # openocr-python hard-depends on plain `onnxruntime`, which gets pulled
        # in alongside `onnxruntime-directml` and (since both share the same
        # `onnxruntime/` package dir) the last touched install wins. Force a
        # reinstall here so the directml binaries are the ones PyInstaller
        # collects, which is what enables `DmlExecutionProvider` in the frozen
        # sidecar. `--no-deps` keeps the rest of the env untouched.
        step("Pinning onnxruntime-directml binaries (Windows)")
        subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                "--force-reinstall",
                "--no-deps",
                "onnxruntime-directml",
            ],
            cwd=PROJECT_ROOT,
            check=True,
        )

    step("PyInstaller-freezing backend sidecar")
    subprocess.run(
        [sys.executable, "-m", "PyInstaller", str(SPEC), "--noconfirm"],
        cwd=PROJECT_ROOT,
        check=True,
    )

    step("Staging sidecar at Tauri externalBin path")
    TAURI_BIN_DIR.mkdir(parents=True, exist_ok=True)
    shutil.copy2(FROZEN_EXE, TAURI_TARGET)
    print(f"{FROZEN_EXE.name} -> {TAURI_TARGET}")

    step("npm run tauri:build")
    # Use npm.cmd directly (Windows shim); no shell so argv stays argv.
    subprocess.run(
        ["npm.cmd", "run", "tauri:build"],
        cwd=FRONTEND,
        check=True,
    )

    version = read_version()

    step(f"Renaming NSIS installer to brand-canonical form (v{version})")
    nsis_default = NSIS_DIR / f"EntropiaOrme_{version}_x64-setup.exe"
    nsis_renamed = NSIS_DIR / f"entropiaorme-{version}-x64-setup.exe"
    if not nsis_default.exists():
        print(
            f"ERROR: {nsis_default} not found; expected `tauri:build` to produce it.",
            file=sys.stderr,
        )
        return 1
    if nsis_renamed.exists():
        nsis_renamed.unlink()
    nsis_default.rename(nsis_renamed)
    print(f"{nsis_default.name} -> {nsis_renamed.name}")

    step("Building portable zip")
    portable_zip = TAURI_RELEASE_DIR / f"entropiaorme-{version}-x64-portable.zip"
    portable_root = f"entropiaorme-{version}-x64-portable"
    if portable_zip.exists():
        portable_zip.unlink()
    with zipfile.ZipFile(portable_zip, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.write(TAURI_PORTABLE_EXE, f"{portable_root}/entropia-orme.exe")
        # Source is the staged platform-triple-suffixed sidecar; arcname drops
        # the suffix so the extracted layout has the runtime-expected name.
        zf.write(TAURI_TARGET, f"{portable_root}/entropiaorme-backend.exe")
        zf.writestr(f"{portable_root}/README.txt", PORTABLE_README)
    print(f"Created {portable_zip.name}")

    step("Generating sha256 sidecars")
    for artefact in (nsis_renamed, portable_zip):
        sidecar = write_sha256_sidecar(artefact)
        print(sidecar.name)

    step("Done")
    print(f"NSIS installer:  {nsis_renamed}")
    print(f"Portable bundle: {portable_zip}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
