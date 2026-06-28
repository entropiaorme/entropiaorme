# Ratification: hydration-only tracking surface

Adversarial review of the testing-oracle output that accompanies consolidating
the tracking read surface onto the single `/api/tracking/snapshot` hydration
endpoint and removing the three superseded readouts it replaced.

## Change under review

The three superseded tracking GET routes (`/api/tracking/status`,
`/api/tracking/live`, `/api/tracking/recent-events`) and their
`/api/demo/tracking/*` mirrors were removed from the HTTP surface, along with the
two now-orphaned response models (`TrackingStatus`, `TrackingLive`). The
consolidated `/api/tracking/snapshot` endpoint (and its demo mirror), which
already serves the union of those readouts and recomputes it self-containedly,
is unchanged, as is its `TrackingSnapshot` model.

## Oracle delta reviewed

- `backend/tests/expected/openapi.snapshot.json` (regenerated): an object-level
  comparison of the spec confirms exactly six path blocks removed (three
  production `/api/tracking/{status,live,recent-events}` plus three
  `/api/demo/tracking/*` mirrors) and two component schemas removed
  (`TrackingStatus`, `TrackingLive`). Every surviving path and schema,
  `TrackingSnapshot` included, is byte-for-byte unchanged; no path, operation, or
  schema field was silently dropped, and no dangling `$ref` to a removed schema
  remains. The apparent additions in the textual diff are realignment re-emitting
  the unchanged `TrackingSnapshot` schema, not new content.
- The per-scenario HTTP-response goldens for the removed routes
  (`GET_tracking_status`, `GET_tracking_live`, `GET_tracking_recent_events`
  across the seven captured scenarios) were deleted as orphans: the response
  fingerprinter only writes captured endpoints and never deletes dropped ones, so
  these are removed by hand. The surviving `GET_tracking_snapshot` goldens are
  unchanged.

## Verdict

The delta is a faithful, minimal projection of an intended behaviour change. The
removed routes had no remaining callers (the frontend client wrappers were
deleted with no surviving call sites), the consolidated snapshot handler was
already self-sourcing and never coupled to the removed handlers (so no runtime
payload was degraded behind an unchanged schema), and the surviving contract is
untouched. Removing the dead routes and adapting the oracle to match is the
correct action, not a regression pinned as the new truth.

```text
ORACLE-RATIFICATION
range: 2a52b58..HEAD
goldens: openapi.snapshot.json, basic_hunt_10_events, consistency_codex_isolation_midpoint, consistency_quests_mission_lifecycle_midpoint, consistency_scan_isolation_midpoint, consistency_tracking_hunt_midpoint, mission_completion_with_reward_suppression, multi_mob_hunt_loot_grouping
VERDICT: ratification-sound
```
