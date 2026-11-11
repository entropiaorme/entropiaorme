# OCR pipeline

EntropiaOrme reads a player's skill points directly from the game's own
Skills window: while a scan is running the application watches the game
window, reads each page of the window's SKILLS tab as the player pages
through it, checks the figures against the total the game prints, and
resolves each row's name against the bundled skill list. The same recogniser
also serves on-demand repair-cost, Trade Terminal value, auction sale-window,
and map-coordinate reads. This page traces the shared machinery and the skill
scan's journey stage by stage.

The pipeline runs in-process in the native Rust spine, and the Rust
implementation is the implementation. The recogniser is pinned against a
recorded ground-truth corpus (see
[Equivalence and the ground-truth bench](#equivalence-and-the-ground-truth-bench)).
For the wider context, see the [System overview](overview.md) and the
[service and crate map](service-map.md).

## Overview

The skill scan runs only while the player asks for it. The player opens the
scan overlay, starts a scan, and pages through the Skills window in the game;
about twice a second the scan captures the game window, reads the page
showing, and keeps it once every row of it has read. Nothing is pressed per
page, and nothing is captured unless a scan is collecting. When every page is
in, the result is held for an in-app review where the player accepts
(persisting the values) or rejects (discarding them).

The recogniser is an ONNX model. Specifically it is the SVTRv2-mobile text
recogniser, distributed as an ONNX graph and executed through ONNX Runtime.
The engine loads the bundled model
(`app/src-tauri/entropia-orme/resources/models/svtrv2_rec.onnx`) through
the `ort` crate. The session prefers the platform's GPU execution provider
and falls back to the **CPU** provider when no usable GPU is present: on
Windows with a DirectX 12 GPU that is **DirectML**, on Linux with a Vulkan
driver it is **WebGPU** (Dawn over Vulkan). The engine records which provider
was actually committed. Running inference on the GPU is a deliberate design
choice, not an optimisation: the game the recogniser reads is CPU-limited, so
OCR belongs on the GPU the game leaves idle rather than the CPU it competes
for. The ONNX Runtime itself is platform-forked: Windows bundles the DirectML
build, Linux bundles the official WebGPU-enabled build
(`app/src-tauri/entropia-orme/resources/ort-linux/`; see its
`PROVENANCE.txt`), each resolved and pinned by absolute path.

The model weights ship inside the installer and the recogniser operates fully
offline from a cold start: there is no network access at any point of the read
path.

Five consumers share the recogniser:

| Consumer | Input | Output |
| --- | --- | --- |
| Skill scan | The whole game window, searched for the Skills window, page after page | Each skill's points exactly as shown, checked against the game's total, for review |
| Repair-cost read | The whole game window, searched for the Repair Terminal and its labelled cost | The cost exactly as shown, or a typed refusal |
| Trade Terminal value read | The whole game window, searched for the Trade Terminal and its labelled total | The total exactly as shown, or a typed refusal, for a limited-set observation |
| Sale-window read | One calibrated auction panel sliced into independently validated fields | A reviewable listing draft |
| Map-coordinate read | The whole game window, searched for the radar and the coordinates printed under it | Longitude and latitude exactly as shown, or a typed refusal per value |

This page focuses on the skill scan; the repair-cost and Trade Terminal reads
(`app/src-tauri/eo-services/src/repair_ocr.rs`) reuses the same recogniser
for a single on-demand number and is summarised under
[The shared repair and Trade Terminal reads](#the-shared-repair-and-trade-terminal-reads). The auction reader
(`app/src-tauri/eo-services/src/sale_window_ocr.rs`) is described under
[The auction sale-window read](#the-auction-sale-window-read). The coordinate
reader (`app/src-tauri/eo-services/src/coord_capture.rs`) finds the radar in
the whole game window (`game_ui/radar.rs`) and reads `LON` and `LAT` under it,
each checked by grammar, by the width of its own lettering and by the planet's
bounds; a position is answered only when both read, and a refusal carries no
number. The planet's name printed above the radar is read too, and a position
under a radar naming another planet is refused. Once the radar is found, the
next read captures only its neighbourhood. A rectangle the player draws by hand is read instead only when the
radar cannot be found, under the same grammar and bounds. The same module
finds the radar for the route guidance overlays, with the map's scale read
from the distances printed on its rings.

## The stages in order

Every reader of the game's interface runs the same stages, implemented in
`app/src-tauri/eo-services/src/game_ui/`: find the window, cut out the text
that matters, recognise it, and refuse anything that does not validate. What
the client's interface looks like (its window frame, its titles, its table
headings, its rule lines) is data in the bundled interface profile
(`app/src-tauri/entropia-orme/resources/ui_profile.json`), so a redesigned
client is answered by editing that file, not the code. The skill scan's own
reader is `read_skills_page` (`app/src-tauri/eo-services/src/game_ui/skills.rs`).

### 1. Screen capture

The game window is captured whole: the client area of the game's window, as
one BGR frame, through the shared `GameWindowFrame` seam
(`app/src-tauri/eo-services/src/game_ui/seams.rs`), which the composition root
builds once over `capture_game_window`
(`app/src-tauri/eo-services/src/eu_window.rs`). The answer is the frame, or why
there is none: the game window was not found, lies outside what can be
captured, or could not be captured.

On Windows the capture is an on-demand GDI capture. On Linux it acquires a
user-consented ScreenCast portal session and consumes its PipeWire node through
GStreamer. The portal's D-Bus connection is driven by a dedicated,
process-lifetime Tokio worker because the PipeWire grant remains valid only
while that peer is alive; the GStreamer pipeline retains the newest
full-monitor frame and crops requested rectangles from it.

### 2. Finding the Skills window and its table

The client's windows are found by their frame, and the Skills window by the
title along its top, wherever the player has put it and at whatever size. A
window whose extent is uncertain, or that runs off the game window, is refused.
Inside it, the table's header names the tab: its name and points headings
belong to the SKILLS tab alone, so another tab is refused. The table's rule
lines divide the rows and its dividers the columns; a row is read only when the
whole of it is in view and uncovered, and the table's scrollbar says whether
the whole list is in view, so a window too short for a whole page is told
apart from one that shows it all.

### 3. Recognition

Each run of lettering is shaped for the model and recognised. The recogniser's
preprocess resizes a crop to a fixed height of 48 pixels, normalises pixel
values as `(v / 255 - 0.5) / 0.5`, keeps BGR channel order and CHW layout, and
zero-pads the width. The padded width is `int(48 * max(w / h, 320 / 48))`: a
crop wider than the `320 / 48` aspect floor pads to track its own aspect ratio,
while narrower crops pad out to the floor width. `preprocess`
(`app/src-tauri/eo-services/src/ocr_engine.rs`) implements this
resize-normalise-pad shape, with its bilinear resize written as a byte-for-byte
match of OpenCV's fixed-point `INTER_LINEAR` path so the input tensor is exact.

The shaped tensor is fed to the ONNX session, which returns per-timestep class
logits. The engine decodes them into a `(text, score)` pair:

* **Text** comes from a CTC decode: a per-timestep argmax (the first maximum
  wins a tie), consecutive-duplicate timesteps collapsed against the previous
  timestep, and the blank class dropped. The decode alphabet is the PaddleOCR v1
  key set with the CTC blank prepended and the space character appended
  (`load_dict` in `app/src-tauri/eo-services/src/ocr_engine.rs`).
* **Score** is the mean of the kept timesteps' probabilities, or `0.0` when no
  characters survive.

The score never decides whether a reading is believed: a recogniser can be
confident and wrong. What is believed is decided by validation.

### 4. Validation of each row

A row's points must have exactly the shape the client prints (digits grouped
in thousands by commas, then any fraction) and their ink must be as wide as
that reading predicts, which catches a digit lost or doubled; points crowding their
column's divider, which may have lost leading digits, are refused. A row whose
points do not validate is unread, never estimated.

A row's name is resolved against the bundled skill list
(`resolve_name` in the same module): text that spells a skill up to case,
spacing and punctuation is that skill; a name the client cut short with an
ellipsis is the one skill it begins; a slip of a character or two from exactly
one skill, clear of every other, is a near miss of that skill. Text near no
skill is a skill the game has and the list lacks, kept as such and never given
a listed name; text that could be either of two skills leaves the row unread.

### 5. The sweep across pages

A page is kept only when it is whole: every row in view read, none cut or
covered, and the scrollbar showing the whole list. The sweep
(`SkillsSweep`) holds the latest whole reading of each page number, in any
order, for one listing: a page from another category or another page count is
turned away, and a page whose skills differ from those already held for its
number leaves that page missing until a later reading agrees with one of the
two. When every page is in, the sweep merges them and checks them against the
total the game prints: the whole points of every skill counted must add up to
it. In the all-categories listing, a name the list did not know is looked at
once more among the skills the sweep has not read, at a lower similarity floor;
the skill it comes nearest is only a likely guess, never a reading, and in a
single category's listing (where nearly every skill is unread) no guess is
made at all.

### 6. The review

What accepting saves is each skill the sweep read on its page exactly once,
by the bundled list's name, with its points as read: spelled whole, or a near
miss of exactly one skill (`app/src-tauri/eo-services/src/skill_scan/review.rs`).
The review also tells the player, only when true, that the figures do not add
up to the game's total or could not be checked against it, which skills were
matched from a slightly misread name, which names were only guessed at, and
which skills the game shows that this version does not know; guesses, unknown
skills, names that could be either of two skills, and skills read more than
once are shown and never saved. A review whose figures do not add up, or could
not be checked, leads with scanning again; saving it takes an explicit second
request.

### 7. Persistence via the completion callback

The review is not written directly. On **accept**, which names the review it
settles, the scan service hands its skills to a completion callback installed
by the composition root, which performs the persistence. Each level is written
stamped with the moment its page was captured, not the moment of acceptance:
pages are read over minutes and a review can wait longer, while the chat log
goes on recording gains, and the current level is the latest in time, so a
reading never supersedes a gain recorded after it. A callback failure is
answered to the player and leaves the review intact so they can retry. On
**reject**, the review is discarded. The scan service itself never owns the
storage concern.

## The scan service

The scan is implemented by `SkillScanService`
(`app/src-tauri/eo-services/src/skill_scan/`) and exposed over typed Tauri
commands by the scan facade (`app/src-tauri/eo-api/src/scan.rs`).

### Phases

| Phase | Meaning |
| --- | --- |
| `idle` | No scan; carries whether one can run (the recogniser, the profile and the skill list loaded) and why the last scan stopped by itself, if it did |
| `collecting` | Pages are being collected; carries how many of how many, which are missing, and what, if anything, stops the current view being collected |
| `awaitingReview` | Every page is in; the review is open |
| `saving` | The accepted review is being saved; every other request is refused at once |

What stops the current view being collected is one plain hint the player can
act on: the game window or the Skills window is not found, the Skills window is
covered or off the screen, it shows another tab, it is too short to show a
whole page, the page did not read, or it is from another category than the
pages already collected. A hint is shown once it has held for two looks
running, and clears the same way.

A scan stops by itself when reading fails, when screen capture is not allowed
(on a desktop that asks the player's consent; it never asks again by itself),
and when a while (three minutes by default, a setting of the service) passes
with no new page.

### Commands

| Command | Effect |
| --- | --- |
| `scan_status` | Read the current status |
| `scan_start` | Start collecting; a scan already under way is left as it is; refused while saving or while the scan window is closed |
| `scan_cancel` | Stop collecting; refused while saving |
| `scan_pending` | Read the review awaiting acceptance, with the number of its scan |
| `scan_accept` | Save the named review via the completion callback; one whose figures do not add up only when the request says to save it anyway |
| `scan_reject` | Discard the named review |

Collecting happens only while the scan overlay shows: the shell tells the scan
when the overlay is shown, and when it is hidden or closed, under the same lock
a start checks, so no start can slip in after the overlay goes. A review keeps
its result, since it lives in the main window.

### The worker

Each scan has its own number, its own sweep and a single worker thread of its
own, so recognition never runs on an async runtime thread and nothing one scan
read can reach another. Looks are taken one at a time across workers, so a scan
started just after another ended never reads beside it. The worker captures and
reads a page, writes the outcome back only while its scan is still the one
collecting, and waits one tick (half a second by default, a setting of the
service) before the next look. Leaving the collecting phase wakes and ends the
worker; a worker caught mid-read drops what it read, and a failure anywhere in
a look or its write-back ends the scan rather than leaving it showing as
collecting. At app exit the service stops collecting and waits a short while
for its worker; one held inside a capture by a consent prompt is not waited
for.

Status changes are announced over the in-process event bus as a
`scan.status.changed` envelope, coalesced so each change of status emits
exactly one frame. The envelope carries only the coarse phase; a listener
re-reads the full status, so progress and hints stay live without widening the
wire. This push-then-read shape is shared with the rest of the application's
eventing.

## Interface and vocabulary inputs

Two committed data files drive the read:

* **Interface profile**
  (`app/src-tauri/entropia-orme/resources/ui_profile.json`) describes how the
  client draws its interface: the window frame, the titles, the Skills table's
  headings, total labels, scrollbar and the thresholds the reader measures
  against. It holds no positions: a window is found wherever it is.
* **Skill vocabulary snapshot**
  (`app/src-tauri/entropia-orme/resources/snapshot/skills.json`) is the
  list of skill names and categories rows are resolved against, so what gets
  persisted is always a listed skill name rather than raw recognised text. The
  game can add skills before the bundled list catches up; such a skill is
  shown in the review as one this version does not know, and not saved.

## Equivalence and the ground-truth bench

The recogniser is held to a recorded baseline rather than left free to drift. A
ground-truth corpus pins the recogniser's output: a set of graded panel cell
crops, each annotated with its expected screen-verbatim text. The recogniser is
run over every graded cell and its raw exact-match count is held to a committed
ground-truth figure; the engine must not fall below it.

The bench is implemented in
`app/src-tauri/eo-services/tests/ocr_bench_differential.rs`. It grades 594
data cells and asserts the engine's raw-exact count is at least the committed
figure of 262 over the same cells. The raw exact count is strict against
screen-verbatim grading: spacing and case drift in the raw model text is
precisely what the downstream name resolution recovers.

The bench runs only where its inputs are present. The captured gameplay screens
are held locally and kept out of the public tree, so the test runs only when
`EO_OCR_BENCH_DIR` points at the corpus and the ONNX Runtime library is
loadable; otherwise it skips with its reason stated rather than passing
vacuously. The same host-gating applies to the provider-selection tests in
`app/src-tauri/eo-services/src/ocr_engine.rs`, which additionally run only
on Windows, where the bundled Windows ONNX Runtime build is present; the Linux
runtime is exercised by the platform port's scan-chain verification rather than
these Windows-pinned provider-ladder tests.

The rationale for pinning equivalence to this recorded corpus, rather than
chasing a moving accuracy target, is recorded in
[ADR-0008: OCR equivalence frozen to the corpus](../adr/0008-ocr-equivalence-frozen.md).

## The shared repair and Trade Terminal reads

The repair-cost and Trade Terminal reads capture the whole game window and
hand it to the interface reader (`app/src-tauri/eo-services/src/game_ui/`),
which finds the terminal by the title the client prints along its top and
the figure by its label (`REPAIR COST:`, `TOTAL AMOUNT:`), wherever the
player has put the window and at whatever size. What the client's interface
looks like is data in the bundled interface profile
(`app/src-tauri/entropia-orme/resources/ui_profile.json`), not constants in
code. A figure is accepted only when its reading has exactly the shape of a
PED amount and its ink is as wide as that reading predicts; anything else is
refused rather than approximated.

The answer is the figure, or a typed refusal naming why there is none: the
game window was not found or could not be captured, the terminal is not open
(or its title is covered), the figure is covered or out of view, the figure
did not read, or no recogniser or profile is available. A refusal carries no
number, so it can never be recorded as a cost; the player sees what to change
and can always enter the figure by hand.

The limited-protection read uses the Trade Terminal total. The player places
either all seven armour pieces or all seven plates in the terminal, without
selling them, and reviews the value before confirming it. Successive
confirmed readings measure consumed TT; the protection service, not OCR,
applies the set's frozen average markup and spreads the result over the
sessions the player ticks (ADR-0031).

## The auction sale-window read

The sale-window reader is another one-shot consumer of the shared capture and
recognition seams. Its calibrated panel and per-field rectangles live in
`app/src-tauri/entropia-orme/resources/panel_geometry.json`. The service
captures the panel once, verifies a fixed heading landmark before trusting any
field rectangle, then recognises the item name, quantity, TT value, auction fee,
duration, starting bid, and buyout from crops of that single frame. Each field
must clear the recogniser confidence floor and its own loose plausibility
bounds; unreadable fields remain empty and are named back to the frontend so a
plausible-looking misread is never silently substituted.

The result is deliberately not an accounting command. The typed
`inventory_sale_window_capture` operation returns a draft which fills the
ordinary Create listing form, where the player reviews and may correct every
value before the existing listing command is allowed to mutate stock or the
ledger. A narrowly permissioned always-on-top overlay exposes the same capture
while a fullscreen game is in front. Opening that overlay from the main window
grants one capture, which the overlay consumes while receiving only success or
failure status. The complete result is held in memory for a single-use
`inventory_sale_window_take_capture` collection when the main form regains
focus; it is never persisted. OCR therefore changes how the draft is filled,
not the commit boundary it must cross.

Development builds add a deliberately separate research session around the
same reader (`app/src-tauri/eo-services/src/auction_fee_research.rs`). While
that session is armed, each Space press uses the existing shared keyboard
source to capture another quoted auction scenario. The service writes a
versioned JSONL record, an analysis-friendly CSV row, and a SHA-256-hashed PNG
under the development data directory. The floating overlay sees
only capture state and sample count; it never receives the OCR fields or the
dataset path. This instrument performs no listing, inventory, ledger, or P&L
mutation, and release builds refuse to start it.
