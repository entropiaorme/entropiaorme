# Vendored WiX `wixstdfn` sources

These five files are the WiX Toolset BAFunctions base headers and proc
implementation, vendored verbatim from the WiX repository so the installer's
native `bafunctions.dll` builds from in-tree sources rather than fetching them
from a mutable tag path at build time. `build.ps1` compiles these directly; it
no longer reaches `raw.githubusercontent.com`.

## Source

- Repository: <https://github.com/wixtoolset/wix>
- Tag: `v6.0.2` (matches the WixToolset NuGet inputs and `.config/dotnet-tools.json`)
- Upstream directory: `src/ext/Bal/wixstdfn/`

## Licence

Microsoft Reciprocal License (MS-RL). The full text as published at the pinned
tag is vendored alongside as `LICENSE.MS-RL.txt`. Copyright (c) .NET Foundation
and contributors.

## Files and integrity (SHA-256)

Each hash is the SHA-256 of the vendored file as fetched from the tag above.
`build.ps1` is the consumer; the manifest is the audit record. To re-verify:
`sha256sum inc/*.h BalBaseBAFunctionsProc.cpp LICENSE.MS-RL.txt` from this
directory should reproduce these digests.

| File | Upstream path (under the directory above) | SHA-256 |
|---|---|---|
| `inc/BAFunctions.h` | `inc/BAFunctions.h` | `658ea9f8a6a387f8188e94bb3f439d1c2cd06c5b988f95277644b9f7063c49a6` |
| `inc/IBAFunctions.h` | `inc/IBAFunctions.h` | `36003581e0ca57b9bb6da8c20fd7a4f2c0f86d5cf78e255c0337bf23bd2a492d` |
| `inc/BalBaseBAFunctions.h` | `inc/BalBaseBAFunctions.h` | `a203dd6568ee8aec3f40529dbbaaa913bc70a55eb98cea42639f08ba7b2c3634` |
| `inc/BalBaseBAFunctionsProc.h` | `inc/BalBaseBAFunctionsProc.h` | `1ab68a8ff112fea8a295769438a028560dc5c4fb64466b8f10800a5ef19e9d8f` |
| `BalBaseBAFunctionsProc.cpp` | `BalBaseBAFunctionsProc.cpp` | `4a1f4db0774b137f9d7adb70213dfef094f25e1a0d226125d6160dec2321816a` |
| `LICENSE.MS-RL.txt` | `/LICENSE.TXT` (repo root) | `dfdf2048787635215a6baf3b9d461dee89a2904d246787258a44f072a98d4786` |

## Refreshing on a WiX version bump

See `../../README.md` ("Refreshing the vendored inputs"). In short: re-fetch the
six files from the new `vX.Y.Z` tag, replace the SHA-256 digests above and the
NuGet digests in `build.ps1`, and re-run `build.ps1 -Clean` to confirm a green
x86 build.
