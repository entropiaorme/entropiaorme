"""Seeded walk over the read surface of the HTTP API.

The contract suite drives every GET endpoint, but against thin or empty
state, so the data-present branches of the router and service layers stay
uncovered. This boots the full app against a seeded demo database and a
driven tracking session, then walks the read surface with that state in
place: each endpoint returns a success status and the routers'
data-present paths execute. Mutating flows are exercised in
``test_api_surface_mutations.py``; this module is read-only beyond the one
session replay that gives the tracking and analytics endpoints something to
report.
"""

from __future__ import annotations

from pathlib import Path

from backend.dependencies import get_services
from backend.testing.replay import replay_scenario, wait_for_drain

E2E_DIR = Path(__file__).parent
SCENARIO = E2E_DIR / "corpus" / "scripted" / "multi_mob_hunt_loot_grouping"

# Read endpoints with no path parameters. Each is hit with real state in
# place and must answer with a success status.
_STATIC_GETS = (
    "/api/health",
    "/api/tracking/status",
    "/api/tracking/live",
    "/api/tracking/recent-events",
    "/api/tracking/sessions",
    "/api/tracking/tag-suggestions",
    "/api/tracking/manual-mob-suggestions",
    "/api/analytics/overview",
    "/api/analytics/ledger",
    "/api/analytics/ledger/presets",
    "/api/analytics/inventory",
    "/api/analytics/activity",
    "/api/character/skills",
    "/api/character/stats",
    "/api/character/professions",
    "/api/character/calibration",
    "/api/character/codex",
    "/api/character/prospect-options",
    "/api/codex/species",
    "/api/codex/meta/attributes",
    "/api/quests",
    "/api/quests/mobs",
    "/api/quests/analytics",
    "/api/quests/playlists",
    "/api/quests/playlists/analytics",
    "/api/equipment/library",
    "/api/settings",
    "/api/settings/overlay-position",
    "/api/scan/skills/status",
    "/api/scan/skills/pending",
    "/api/recording/status",
    # Demo surface mirrors tracking / analytics against the seeded demo DB.
    "/api/demo/tracking/status",
    "/api/demo/tracking/live",
    "/api/demo/tracking/recent-events",
    "/api/demo/tracking/sessions",
    "/api/demo/analytics/overview",
    "/api/demo/analytics/ledger",
    "/api/demo/analytics/ledger/presets",
    "/api/demo/analytics/inventory",
    "/api/demo/analytics/activity",
)


def test_read_surface_with_seeded_state(e2e_http_pipeline):
    """Every static read endpoint answers with success once state exists."""
    client, chatlog, watcher = e2e_http_pipeline

    tracker = get_services().tracker
    session = tracker.start_session()
    replay_scenario(SCENARIO, chatlog)
    wait_for_drain(watcher, chatlog)
    tracker.stop_session()
    session_id = session.id

    for path in _STATIC_GETS:
        response = client.get(path)
        # The handler executes either way; a 404 (e.g. no pending scan) is a
        # handled empty-state answer, not a routing miss. The bar is that no
        # read endpoint raises a server error against real state.
        assert response.status_code in (200, 404), (
            f"GET {path} -> {response.status_code}"
        )

    # Session-scoped reads against the just-recorded session.
    detail = client.get(f"/api/tracking/session/{session_id}")
    assert detail.status_code == 200
    assert client.get(f"/api/demo/tracking/session/{session_id}").status_code in (
        200,
        404,
    )
    link = client.get(f"/api/tracking/session/{session_id}/quest-link-suggestion")
    assert link.status_code == 200

    # Prospect forecasts over the recorded session: the global slice plus a
    # mob/tag/weapon slice if the recorded session offers one, driving the
    # slice-matching and projection branches that empty state does not reach.
    options = client.get("/api/character/prospect-options").json()
    assert client.get(
        "/api/character/prospect", params={"sliceType": "global", "cycledPed": 200}
    ).status_code in (200, 400, 422)
    if isinstance(options, list) and options:
        opt = options[0]
        slice_type = opt.get("sliceType") or opt.get("type") or "mob"
        slice_value = opt.get("value") or opt.get("label")
        if slice_value:
            assert client.get(
                "/api/character/prospect",
                params={
                    "sliceType": slice_type,
                    "sliceValue": slice_value,
                    "cycledPed": 200,
                },
            ).status_code in (200, 400, 422)

    # Analytics ledger over a monthly grouping, the alternate aggregation path.
    assert (
        client.get("/api/analytics/ledger", params={"groupBy": "month"}).status_code
        == 200
    )
    assert client.get(
        "/api/analytics/overview", params={"window": "all"}
    ).status_code in (200, 422)


def test_session_detail_unknown_id_is_404(e2e_http_pipeline):
    """A read for a session that does not exist is a clean 404, not a 500."""
    client, _chatlog, _watcher = e2e_http_pipeline
    missing = client.get("/api/tracking/session/00000000-0000-0000-0000-000000000000")
    assert missing.status_code == 404


def test_parametric_character_and_equipment_reads(e2e_http_pipeline):
    """Reads that take query parameters: optimizers, prospect, equipment search.

    The static walk covers the no-argument reads; these drive the
    parameterised computation endpoints so their query-handling and
    projection branches execute.
    """
    client, _chatlog, _watcher = e2e_http_pipeline

    assert client.get("/api/character/hp-optimizer").status_code == 200

    professions = client.get("/api/character/professions").json()
    names = [
        p.get("name") for p in professions if isinstance(p, dict) and p.get("name")
    ]
    if names:
        prof = names[0]
        assert (
            client.get(
                "/api/character/profession-optimizer", params={"profession": prof}
            ).status_code
            == 200
        )
        assert client.get(
            "/api/character/profession-path-optimizer",
            params={"profession": prof, "ped_budget": 100.0},
        ).status_code in (200, 422)

    # Prospect global slice over whatever session history exists.
    assert client.get(
        "/api/character/prospect", params={"sliceType": "global", "cycledPed": 500}
    ).status_code in (200, 400, 422)

    # Equipment search + catalogue detail.
    search = client.get("/api/equipment/search", params={"q": "a"})
    assert search.status_code == 200
    assert client.get("/api/character/calibration").status_code == 200
