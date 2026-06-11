# Third-Party Notices

EntropiaOrme bundles the following third-party assets and libraries. The Software itself is licensed under the [MIT License](LICENSE); the components below retain their respective upstream licenses.

## Bundled assets

### OpenOCR SVTRv2 mobile recognition model

- **File:** `backend/assets/models/svtrv2_rec.onnx` (~24 MB)
- **Source:** [`openocr-python`](https://github.com/Topdu/OpenOCR) by Topdu — `develop0.0.1` release artefact `openocr_rec_model.onnx`, byte-for-byte (SHA256 `2cebf56ec416d97a3a656337ae026502fbb95be11400c2ec8df512404f225085`)
- **Used by:** `backend/services/local_ocr.py` (the single OCR engine: skill / profession panel scans and repair-window cost reads)
- **License:** Apache License 2.0
- **Notice:** OpenOCR is Copyright (c) Topdu and contributors, distributed under Apache License 2.0. The bundled model artefact is governed by the same upstream license. The model file ships inside the installer; the application performs no network access for OCR.

### PaddleOCR character dictionary

- **File:** `backend/assets/models/ppocr_keys_v1.txt`
- **Source:** the PaddleOCR `ppocr_keys_v1` key set, redistributed verbatim inside [`openocr-python`](https://github.com/Topdu/OpenOCR) (SHA256 `28b2362ad4ab2dc38769aa72feb535e3a9ddb3fd2a7585a05920e6393b1dc7f7`)
- **Used by:** the OCR engines as the recognition model's decode alphabet (`backend/services/local_ocr.py` via its bundled package copy; `frontend/src-tauri/eo-services/src/ocr_engine.rs` via this file)
- **License:** Apache License 2.0
- **Notice:** PaddleOCR is Copyright (c) PaddlePaddle authors, distributed under Apache License 2.0. The dictionary ships inside the installer beside the model it decodes.

### Entropia Universe game-data snapshot

- **Files:** `backend/data/snapshot/*.json` (weapons, weapon_amplifiers, medical_tools, mobs, professions, skills, skill_ranks, stimulants, absorbers, enhancers, weapon_vision_attachments).
- **Source:** Curated subset re-bundled from [Entropia Nexus](https://entropianexus.com/), a community-maintained wiki for Entropia Universe. The underlying constants (item names, statistics, catalogue identifiers, mob species, profession names, skill names) originate with MindArk PE AB as the publisher of Entropia Universe; Entropia Nexus's contribution is the structured bundling.
- **Used by:** backend equipment library, mob taxonomy, profession and skill panel scans, demo seeder; loaded at startup as static reference content (no runtime fetch).
- **Notice:** Bundled with the permission of Entropia Nexus. EntropiaOrme is independent and unofficial; it is not affiliated with, endorsed by, or sponsored by either Entropia Nexus or MindArk PE AB. Item names and statistics are factual references; "Entropia Universe" and related names remain trademarks of MindArk PE AB (see "Game references" below).

## Python dependencies

The Python sidecar's runtime dependencies (`backend/requirements.txt`) and dev dependencies (`backend/requirements-dev.txt`) are pulled from PyPI under their published licenses. Notable inclusions:

- `fastapi`, `uvicorn`, `pydantic` — MIT
- `onnxruntime` (and `onnxruntime-directml` on Windows) — MIT
- `opencv-python-headless` — Apache 2.0
- `numpy` — BSD-3-Clause
- `rapidfuzz` — MIT
- `pynput` — LGPL-3.0
- `openocr-python` — Apache 2.0 (see above)
- `PyInstaller` (dev) — GPL-2.0 with bootloader exception (the bootloader exception permits redistribution of the resulting frozen binary under any license)

## Frontend dependencies

The Tauri shell's npm dependencies (`frontend/package.json`) and Rust dependencies (the `frontend/src-tauri/` cargo workspace) are pulled from npm and crates.io under their published licenses. The Tauri framework itself is dual-licensed Apache 2.0 / MIT.

## Game references

References to "Entropia Universe" and related names, logos, and assets are descriptive only. Those names are trademarks or registered trademarks of MindArk PE AB. EntropiaOrme is independent and unofficial; it is not affiliated with, endorsed by, or sponsored by MindArk PE AB. See <https://entropiaorme.com/terms> for the full notice.
