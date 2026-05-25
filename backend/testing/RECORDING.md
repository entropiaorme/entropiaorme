# Recording scenarios from live gameplay

This is the companion to `AUTHORING.md`. Where that document covers
hand-authoring scripted scenarios via the DSL, this one covers capturing
**recorded** scenarios from a live session. Recorded scenarios live under
`backend/tests/e2e/corpus/recorded/<name>/` and replay through the same
production pipeline and golden machinery as scripted ones.

## Why record

Some gameplay moments are rare and non-repeatable: a codex tier promotion, a
profession unlock, a notable rare loot, an enhancer break on a specific item
shape. You cannot reliably script these because the realistic event ordering
and timing are hard to fabricate, and you cannot re-run them on demand.
Recording mode turns each such moment, once it happens, into a permanent
regression fixture: the verbatim chat.log slice plus any scan captures and
keystrokes that fired during the session, replayable forever.

The intended cadence: keep recording mode running during any session likely to
contain a rare event, and finalise the capture as a named scenario when one
occurs.

## What gets captured

Recording installs three observer taps on the live services. They are pure
observers: production behaviour is unchanged while recording, and unchanged
again when it stops.

- **Chat events** (`chat_replay.log`): every line the chat.log watcher tails,
  copied verbatim. This is the replayable core of the bundle.
- **Scan captures** (`scan_captures/`): each skill-scan page and repair-cost
  capture, saved as a PNG with a JSON sidecar recording the panel, the source
  region, and a timestamp.
- **Keystrokes** (`keystrokes.jsonl`): hotbar-slot presses and spacebar
  press/release edges, with monotonic offsets from the recording start. Only
  the narrow key range the live listeners already observe is captured; nothing
  broader.

Note that scan captures only land if a scan actually fires during the session,
and keystrokes only land while the hotbar and spacebar listeners are themselves
running (their own capability toggles plus an active tracking session). The
recorder cannot capture input the live listeners would not see.

### What is replay-verified today

Only the **chat-event surface** has a replay-side consumer at present, so only
`chat_replay.log` is golden-verified. The bundle still preserves
`scan_captures/` and `keystrokes.jsonl` permanently (that is the whole point of
capturing a precious moment), but their replay verification activates with the
screen-capture and keystroke-source replay layers that land later. A recorded
bundle's `expected/` therefore covers the chat-driven fingerprint and DB
snapshot only; its `metadata.yaml` states this explicitly under `verification`.

## How to enable

Recording is a developer-only capability and is double-gated:

1. It is compiled out of production builds entirely; the surface appears only
   when running the app in development.
2. Within a development run, it appears only when **Developer mode** is on.
   Turn it on in **Settings → Developer → Developer mode** (off by default).

The backend enforces the same flag server-side, so the recording endpoints
refuse to act unless developer mode is enabled, independent of the UI.

## The workflow

1. Enable developer mode (above). A **Session recording** panel appears in the
   Developer settings cluster.
2. Click **Start recording**. A live indicator shows the running counts of
   captured lines, scan captures, and keystrokes.
3. Play. For a hunt-shaped scenario, a few minutes is plenty; for a rare-event
   capture, record across the window the event is likely to occur in.
4. Click **Stop & name scenario** and fill in the metadata:
   - **scenario_name**: a lowercase slug (letters, digits, underscores). This
     becomes the directory name and the test's stable identifier.
   - **description**: a one-line summary of what the scenario covers.
   - **notes**: anything useful to a future reader (character context, what
     rare event it captured, caveats).
5. Click **Finalise scenario**. The bundle is moved atomically into
   `corpus/recorded/<scenario_name>/`, its `expected/` goldens are generated
   from the first replay, and the determinism check runs (see below).
6. Review the result surfaced in the panel. The bundle is now a local working
   fixture: keep it for replay and promote it into the maintainer's central
   store (see "Where recorded bundles live" below) rather than committing it to
   the public repo.

**Discard** aborts an in-progress recording and deletes the staging directory
without finalising; use it for a bad take.

## The determinism check

Immediately after finalisation, the recorder replays the just-captured
`chat_replay.log` through a throwaway pipeline twice: once to generate the
goldens, then again to assert against them. If the two replays diverge, the
panel reports a **determinism leak** with a diff.

A leak is a signal, not a nuisance. It means something in the recording or in a
production code path is non-deterministic (a wall-clock dependency, an unstable
ordering, a hash-map iteration leaking into output). The fix is to find and
correct that non-determinism, **not** to re-run until it happens to pass and
ratify the flaky output as the golden. The bundle and its goldens are left on
disk for inspection when a leak is reported.

## Bundle layout

```
corpus/recorded/<name>/
  metadata.yaml         # name, flavour: recorded, description, surfaces,
                        # character_context, counts, notes, verification scope
  chat_replay.log       # verbatim tailed chat.log lines
  scan_captures/        # <NNNN>-<panel>.png + <NNNN>-<panel>.json (if any fired)
  keystrokes.jsonl      # captured hotbar/spacebar edges (if any fired)
  expected/
    fingerprint.jsonl   # golden event stream (chat-driven)
    db_state.json       # golden DB snapshot (chat-driven)
```

To pin a recorded scenario as an explicit regression test, add or repoint a
test under `backend/tests/e2e/` that loads it and calls
`goldens.assert_matches(...)`, following the scripted-scenario convention in
`AUTHORING.md`.

## Where recorded bundles live

Recorded bundles are **local-by-default**. A finalised bundle is a real slice
of live gameplay (your own chat, scan images of your account's panels,
keystroke timings), so it is not committed to the public project repo. Instead:

- The synthetic `placeholder_recorded_hunt` is the one tracked, public
  recorded-flavour fixture; it keeps the recorded-scenario replay path green
  for any reader or CI without exposing real gameplay.
- `corpus/recorded/*` is gitignored in the project repo (everything except the
  placeholder), so a real bundle sitting on disk never reaches the public
  remote.
- Real bundles are kept in a local, un-pushed store outside the project repo
  and copied back into `corpus/recorded/` when the development environment is
  set up. That store is their durable home and the input the later
  screen-capture and keystroke replay layers build against.

Publishing a real bundle is a deliberate act, reserved for a genuine shared-CI
need (for example, multiple collaborators needing to run the same recorded
regression). At that point, and only then, do the privacy pass below before
committing it to a public surface.

## Privacy note (before any deliberate publication)

`chat_replay.log` is a verbatim slice of the live chat.log and may contain
chat from other channels (global, local, society, private messages) that
happened to be written during the recording window; `scan_captures/` are images
of your own account's panels. While a bundle stays local (the default) this is
simply your own data on your own disk. Before ever promoting a bundle to a
public surface, review it and trim anything you would not want public, then
regenerate its goldens (see below).

## What not to do

- Don't ratify a determinism leak by re-recording until it passes. Fix the
  non-determinism.
- Don't hand-edit a recorded `chat_replay.log` to "clean it up" beyond the
  privacy trim above; the goldens are derived from it, and silent edits desync
  the fixture from what the pipeline actually produced. Regenerate the goldens
  if you do trim it.
- Don't treat the presence of `scan_captures/` or `keystrokes.jsonl` as proof
  those surfaces are verified. Until their replay layers land, they are
  preserved but unasserted.
