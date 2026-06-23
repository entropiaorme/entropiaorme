<#
.SYNOPSIS
  Build the installer's native x86 bafunctions.dll reproducibly from source.

.DESCRIPTION
  The DLL flips the WiX Burn bootstrapper window to DWM dark mode and suppresses
  the focus/accelerator cues (thmutil cannot touch the OS frame). It must be x86:
  the Burn BA host is the x86 universal engine, so an x64 build fails to load
  with ERROR_BAD_EXE_FORMAT.

  Inputs, all pinned to WiX 6.0.2 (matching .config/dotnet-tools.json):
    - NuGet WixToolset.BootstrapperApplicationApi -> BA API + balutil headers/lib
    - NuGet WixToolset.DUtil                      -> dutil headers/lib
    - wixstdfn base headers + BalBaseBAFunctionsProc.cpp from wixtoolset/wix,
      at the matching vX.Y.Z tag.

  Downloads land in ./.build (gitignored); re-runs reuse the cached NuGets.
  Output: ../bafunctions.dll (overwrites). Requires the MSVC x86 cross toolset
  (Visual Studio / Build Tools with "VC.Tools.x86.x64").

.PARAMETER WixVersion
  The WiX release to pin inputs to. Defaults to 6.0.2.

.PARAMETER Clean
  Discard the cached .build directory first.
#>
[CmdletBinding()]
param(
    [string] $WixVersion = "6.0.2",
    [switch] $Clean
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # the IWR progress bar is slow on large files
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$here  = $PSScriptRoot
$build = Join-Path $here ".build"
$pkgDir = Join-Path $build "pkg"
$fnDir  = Join-Path $build "wixstdfn"
$fnInc  = Join-Path $fnDir "inc"
$outDll = Join-Path (Split-Path $here -Parent) "bafunctions.dll"   # installer/burn/bafunctions.dll

if ($Clean -and (Test-Path $build)) { Remove-Item -Recurse -Force $build }
New-Item -ItemType Directory -Force -Path $pkgDir, $fnInc | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem

# --- NuGet native packages (.nupkg is a zip): download once, extract, cache ---
function Get-Nupkg([string] $id, [string] $version) {
    $dest = Join-Path $pkgDir $id
    if (Test-Path (Join-Path $dest ".done")) { return $dest }
    $low = $id.ToLowerInvariant()
    $url = "https://api.nuget.org/v3-flatcontainer/$low/$version/$low.$version.nupkg"
    $tmp = Join-Path $build "$id.zip"
    Write-Host "==> download $id $version"
    Invoke-WebRequest -Uri $url -OutFile $tmp
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
    [System.IO.Compression.ZipFile]::ExtractToDirectory($tmp, $dest)
    Remove-Item $tmp
    New-Item -ItemType File -Path (Join-Path $dest ".done") | Out-Null
    return $dest
}

$baApi = Get-Nupkg "WixToolset.BootstrapperApplicationApi" $WixVersion
$dutil = Get-Nupkg "WixToolset.DUtil" $WixVersion

$baInc = Join-Path $baApi "build\native\include"
$baLib = Join-Path $baApi "build\native\v14\x86"
$duInc = Join-Path $dutil "build\native\include"
$duLib = Join-Path $dutil "build\native\v14\x86"
foreach ($p in @($baInc, $baLib, $duInc, $duLib)) {
    if (-not (Test-Path $p)) { throw "expected NuGet payload missing: $p (package layout changed?)" }
}

# --- wixstdfn base headers + the proc implementation, at the pinned tag ---
$raw = "https://raw.githubusercontent.com/wixtoolset/wix/v$WixVersion/src/ext/Bal/wixstdfn"
foreach ($h in @("BAFunctions.h", "IBAFunctions.h", "BalBaseBAFunctions.h", "BalBaseBAFunctionsProc.h")) {
    Invoke-WebRequest -Uri "$raw/inc/$h" -OutFile (Join-Path $fnInc $h)
}
$procCpp = Join-Path $fnDir "BalBaseBAFunctionsProc.cpp"
Invoke-WebRequest -Uri "$raw/BalBaseBAFunctionsProc.cpp" -OutFile $procCpp

# --- locate the MSVC x86 cross toolset ---
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw "vswhere.exe not found; install Visual Studio Build Tools with the C++ x86 toolset." }
$vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsPath) { throw "no Visual Studio install with the VC x86/x64 tools (Microsoft.VisualStudio.Component.VC.Tools.x86.x64) found." }
$vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvarsall.bat"
if (-not (Test-Path $vcvars)) { throw "vcvarsall.bat not found at $vcvars." }

# --- compile inside the x86 dev environment ---
# Run cl from $build so its .obj files land there (gitignored), not in source.
# /MT statically links the CRT so the DLL needs no VC runtime in the download.
$src  = Join-Path $here "bafunctions.cpp"
$def  = Join-Path $here "bafunctions.def"
$sys  = "dwmapi.lib advapi32.lib ole32.lib oleaut32.lib user32.lib shlwapi.lib shell32.lib gdi32.lib uuid.lib msi.lib crypt32.lib wininet.lib version.lib"
$bat  = Join-Path $build "compile.bat"
Set-Content -Path $bat -Encoding ascii -Value @"
@echo off
call "$vcvars" amd64_x86 || exit /b 1
cd /d "$build" || exit /b 1
cl /nologo /LD /MT /EHsc /I"$here" /I"$baInc" /I"$duInc" /I"$fnInc" "$src" "$procCpp" /Fe:"$outDll" ^
   /link /DEF:"$def" /IMPLIB:"$build\bafunctions.lib" /LIBPATH:"$baLib" /LIBPATH:"$duLib" ^
   balutil.lib dutil.lib $sys || exit /b 1
"@
Write-Host "==> compile (x86)"
& cmd /c "`"$bat`""
if ($LASTEXITCODE -ne 0) { throw "bafunctions compile failed (exit $LASTEXITCODE)." }

# --- verify the output is genuinely x86 (PE machine 0x14C) ---
$bytes = [System.IO.File]::ReadAllBytes($outDll)
$peOff = [BitConverter]::ToInt32($bytes, 0x3C)
$machine = [BitConverter]::ToUInt16($bytes, $peOff + 4)
if ($machine -ne 0x14C) { throw ("bafunctions.dll is not x86 (PE machine 0x{0:X4}); the Burn BA host requires x86." -f $machine) }
Write-Host ("Built x86 bafunctions.dll ({0:N0} bytes) -> {1}" -f (Get-Item $outDll).Length, $outDll)
