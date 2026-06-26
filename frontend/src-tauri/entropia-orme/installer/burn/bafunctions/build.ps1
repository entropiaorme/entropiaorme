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
    - wixstdfn base headers + BalBaseBAFunctionsProc.cpp, vendored in-tree under
      ./vendor/wixstdfn (no build-time network fetch; see that directory's
      PROVENANCE.md for the source tag, licence, and per-file SHA-256).

  The build is hermetic by content: the only network inputs are the two NuGets,
  and each is verified against a recorded SHA-256 and fails closed on mismatch,
  so a tampered or substituted package aborts the build rather than compiling
  into the shipped DLL.

  Downloads land in ./.build (gitignored); re-runs reuse the cached NuGets.
  Output: ../bafunctions.dll (overwrites). Requires the MSVC x86 cross toolset
  (Visual Studio / Build Tools with "VC.Tools.x86.x64").

.PARAMETER WixVersion
  The WiX release to pin inputs to. Defaults to 6.0.2. The vendored wixstdfn
  sources and the asserted NuGet SHA-256 digests below are pinned to this
  version; bumping it requires refreshing both (see vendor/wixstdfn/PROVENANCE.md
  and README.md), or the SHA-256 assertion fails closed by design.

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
$fnDir  = Join-Path $here "vendor\wixstdfn"   # vendored in-tree (see vendor/wixstdfn/PROVENANCE.md)
$fnInc  = Join-Path $fnDir "inc"
$outDll = Join-Path (Split-Path $here -Parent) "bafunctions.dll"   # installer/burn/bafunctions.dll

if ($Clean -and (Test-Path $build)) { Remove-Item -Recurse -Force $build }
New-Item -ItemType Directory -Force -Path $pkgDir | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem

# Expected SHA-256 of each NuGet (.nupkg), pinned to WixVersion. A NuGet package
# at a fixed id+version is immutable on nuget.org, so these digests are stable;
# asserting them makes a tampered or substituted package fail the build instead
# of compiling into the shipped DLL. Refresh both when bumping WixVersion
# (see README.md "Refreshing the vendored inputs"). Keyed by lowercased id.
$NupkgSha256 = @{
    "wixtoolset.bootstrapperapplicationapi" = "899a3d88db31098d87fbb24c2e72fa1d2dbacf9b38af73a91dae762b0653e6f5"
    "wixtoolset.dutil"                      = "3428929aec192370ae17ac834e05f8b9b423b5346141e277c4f216094830c9de"
}

# --- NuGet native packages (.nupkg is a zip): the download is cached, but the
# verification and extraction run on every call so each build is fail-closed ---
function Get-Nupkg([string] $id, [string] $version) {
    $low = $id.ToLowerInvariant()
    $expected = $NupkgSha256[$low]
    if (-not $expected) { throw "no pinned SHA-256 for NuGet '$id'; add it to `$NupkgSha256 before building." }
    # Key the cache by id AND version, so a WixVersion bump can never reuse a
    # previous version's bytes: only the download is cached, never the trust.
    $nupkg = Join-Path $pkgDir "$id.$version.nupkg"
    $dest  = Join-Path $pkgDir "$id.$version"
    if (-not (Test-Path $nupkg)) {
        $url = "https://api.nuget.org/v3-flatcontainer/$low/$version/$low.$version.nupkg"
        Write-Host "==> download $id $version"
        Invoke-WebRequest -Uri $url -OutFile $nupkg
    }
    # Verify on every call, a cache hit included, so the SHA-256 gates every
    # build and not just the first. Fail closed: a mismatch means the bytes are
    # not the pinned package.
    $actual = (Get-FileHash -Algorithm SHA256 -Path $nupkg).Hash.ToLowerInvariant()
    if ($actual -ne $expected.ToLowerInvariant()) {
        Remove-Item -Force $nupkg
        throw "SHA-256 mismatch for $id $version`n  expected $expected`n  actual   $actual`nRefusing to build from an unverified package."
    }
    Write-Host "    verified SHA-256 $actual"
    # Re-extract from the just-verified package every run, so a stale or locally
    # modified extraction can never reach the compiler.
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
    [System.IO.Compression.ZipFile]::ExtractToDirectory($nupkg, $dest)
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

# --- wixstdfn base headers + the proc implementation, vendored in-tree ---
# No build-time fetch: these are committed under vendor/wixstdfn (provenance and
# per-file SHA-256 recorded there). Assert they are present so a partial checkout
# fails with a clear message rather than an opaque compiler error.
$procCpp = Join-Path $fnDir "BalBaseBAFunctionsProc.cpp"
$vendored = @($procCpp) + @("BAFunctions.h", "IBAFunctions.h", "BalBaseBAFunctions.h", "BalBaseBAFunctionsProc.h" |
    ForEach-Object { Join-Path $fnInc $_ })
foreach ($f in $vendored) {
    if (-not (Test-Path $f)) { throw "vendored wixstdfn source missing: $f (see vendor/wixstdfn/PROVENANCE.md)." }
}

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
