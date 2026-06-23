<#
.SYNOPSIS
  Build the bespoke WiX Burn installer end to end (per-user MSI -> native x86
  bafunctions helper -> themed Burn bundle), producing setup.exe.

.DESCRIPTION
  The single entry point for the Windows installer, used both locally
  (`just installer`) and by the release pipeline. Stages:

    1. Per-user MSI payload  -> scripts/build-msi.ps1
       (Tauri renders + compiles installer/main.wxs; we own the per-user relink.)
    2. Native bafunctions.dll -> installer/burn/bafunctions/build.ps1
       (x86, built from source; flips the bootstrapper window to dark mode.)
    3. WiX Burn bundle        -> dotnet wix build
       (themed WixStdBA wrapping the MSI + bafunctions into one setup.exe.)

  Output:
    frontend/src-tauri/target/release/bundle/burn/EntropiaOrme-<version>-x64-setup.exe

  The release binary and its bundled resources are produced as a side effect of
  stage 1 (under target/release), which is what the portable ZIP is staged from.

  Requirements: Node + the frontend deps (npm ci), the Rust toolchain, the WiX 6
  dotnet tool (.config/dotnet-tools.json), and the MSVC x86 toolset.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$conf     = Join-Path $repoRoot "frontend\src-tauri\entropia-orme\tauri.conf.json"
$burnDir  = Join-Path $repoRoot "frontend\src-tauri\entropia-orme\installer\burn"
$bafDir   = Join-Path $burnDir "bafunctions"
$version  = (Get-Content $conf -Raw | ConvertFrom-Json).version

$msi    = Join-Path $repoRoot "frontend\src-tauri\target\release\bundle\msi\EntropiaOrme_${version}_x64_en-US.msi"
$outDir = Join-Path $repoRoot "frontend\src-tauri\target\release\bundle\burn"
$setup  = Join-Path $outDir "EntropiaOrme-${version}-x64-setup.exe"
$wixExt = "WixToolset.BootstrapperApplications.wixext"

Write-Host "==> [1/3] per-user MSI payload"
& (Join-Path $PSScriptRoot "build-msi.ps1")
if (-not (Test-Path $msi)) { throw "MSI payload not produced at $msi" }

Write-Host "==> [2/3] native x86 bafunctions.dll"
& (Join-Path $bafDir "build.ps1")
if (-not (Test-Path (Join-Path $burnDir "bafunctions.dll"))) { throw "bafunctions.dll not produced" }

Write-Host "==> [3/3] WiX Burn bundle"
Push-Location $repoRoot
try {
    & dotnet tool restore | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "dotnet tool restore failed (exit $LASTEXITCODE)" }
    # Ensure the Burn BA extension is in the global wix cache (the -ext below
    # references it by name). Idempotent; harmless if already present.
    $toolVersion = (Get-Content (Join-Path $repoRoot ".config\dotnet-tools.json") -Raw | ConvertFrom-Json).tools.wix.version
    & dotnet wix extension add -g "$wixExt/$toolVersion" | Out-Host
    if ($LASTEXITCODE -ne 0) { Write-Warning "wix extension add returned $LASTEXITCODE (continuing; the build will be the real gate)" }
} finally { Pop-Location }

New-Item -ItemType Directory -Force -Path $outDir | Out-Null
Push-Location $burnDir
try {
    & dotnet wix build bundle.wxs -d Version=$version -d MsiPath=$msi -ext $wixExt -o $setup | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "wix build (Burn bundle) failed (exit $LASTEXITCODE)" }
} finally { Pop-Location }

if (-not (Test-Path $setup)) { throw "setup.exe not produced at $setup" }
Write-Host ("Built installer ({0:N0} bytes) -> {1}" -f (Get-Item $setup).Length, $setup)
