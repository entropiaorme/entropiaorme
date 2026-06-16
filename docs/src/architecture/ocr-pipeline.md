# OCR pipeline

EntropiaOrme reads a player's skill levels directly from the in-game skills
panel: the user captures the panel page by page, the application recognises
the text in each cell, and the recognised values are resolved into a map of
canonical skill name to level. This page traces that journey stage by stage.

The pipeline exists in two implementations. The original was written in Python
and runs inside the FastAPI sidecar; the native implementation was ported to
Rust as part of the move from the Python sidecar to a native Rust HTTP spine.
The two are held to behave identically on the same inputs, and the recogniser
in particular is pinned against a recorded ground-truth corpus (see
[Equivalence and the ground-truth bench](#equivalence-and-the-ground-truth-bench)).
For the wider context of the port, see the
[System overview](overview.md) and the
[service and crate map](service-map.md).

## Overview

The skill scan is **manual and user-driven**: there is no continuous screen
watching. The user docks the in-game skills panel in a known on-screen
position, opens a dedicated scan overlay, and clicks "capture" once per page,
manually flipping pages in-game between captures. Only after the final page is
captured does recognition run; the result is then held for an in-app
diff-review screen where the user accepts (persisting the values) or rejects
(discarding them).

The recogniser is an ONNX model. Specifically it is the SVTRv2-mobile text
recogniser, distributed as an ONNX graph and executed through ONNX Runtime.
The Python side loads the bundled model
(`backend/assets/models/svtrv2_rec.onnx`) and drives it via an
`onnxruntime.InferenceSession`; the native side loads the same model file
through the `ort` crate. On Windows with a DirectX 12 GPU the session runs
under the **DirectML** execution provider; otherwise it falls back to the
**CPU** execution provider. Both implementations record which provider was
actually committed.

The model weights ship inside the installer and the recogniser operates fully
offline from a cold start: there is no network access at any point of the read
path.

The native side also carries an **optional second backend** that runs the same
model on candle (a Rust ML framework), behind a default-off feature flag; it is
described under [The candle second backend](#the-candle-second-backend).
ONNX Runtime is the default and the only backend a normal build compiles.

Two consumers share the recogniser:

| Consumer | Input | Output |
| --- | --- | --- |
| Skill-panel scan | A captured panel sliced into per-cell crops | A `name → level` map |
| Repair-cost read | A single small numeric region on the repair terminal | A parsed PED cost |

This page focuses on the skill-panel scan; the repair-cost read
(`backend/services/repair_ocr.py`,
`frontend/src-tauri/eo-services/src/repair_ocr.rs`) reuses the same recogniser
for a single on-demand number and is summarised under
[The shared repair-cost read](#the-shared-repair-cost-read).

## The stages in order

A captured page travels through a fixed sequence. The orchestration lives in
`read_skill_panel` (`backend/services/local_ocr.py`, mirrored by
`read_skill_panel` in `frontend/src-tauri/eo-services/src/skill_panel.rs`); the
device-free post-processing is factored into
`backend/services/skill_panel_parse.py` so it can be unit-tested without the
engine, file IO, or screen-capture glue.

### 1. Screen capture

`ScreenCapturer` (`backend/ocr/capturer.py`) is the single capture path. It
takes a screen rectangle (`x`, `y`, `width`, `height`) and returns either a BGR
`uint8` array or PNG-encoded bytes, owning its `mss` session internally through
a lazy per-thread handle. The manual scan captures the panel region as PNG
bytes (`capture_region_png`), so each page is stored as a self-contained PNG
for preview and persistence; the bytes are produced via `mss.tools.to_png` in a
form that an `IMREAD_COLOR` decode reads back as BGR, keeping the preview and
recognition paths interchangeable.

In the native implementation the capture is supplied to the scan service as an
injected provider (`capture_region` on `ScanProviders`), so the scan logic
stays independent of the platform capture mechanism.

### 2. Image decode and preprocess

When recognition runs, the stored PNG is decoded lazily. `decode_panel_png`
(`backend/services/skill_panel_parse.py`) turns the PNG byte-string into a BGR
`uint8` array via `cv2.imdecode`; the native side does the equivalent in
`load_bgr_png` (`frontend/src-tauri/eo-services/src/ocr_engine.rs`), loading the
PNG as BGR HWC bytes.

Each per-cell crop is then shaped for the model. The recogniser's preprocess
(`RecDynamicResize([48, 320])`) resizes the crop to a fixed height of 48 pixels,
normalises pixel values as `(v / 255 - 0.5) / 0.5`, keeps BGR channel order and
CHW layout, and zero-pads the width. The padded width is
`int(48 * max(w / h, 320 / 48))`: a crop wider than the `320 / 48` aspect floor
pads to track its own aspect ratio, while narrower crops pad out to the floor
width. The Python side uses the upstream OpenOCR preprocess; the native
`preprocess` (`frontend/src-tauri/eo-services/src/ocr_engine.rs`) reproduces the
same resize-normalise-pad shape, with its bilinear resize implemented as a
byte-for-byte port of OpenCV's fixed-point `INTER_LINEAR` path so the input
tensor matches.

### 3. ONNX recognition

The shaped tensor is fed to the ONNX session, which returns per-timestep class
logits. Both implementations decode those logits and produce a `(text, score)`
pair:

* **Text** comes from a CTC decode: a per-timestep argmax (the first maximum
  wins a tie), consecutive-duplicate timesteps collapsed against the previous
  timestep, and the blank class dropped. The decode alphabet is the PaddleOCR v1
  key set with the CTC blank prepended and the space character appended
  (`load_dict` in `frontend/src-tauri/eo-services/src/ocr_engine.rs`).
* **Score** is the mean of the kept timesteps' probabilities, or `0.0` when no
  characters survive.

On the Python side, a cell whose confidence falls below `OCR_CONFIDENCE_WARN`
(0.85) emits a backend warning but still flows through; the user is expected to
catch any misread in the accept/reject diff review rather than have the cell
silently dropped. The native skill-panel reader does not carry that logging
surface, consistent with logging being omitted from the ported core.

### 4. Per-cell parsing

A panel is not a single text field: it is a grid, and each row carries a name
cell, a level cell, and a bar cell. The calibrated geometry (see
[Geometry and vocabulary inputs](#geometry-and-vocabulary-inputs)) drives
`slice_panel_cells`, which walks rows top to bottom and, within each row, the
cells in geometry order, producing one crop per cell tagged with its row index
and cell name. Each cell type is parsed differently:

| Cell | Parse | Result |
| --- | --- | --- |
| `name` | OCR text, then fuzzy resolution against the vocabulary | A canonical skill name, or `None` |
| `level` | First integer run of the OCR text (`parse_level`) | An integer, or `None` |
| `bar` | Fill-ratio estimate over the bar pixels (`parse_bar_fill`); no OCR | A fraction in `[0, 1)` |

`parse_level` (`backend/services/skill_panel_parse.py`) reads the first run of
digits from the level cell's recognised text. `parse_bar_fill` estimates the
fractional progress within the current level directly from the bar crop's
pixels: it takes the per-column mean luminance, thresholds at the midpoint of
the column-mean range, and reports the rightmost bright column over the bar
width (roughly 1% resolution on a 95-pixel bar). Low-contrast bars (where no
fill edge is detectable, including empty bars) return `0.0`; a reading of `1.0`
is treated as impossible mid-bar (the in-game bar would just have levelled up),
so it is read as a misread of an empty bar and flipped to `0.0`. The native
`parse_bar_fill` (`frontend/src-tauri/eo-services/src/skill_panel.rs`) carries
the same logic, including a fixed-point BGR-to-grey conversion matched to the
original.

### 5. Fuzzy skill-name resolution

The recognised name text is a lookup key, not display text. `fuzzy_resolve`
(`backend/services/skill_panel_parse.py`, mirrored in
`frontend/src-tauri/eo-services/src/skill_panel.rs`) resolves it against the
canonical skill vocabulary snapshot, returning the chosen canonical entry, a
score, and the top candidates. Resolution proceeds in tiers and stops at the
first that matches:

1. **Exact match.** If the cleaned text is present in the vocabulary verbatim,
   it is taken as-is (score 100).
2. **Normalised match.** Otherwise the text is normalised (whitespace removed,
   lower-cased) and compared against each vocabulary entry under the same
   normalisation. This covers case and spacing drift, for example `whip` versus
   `Whip` or `FoodTechnology` versus `Food Technology` (score 100).
3. **Fuzzy match.** Otherwise the text is scored against the whole vocabulary
   with the `rapidfuzz` WRatio scorer, taking the top candidates; the
   best-scoring vocabulary entry is selected as the canonical name.

The canonical entry is what gets persisted. The native `extract_top`
reproduces the WRatio scoring so the two implementations select the same
candidate.

### 6. Aggregation into a name-to-level map

`read_skill_panel` groups the parsed cells by row. For each row it combines the
integer from the level cell with the fractional bar fill into a single level
value (`int_level + bar_fill`); a row whose level cell yielded no integer has a
`None` level even when a bar was read. Rows whose name does not resolve are
still emitted (with `name = None`) and the caller decides their fate. The
per-page extractor (`extract_page_levels`,
`backend/services/skill_scan_core.py`) then filters to rows that have both a
resolved name and a non-`None` level, yielding a `{canonical_name: level}` map
for the page.

Across a multi-page scan the per-page maps merge in page order, with later
pages overwriting earlier entries for a duplicated name. The native
implementation preserves first-seen ordering while applying the same
later-page-wins overwrite (`extract_levels` in
`frontend/src-tauri/eo-services/src/skill_scan_manual.rs`).

### 7. Persistence via the completion callback

The aggregated map is not written directly. It is held as a pending result for
the diff-review screen; on **accept**, the scan service hands it to a
completion callback installed by the composition root, which performs the
actual persistence. A callback failure surfaces as an error on the scan status
and leaves the pending result intact so the user can retry. On **reject**, the
pending result is discarded. The scan service itself never owns the storage
concern.

## The manual scan state machine

The user-driven flow is a small state machine over an owned scan state. It is
implemented by `SkillScanManual` (`backend/services/skill_scan_manual.py`) and
its native port (`frontend/src-tauri/eo-services/src/skill_scan_manual.rs`), and
exposed over HTTP by `backend/routers/scan_manual.py`.

### Phases

The reported phase is derived from the owned state:

| Phase | Condition | Meaning |
| --- | --- | --- |
| `idle` | No active scan, nothing processing or pending | Resting state |
| `capturing` | A scan is active | Capturing pages |
| `processing` | Recognition is running | Background extraction in flight |
| `awaiting_review` | A pending result is held | The diff review is open |

The status payload also reports the captured page count, the expected page
count, per-page processing progress (`done`/`total`), whether the engine is
available, whether the game window is present, and any error string.

### Endpoints

The router drives the service verbs under the `/scan/skills` prefix:

| Endpoint | Verb | Effect |
| --- | --- | --- |
| `POST /scan/skills/start` | `start` | Begin a scan; resolves the region and the (optional) page count |
| `POST /scan/skills/capture` | `capture_current_page` | Grab the current page; stores its PNG (or records a failed grab) |
| `POST /scan/skills/undo` | `undo_last_capture` | Pop the most recent capture, stepping the user back one page |
| `POST /scan/skills/process` | `process` | Kick off recognition on a background worker |
| `POST /scan/skills/accept` | `accept` | Persist the held result via the completion callback |
| `POST /scan/skills/reject` | `reject` | Discard the held result |
| `POST /scan/skills/cancel` | `cancel` | Abandon the active scan and reset |
| `GET /scan/skills/status` | `get_status` | Read the current status |
| `GET /scan/skills/pending` | `get_pending_result` | Read the held result awaiting review |
| `GET /scan/skills/capture/{page}` | `get_capture_png` | Read a captured page's PNG for preview |

The default page count is 12; a scan may request between 1 and 30 pages, and a
request outside that range is refused. The verbs guard their preconditions:
`start` refuses when the engine is unavailable, the game window is not found, or
a result is already pending; `process` refuses until the expected number of
pages has been captured; `undo` and `process` refuse while processing or while
a result awaits review.

### Background worker

`process` does not run recognition on the request thread. It snapshots the
captures, flips the state to `processing`, and spawns a background worker that
runs the per-page extraction. This is deliberate: the ONNX session is
**single-threaded** (both implementations build it with a single intra-op and
inter-op thread, sequential execution, and a guard serialising calls), so the
pages are extracted **serially** to avoid contention on the shared engine. As
each page resolves, the worker advances the `done`/`total` progress. When the
worker finishes it stores the result as the pending review (or records an
error) and clears the processing flag. The worker catches all failures so a
crash settles the state cleanly rather than wedging the scan; the native worker
additionally exposes a join handle for orderly shutdown and test rigs.

Status changes are announced over the in-process event bus as a
`scan.status.changed` envelope, coalesced so each settled transition emits
exactly one frame. The envelope carries only the coarse phase; a listener
re-hydrates the full status via the status `GET`, so per-page progress stays
live without widening the wire. This push-then-hydrate shape is shared with the
rest of the application's eventing.

## Geometry and vocabulary inputs

Two committed data files drive the parse:

* **Panel geometry** (`backend/data/panel_geometry.json`) defines the
  per-cell grid for each panel. The `skill` entry declares the row count
  (`n_rows`) and, per cell (`name`, `level`, `bar`), the left/right x bounds,
  the y-offset of the first and last row's top, and the cell height. Row tops
  are interpolated linearly between the first and last offsets across `n_rows`,
  with banker's rounding (round half to even) so the Python and native slicers
  land on the same pixel rows. The file also carries a `profession` entry with
  its own cells (`name`, `rank_level`, `percent`, `bar`). The geometry is what
  makes the slicer panel-shape-agnostic: a recalibration changes the file, not
  the code.
* **Skill vocabulary snapshot** (`backend/data/snapshot/skills.json`) is the
  canonical list of skill names that fuzzy resolution matches against. Each
  entry carries a `name` (plus auxiliary fields such as category and HP
  increase); the OCR path reads the `name` values to form the vocabulary. The
  snapshot is what `fuzzy_resolve` resolves recognised text into, so what gets
  persisted is always a canonical vocabulary entry rather than raw recognised
  text.

## Equivalence and the ground-truth bench

Because two implementations read the same screens, the recogniser is held to a
recorded baseline rather than left free to drift. A ground-truth corpus pins
the recogniser's output: a set of graded panel cell crops, each annotated with
its expected screen-verbatim text. The native recogniser is run over every
graded cell and its raw exact-match count is held to the figure the original
Python engine recorded over the same cells; the port must not fall below it.

The bench is implemented in
`frontend/src-tauri/eo-services/tests/ocr_bench_differential.rs`. It grades 594
data cells and asserts the native engine's raw-exact count is at least the
original engine's recorded figure of 262 over the same cells. The raw exact
count is strict against screen-verbatim grading: spacing and case drift in the
raw model text is precisely what the downstream name resolution recovers, so
the production read path's effective accuracy sits well above the raw figure.

The bench runs only where its inputs are present. The captured gameplay screens
are held locally and kept out of the public tree, so the test runs only when
`EO_OCR_BENCH_DIR` points at the corpus and the ONNX Runtime library is
loadable; otherwise it skips with its reason stated rather than passing
vacuously. The same host-gating applies to the provider-selection tests in
`frontend/src-tauri/eo-services/src/ocr_engine.rs`, which additionally run only
on Windows, where the bundled Windows ONNX Runtime build is present.

The rationale for pinning equivalence to this recorded corpus, rather than
chasing a moving accuracy target, is recorded in
[ADR-0008: OCR equivalence frozen to the corpus](../adr/0008-ocr-equivalence-frozen.md).

## The candle second backend

Alongside the ONNX Runtime engine, the recogniser carries an optional second
backend that runs the same SVTRv2 model on
[candle](https://github.com/huggingface/candle), a Rust ML framework, with no
ONNX Runtime dependency. It is compiled only under the `candle` Cargo feature,
which is **off by default**: a normal build neither compiles nor links it, so
the default (and soaked) recognition path is unchanged. It is not the default
and is not intended as one (it is slower on CPU; see below); it is an
alternative implementation of the same recogniser.

Both backends plug into a shared `InferenceBackend` seam in
`frontend/src-tauri/eo-services/src/ocr_engine.rs`, so the candle path
reimplements only the inference step. Everything around it (the PNG decode, the
OpenCV-exact preprocess, and the CTC decode) is the same shared code both
backends run.

### How the candle backend is built

The candle backend (`frontend/src-tauri/eo-services/src/ocr_candle.rs`)
implements the SVTRv2-mobile forward pass from scratch: the LCNet convolutional
encoder (a two-conv stem, then thirteen RepMixer blocks, with squeeze-excite on
a subset and two height-halving downsample blocks), the SVTR sequence neck (the
height pooled to a single row, two global multi-head-attention transformer
blocks, then a convolutional re-fusion), and the CTC linear head. The weights
come from a safetensors export of the same bundled ONNX model, produced by the
reproducible `backend/scripts/convert_svtrv2_to_safetensors.py` (it lifts the
ONNX graph's weight initialisers, transposes the linear weights into candle's
layout, and writes `backend/assets/models/svtrv2_rec.safetensors`).

### Faithfulness and the comparison sweep

The candle backend is held to **reproduce** the ONNX Runtime engine, not to be
independently different. The comparison harness
(`frontend/src-tauri/eo-services/tests/ocr_backend_comparison.rs`) checks this
two ways: a single-cell tensor-level diff (the post-softmax probabilities agree
to **cosine 1.000000**, max-abs around 6e-5, with every timestep's argmax
identical), and a corpus-level sweep over all 594 graded bench cells:

| Backend | Raw-exact | Raw accuracy | Recognition latency (CPU) |
| --- | --- | --- | --- |
| ONNX Runtime (default) | 262 / 594 | 44.1% | ~37 ms / cell |
| candle (`--features candle`) | 262 / 594 | 44.1% | ~158 ms / cell |

candle matches ONNX Runtime on **every one of the 594 cells** (identical
raw-exact count, 100% per-cell text agreement), so it inherits the same
recovered accuracy through the downstream name resolution. It is roughly **four
times slower** on CPU: candle's CPU kernels have no equivalent to ONNX
Runtime's tuned inference. The gap is wider on a GPU host, where the default
engine uses DirectML and candle has no matching backend (its GPU support is
CUDA and Metal, not DirectML), so the comparison above is CPU-to-CPU. That
latency gap, not any accuracy difference, is why candle stays an optional
backend and ONNX Runtime remains the default. (The latencies are
release-build means over the 594-cell corpus; the accuracy figures are
build-independent. The recogniser's hot path
also pools its preprocess scratch buffers so repeated cells allocate neither
the resize buffer nor the input tensor; `benches/ocr.rs` carries the
preprocess and recognition microbenchmarks.)

Like the bench differential, the comparison harness and the OCR
microbenchmarks are host-gated: they run only where `EO_OCR_BENCH_DIR` and the
ONNX Runtime library are present, and skip with a stated reason otherwise.

The decision to carry a second backend, and to keep it default-off, is recorded
in
[ADR-0014: an optional candle OCR backend](../adr/0014-candle-second-ocr-backend.md).

## The shared repair-cost read

The repair-cost read reuses the recogniser for a single number rather than a
panel. Given the repair terminal's region (derived from the live game window),
it captures one frame, recognises the cost text, and parses it into a PED value:
commas are read as decimal points, spaces are dropped, and the first digit run
with an optional single fraction is taken (`parse_cost` in
`frontend/src-tauri/eo-services/src/repair_ocr.rs`, ported from
`backend/services/repair_ocr.py`). Each failure leg (window not found, invalid
region, capture failure, engine unavailable) surfaces a distinct error while
still returning a zeroed cost, so the caller's contract is preserved. It shares
the capture and recognition seams with the skill scan but holds no multi-page
state machine.
