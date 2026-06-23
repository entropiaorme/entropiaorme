# Installer BA-functions helper

A tiny native (x86) `WixStandardBootstrapperApplication` helper whose only job
is to switch the bootstrapper window's title bar to DWM immersive dark mode
(`DwmSetWindowAttribute`). thmutil styles everything inside the window, but it
cannot touch the OS frame; this closes that last gap with **no .NET runtime** in
the download (purely native).

It is referenced from `../bundle.wxs` as
`<Payload SourceFile="bafunctions.dll" bal:BAFunctions="yes" />`.

## Build

The output **must be x86**: the Burn BA host is x86 (the universal engine), even
though the MSI payload is x64. Building x64 yields `ERROR_BAD_EXE_FORMAT` at load.

Inputs, all pinned to WiX **6.0.2** (match the `wix` tool in
`.config/dotnet-tools.json`):

- NuGet `WixToolset.BootstrapperApplicationApi` 6.0.2 -> `build/native/include`
  (BA API + dutil-adjacent headers) and `build/native/v14/x86/balutil.lib`.
- NuGet `WixToolset.DUtil` 6.0.2 -> `build/native/include` (dutil headers) and
  `build/native/v14/x86/dutil.lib`.
- From `github.com/wixtoolset/wix` tag `v6.0.2`, `src/ext/Bal/wixstdfn/`:
  the four base headers `inc/{BAFunctions,IBAFunctions,BalBaseBAFunctions,BalBaseBAFunctionsProc}.h`
  and the proc implementation `BalBaseBAFunctionsProc.cpp` (compiled alongside
  `bafunctions.cpp`).

Compile with the MSVC x86 toolset (`vcvarsamd64_x86`):

```
cl /nologo /LD /MT /EHsc /I<baapi-inc> /I<dutil-inc> /I<wixstdfn-inc> ^
   bafunctions.cpp BalBaseBAFunctionsProc.cpp ^
   /Fe:..\bafunctions.dll ^
   /link /DEF:bafunctions.def /LIBPATH:<baapi-x86> /LIBPATH:<dutil-x86> ^
   balutil.lib dutil.lib dwmapi.lib advapi32.lib ole32.lib oleaut32.lib ^
   user32.lib shlwapi.lib shell32.lib gdi32.lib uuid.lib msi.lib crypt32.lib ^
   wininet.lib version.lib
```

Output: `../bafunctions.dll` (x86; verify `dumpbin /headers` shows `14C machine`).

## Productionisation (pending)

This is currently built by hand and the DLL is gitignored. The release pipeline
should restore the two NuGets, fetch the four pinned `wixstdfn` headers + the
proc source, and run the compile above as a step before the Burn bundle build.
The window caption is currently hard-coded (`EntropiaOrme Setup`); if the bundle
name ever changes, update the `FindWindowW` call in `bafunctions.cpp`.
