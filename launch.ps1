$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path $PSScriptRoot).Path
$frontendDir = Join-Path $repoRoot "frontend"
$pythonExe = Join-Path $repoRoot ".venv\Scripts\python.exe"

# Source $repoRoot\.env.local (if present) into the current PowerShell session
# before spawning the backend and tauri child tabs. Start-Process wt.exe
# inherits the parent's env block, so values flow through to both children:
# backend/main.py reads ENTROPIAORME_BACKEND_PORT and ENTROPIAORME_DATA_DIR,
# frontend/vite.config.ts reads ENTROPIAORME_FRONTEND_PORT. Absence of the
# file (the common case for a fresh public-tier clone) is intentional — env
# vars stay unset, runtime defaults bind on the documented ports.
$envLocal = Join-Path $repoRoot ".env.local"
if (Test-Path $envLocal) {
    Get-Content $envLocal | ForEach-Object {
        $line = $_.Trim()
        if ([string]::IsNullOrEmpty($line) -or $line.StartsWith("#")) { return }
        $eqIdx = $line.IndexOf("=")
        if ($eqIdx -lt 1) { return }
        $key = $line.Substring(0, $eqIdx).Trim()
        $value = $line.Substring($eqIdx + 1).Trim()
        if ([string]::IsNullOrWhiteSpace($key)) { return }
        if (($value.Length -ge 2) -and
            ((($value.StartsWith('"')) -and ($value.EndsWith('"'))) -or
             (($value.StartsWith("'")) -and ($value.EndsWith("'"))))) {
            $value = $value.Substring(1, $value.Length - 2)
        }
        Set-Item -Path "env:$key" -Value $value
    }
}

if (-not (Get-Command wt.exe -ErrorAction SilentlyContinue)) {
    Write-Error "Windows Terminal (wt.exe) is required for launch.ps1."
}

if (-not (Test-Path $pythonExe)) {
    Write-Error "Missing backend interpreter at $pythonExe. Create the Windows virtualenv first."
}

if (-not (Test-Path $frontendDir)) {
    Write-Error "Missing frontend directory at $frontendDir."
}

# Dev affordance: tauri.conf.json declares the backend sidecar via
# `bundle.externalBin`, which causes Tauri's build-script-build to check
# the platform-triple-suffixed binary's existence at compile time even
# under `tauri dev`. The dev path runs the Python backend directly in a
# sibling terminal (via `python -m backend.main` below) and never invokes
# the sidecar at runtime, so a zero-byte placeholder satisfies the check.
# Release builds via `backend/build_app.py` replace this stub with the
# real PyInstaller-frozen exe at the same path. The guard is strictly
# "create if missing" so a frozen sidecar from a prior release build
# survives launch.ps1 re-invocations untouched.
$sidecarDir = Join-Path $frontendDir "src-tauri\binaries"
$sidecarStub = Join-Path $sidecarDir "entropiaorme-backend-x86_64-pc-windows-msvc.exe"
if (-not (Test-Path $sidecarStub)) {
    New-Item -ItemType Directory -Force -Path $sidecarDir | Out-Null
    New-Item -ItemType File -Path $sidecarStub | Out-Null
    Write-Host "Staged dev sidecar placeholder at $sidecarStub"
}

$backendCmd = "`"$pythonExe`" -X utf8 -m backend.main"
# Use the tauri:dev npm script so the dev-mode CSP overlay (broader
# connect-src allowing alternate localhost backend ports) is applied
# automatically.
$tauriCmd = "npm run tauri:dev"

Start-Process wt.exe -ArgumentList @(
    "-w", "0",
    "new-tab",
    "--title", "Backend",
    "-d", $repoRoot,
    "cmd", "/k", $backendCmd
)

Start-Sleep -Milliseconds 700

Start-Process wt.exe -ArgumentList @(
    "-w", "0",
    "new-tab",
    "--title", "Tauri",
    "-d", $frontendDir,
    "cmd", "/k", $tauriCmd
)
