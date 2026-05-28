"""Snapshot / event-stream consistency harness.

A hydrating client that fetches a snapshot once and then follows the
bus must converge on the same state a fresh re-fetch would return. The
``ConsistencyHarness`` mechanises that property: stream a scenario up
to a midpoint, snapshot the surface, install a reducer that folds the
remaining bus events into the snapshot, stream the rest, then compare
the reducer's hydrated-and-folded state against a fresh snapshot.
Divergence between the two surfaces a bug in either the snapshot view
or the reducer (or both).

Midpoint authoring uses a two-file scenario layout, with the pre-
midpoint events in ``chat_replay.log`` and the post-midpoint events in
``chat_replay_after.log``. The filesystem split is the marker: there is
no embedded sentinel line, no production parser branch, no brittle
event-index parameter coupling the test to scenario length. Scenarios
that do not exercise the consistency property keep their single
``chat_replay.log`` and stay unchanged.

The harness is surface-pluggable via ``SurfaceAdapter``: each surface
provides a snapshot view function (composing the relevant impl
functions today; substitutable for a consolidated snapshot endpoint
when one exists) and a reducer factory. Adding a new surface to the
consistency suite is one ``SurfaceAdapter`` and one scenario, with no
changes to the harness lifecycle.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from backend.core.event_bus import EventBus
from backend.testing.replay import wait_for_drain
from backend.testing.store_reducers import Reducer


@dataclass(frozen=True)
class SurfaceAdapter:
    """Per-surface plug-in for the consistency harness.

    Carries the snapshot view function (a callable producing the
    surface's dict-shaped state, typically by composing the existing
    ``*_impl`` router helpers) and a fresh reducer factory. The
    factory shape rather than a long-lived reducer instance is
    deliberate: every consistency scenario gets a brand-new reducer
    with empty initial state, so a previous scenario's running totals
    cannot leak into the next.
    """

    name: str
    view_fn: Callable[[Any], dict[str, Any]]
    reducer_factory: Callable[[], Reducer]


@dataclass(frozen=True)
class ConsistencyResult:
    """The four observables a consistency run produces.

    ``snapshot_t0`` and ``snapshot_t1`` are the surface's hydration
    view at the midpoint and at the end of the scenario, both freshly
    composed from the live backend state. ``hydrated_state`` is the
    reducer's state after folding the (T0, T1] events into the T0
    snapshot. ``divergence`` lists the keys where ``hydrated_state``
    and ``snapshot_t1`` disagree; an empty list means the property
    held.
    """

    snapshot_t0: dict[str, Any]
    snapshot_t1: dict[str, Any]
    hydrated_state: dict[str, Any]
    divergence: list[str]

    @property
    def holds(self) -> bool:
        """True when the hydrated state matches the fresh T1 snapshot."""
        return not self.divergence


def replay_segment(
    scenario_dir: Path,
    segment_name: str,
    chatlog_path: Path,
    *,
    drain_seconds: float = 0.6,
) -> None:
    """Stream one segment file into the watcher's chatlog and drain.

    The segment file lives at ``scenario_dir / segment_name`` and is
    appended line-by-line to ``chatlog_path`` so the running
    ``ChatlogWatcher`` reads each line through its real tail loop. A
    missing segment file is a contract error rather than a silent
    no-op: a consistency scenario authoring mistake should surface
    loudly, not skip events. After every write completes, ``drain``
    sleeps long enough for the tail loop to read the last line and
    flush its idle tick (default 0.6s, matching ``replay.wait_for_drain``).
    """
    source = scenario_dir / segment_name
    if not source.exists():
        raise FileNotFoundError(
            f"Consistency scenario {scenario_dir.name!r} is missing "
            f"segment {segment_name!r}; both chat_replay.log and "
            "chat_replay_after.log are required for the two-segment "
            "midpoint convention."
        )
    lines = source.read_text(encoding="utf-8").splitlines(keepends=True)
    with chatlog_path.open("a", encoding="utf-8") as sink:
        for line in lines:
            sink.write(line)
            sink.flush()
    wait_for_drain(drain_seconds)


def _diff_state(
    expected: dict[str, Any],
    actual: dict[str, Any],
    *,
    float_tolerance: float = 1e-6,
) -> list[str]:
    """Return the keys where ``expected`` and ``actual`` disagree.

    Float comparison runs under a small absolute tolerance so a
    snapshot view that rounds to four decimal places and a reducer
    that accumulates to four decimal places do not surface
    last-bit drift as a divergence. Non-numeric keys compare under
    plain equality; unknown keys (present in one side but not the
    other) are also reported.
    """
    divergence: list[str] = []
    all_keys = set(expected) | set(actual)
    for key in sorted(all_keys):
        if key not in expected or key not in actual:
            divergence.append(key)
            continue
        lhs = expected[key]
        rhs = actual[key]
        if isinstance(lhs, float) or isinstance(rhs, float):
            try:
                if abs(float(lhs) - float(rhs)) > float_tolerance:
                    divergence.append(key)
            except (TypeError, ValueError):
                divergence.append(key)
            continue
        if lhs != rhs:
            divergence.append(key)
    return divergence


class ConsistencyHarness:
    """Drive the snapshot ↔ event-stream consistency property.

    One harness instance per scenario. Wires onto the same bus the
    e2e pipeline runs on (so reducer subscriptions arrive on the same
    publisher path the production tracker sees) and shares a chatlog
    path with the watcher so the two-segment replay flows through the
    pipeline's normal tail loop. The harness is otherwise stateless;
    re-running ``run`` on the same instance for a different scenario
    works as long as the caller resets the underlying backend
    (fresh ``EventBus``, fresh ``HuntTracker``, fresh chatlog file)
    between runs, which the per-test fixtures already do.
    """

    def __init__(
        self,
        bus: EventBus,
        chatlog_path: Path,
        *,
        drain_seconds: float = 0.6,
    ) -> None:
        """Bind the harness to the e2e pipeline's bus and chatlog.

        ``drain_seconds`` is forwarded to every ``replay_segment``
        call so a slower CI runner can lengthen the drain without
        touching every scenario test.
        """
        self._bus = bus
        self._chatlog_path = chatlog_path
        self._drain_seconds = drain_seconds

    def run(
        self,
        scenario_dir: Path,
        adapter: SurfaceAdapter,
        view_context: Any,
    ) -> ConsistencyResult:
        """Drive the full consistency lifecycle for one surface.

        ``view_context`` is whatever object the surface's ``view_fn``
        needs to compose its snapshot (typically a services container
        or a SQLite connection). The harness:

        1. Streams ``chat_replay.log`` (pre-midpoint) into the watcher,
           drains the bus.
        2. Captures ``snapshot_t0`` via the adapter's view function.
        3. Builds a fresh reducer, installs it on the bus, then hydrates
           it with ``snapshot_t0``. Install-before-hydrate is the only
           safe ordering: a subscription added after the post-midpoint
           replay starts could miss the first event, and hydrate
           replacing state after install discards anything the
           subscription captured between the two calls.
        4. Streams ``chat_replay_after.log`` (post-midpoint), drains.
        5. Captures ``snapshot_t1`` via the same view function.
        6. Returns a ``ConsistencyResult`` carrying both snapshots, the
           hydrated reducer state, and the divergence list.

        The harness does not assert on divergence: that lets the
        calling test phrase its assertion (and goldens via
        pytest-regressions' ``data_regression``) however the surface
        scenario warrants. ``ConsistencyResult.holds`` is the
        convenience boolean.
        """
        replay_segment(
            scenario_dir,
            "chat_replay.log",
            self._chatlog_path,
            drain_seconds=self._drain_seconds,
        )

        # Defensively copy the captured snapshots and the reducer's
        # hydrated state so a downstream comparison or assertion is not
        # aliased onto live dicts the reducer holds an internal
        # reference to. Without the copies, a later fold on the reducer
        # could mutate the T0 snapshot through the same dict identity.
        snapshot_t0 = dict(adapter.view_fn(view_context))
        reducer = adapter.reducer_factory()
        reducer.install(self._bus)
        reducer.hydrate(dict(snapshot_t0))

        replay_segment(
            scenario_dir,
            "chat_replay_after.log",
            self._chatlog_path,
            drain_seconds=self._drain_seconds,
        )

        snapshot_t1 = dict(adapter.view_fn(view_context))
        hydrated = dict(reducer.state)
        divergence = _diff_state(snapshot_t1, hydrated)

        return ConsistencyResult(
            snapshot_t0=snapshot_t0,
            snapshot_t1=snapshot_t1,
            hydrated_state=hydrated,
            divergence=divergence,
        )
