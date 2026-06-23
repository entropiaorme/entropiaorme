# Builds the per-user MSI payload that the WiX Burn installer wraps.
#
# Why this script exists: Tauri's MSI bundler hardcodes a per-machine install
# and exposes no light/ICE knob (tauri-apps/tauri#13792). EntropiaOrme installs
# per-user (no UAC, into %LOCALAPPDATA%\Programs), so the install scope and
# directory are set in the custom WiX template at
# frontend/src-tauri/entropia-orme/installer/main.wxs. A per-user install trips
# the ICE38/ICE64 validations because the resource components Tauri generates use
# file keypaths (valid for a single-user install, but ICE is conservative), so we
# own the final link: Tauri renders the template and compiles it (candle), then
# this script links the compiled object (light) with the per-user ICEs
# suppressed. Tauri's own link attempt fails those ICEs and is discarded;
# success is defined by this script producing the MSI, not by Tauri's exit code.

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$frontend = Join-Path $repoRoot "frontend"
$wixDir = Join-Path $repoRoot "frontend\src-tauri\target\release\wix\x64"
$bundleDir = Join-Path $repoRoot "frontend\src-tauri\target\release\bundle\msi"
$conf = Join-Path $repoRoot "frontend\src-tauri\entropia-orme\tauri.conf.json"

# 1. Build the app and render + compile the installer. Tauri's own light step
#    fails the per-user ICEs; its exit code is deliberately ignored here, and the
#    compiled object is verified below instead.
Write-Host "==> tauri build --bundles msi (its light step is expected to fail the per-user ICEs)"
Push-Location $frontend
& npx tauri build --bundles msi
Pop-Location

# 2. Verify candle produced a fresh object. If it did not, the build failed
#    before compiling the installer (a real error, not the expected ICE failure).
$wixobj = Join-Path $wixDir "main.wixobj"
$wxs = Join-Path $wixDir "main.wxs"
if (-not (Test-Path $wixobj)) { throw "WiX object not produced ($wixobj): the Tauri build failed before compiling the installer." }
if ((Get-Item $wixobj).LastWriteTime -lt (Get-Item $wxs).LastWriteTime) { throw "WiX object is stale: the Tauri build did not recompile the installer." }

# 3. Resolve Tauri's auto-downloaded WiX 3 toolset.
$light = Get-ChildItem (Join-Path $env:LOCALAPPDATA "tauri\WixTools*\light.exe") -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $light) { throw "WiX light.exe not found under $env:LOCALAPPDATA\tauri\WixTools*; a Tauri MSI build fetches it on first run." }
$loc = Get-ChildItem (Join-Path $wixDir "*.wxl") | Select-Object -First 1

# 4. Link with the per-user ICE validations suppressed (ICE38: profile component
#    file keypaths; ICE64/90/91: profile-directory warnings). Output to Tauri's
#    canonical bundle path so the Burn step and release pipeline find it.
$version = (Get-Content $conf -Raw | ConvertFrom-Json).version
$msi = Join-Path $bundleDir "EntropiaOrme_${version}_x64_en-US.msi"
New-Item -ItemType Directory -Force -Path $bundleDir | Out-Null
Write-Host "==> light (per-user link): $msi"
& $light.FullName -ext WixUIExtension -ext WixUtilExtension -cultures:en-US -loc $loc.FullName `
    -sice:ICE38 -sice:ICE64 -sice:ICE90 -sice:ICE91 `
    $wixobj -o $msi
if ($LASTEXITCODE -ne 0) { throw "light failed to link the per-user MSI (exit $LASTEXITCODE)." }
Write-Host "Built per-user MSI: $msi"
