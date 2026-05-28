"""HTTP-fingerprint contract over the scripted scenarios.

For each scenario in the curated set, the test boots the full FastAPI
lifespan against a temp data dir + chatlog, drives the in-lifespan
``HuntTracker`` through ``tracker.start_session`` and the scenario's
chat replay, then captures the curated hydration-endpoint set against
per-scenario goldens under ``<scenario>/expected/http_responses/``.

The curated set covers each of the four hydration prefixes
(``/api/tracking``, ``/api/scan``, ``/api/quests``, ``/api/codex``) so
a regression in any of them surfaces against the same scenarios that
already pin the bus/event-stream contract. ETag-shape and
Cache-Control header projection assert the substrate is engaged on
every captured response.

Authoring note: two-segment consistency scenarios (``chat_replay.log``
+ ``chat_replay_after.log``) are replayed in full before fingerprints
are captured. The fingerprints pin the end-of-scenario hydration
state; the per-surface midpoint property is the consistency suite's
job, not this contract's.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from backend.dependencies import get_services
from backend.testing.http_fingerprint import HttpFingerprinter
from backend.testing.replay import wait_for_drain

# Scenarios in scope for the HTTP-fingerprint contract. Player_name-
# dependent scenarios (e.g. global_kill_correlated, hof_item_drop) are
# excluded because the lifespan-built tracker reads player_name from
# config rather than per-call; including them would need a settings.json
# variant per scenario.
HTTP_FINGERPRINT_SCENARIOS: tuple[str, ...] = (
    "basic_hunt_10_events",
    "multi_mob_hunt_loot_grouping",
    "mission_completion_with_reward_suppression",
    "consistency_tracking_hunt_midpoint",
    "consistency_quests_mission_lifecycle_midpoint",
    "consistency_scan_isolation_midpoint",
    "consistency_codex_isolation_midpoint",
)


def _stream_segment(source: Path, destination: Path) -> None:
    """Append ``source`` to ``destination`` line-by-line, flushing each line.

    Mirrors ``backend.testing.replay.replay_scenario`` minus its DSL-
    layer concerns; the per-line flush is what lets the chatlog
    watcher's tail loop see each event individually as it arrives.
    """
    lines = source.read_text(encoding="utf-8").splitlines(keepends=True)
    with destination.open("a", encoding="utf-8") as sink:
        for line in lines:
            sink.write(line)
            sink.flush()


def _replay_full_scenario(scenario_dir: Path, chatlog_path: Path) -> None:
    """Stream every chat segment (one or two) for ``scenario_dir``."""
    primary = scenario_dir / "chat_replay.log"
    if not primary.exists():
        raise FileNotFoundError(
            f"Scenario {scenario_dir.name!r} is missing chat_replay.log"
        )
    _stream_segment(primary, chatlog_path)
    wait_for_drain()

    secondary = scenario_dir / "chat_replay_after.log"
    if secondary.exists():
        _stream_segment(secondary, chatlog_path)
        wait_for_drain()


def _capture_hydration_set(
    fp: HttpFingerprinter,
    client,
    session_id: str,
) -> None:
    """Capture the curated hydration GET surface for the scenario.

    Endpoint order is fixed so the shared Normalizer's symbol table
    grows in a deterministic sequence across runs of the same scenario.
    """
    captures: tuple[tuple[str, str, str, dict | None], ...] = (
        ("GET_tracking_status", "GET", "/api/tracking/status", None),
        ("GET_tracking_live", "GET", "/api/tracking/live", None),
        ("GET_tracking_recent_events", "GET", "/api/tracking/recent-events", None),
        ("GET_tracking_sessions", "GET", "/api/tracking/sessions", None),
        (
            "GET_tracking_session_detail",
            "GET",
            f"/api/tracking/session/{session_id}",
            None,
        ),
        (
            "GET_tracking_session_quest_link_suggestion",
            "GET",
            f"/api/tracking/session/{session_id}/quest-link-suggestion",
            None,
        ),
        ("GET_quests", "GET", "/api/quests", None),
        ("GET_quests_mobs", "GET", "/api/quests/mobs", None),
        ("GET_quests_analytics", "GET", "/api/quests/analytics", None),
        ("GET_quests_playlists", "GET", "/api/quests/playlists", None),
        ("GET_scan_skills_status", "GET", "/api/scan/skills/status", None),
        ("GET_codex_meta_attributes", "GET", "/api/codex/meta/attributes", None),
    )

    for endpoint_id, method, path, query in captures:
        response = client.get(path)
        assert response.status_code == 200, (
            f"{endpoint_id} ({method} {path}) returned "
            f"{response.status_code}: {response.text!r}"
        )
        fp.capture(
            response,
            endpoint_id=endpoint_id,
            request_method=method,
            request_path=path,
            request_query=query,
        )


@pytest.mark.parametrize("scenario_name", HTTP_FINGERPRINT_SCENARIOS)
def test_http_fingerprint(
    e2e_http_pipeline,
    corpus_root: Path,
    http_fingerprinter,
    scenario_name: str,
) -> None:
    """For each scenario: replay, then capture the hydration goldens.

    The session lifecycle is driven through the production
    ``HuntTracker`` (not through ``POST /api/tracking/start``) so the
    test does not need to pre-seed a trifecta config to clear that
    endpoint's start-time attribution gate. The HTTP contract under
    test is on the read surface, which the tracker shape exercises
    identically.
    """
    client, chatlog = e2e_http_pipeline
    scenario = corpus_root / "scripted" / scenario_name

    tracker = get_services().tracker
    session = tracker.start_session()
    try:
        _replay_full_scenario(scenario, chatlog)
        tracker.stop_session()
    finally:
        if tracker.is_tracking:
            tracker.stop_session()

    fp = http_fingerprinter(scenario)
    _capture_hydration_set(fp, client, session.id)

    # Pin the captured-set cardinality so a future test refactor that
    # silently drops endpoints from _capture_hydration_set surfaces here
    # rather than producing a silently-shrunk golden set.
    assert len(fp.captured_endpoint_ids) == 12, (
        f"Expected 12 captured endpoints, got "
        f"{len(fp.captured_endpoint_ids)}: {fp.captured_endpoint_ids}"
    )
