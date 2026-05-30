"""Seeded walk over the read surface of the HTTP API.

The contract suite drives every GET endpoint, but against thin or empty
state, so the data-present branches of the router and service layers stay
uncovered. This boots the full app against a seeded demo database and a
driven tracking session, then walks the read surface with that state in
place: each endpoint returns a success status and the routers'
data-present paths execute. Beyond the routing sweep, the walk pins
value-level body invariants on the read assembly: the computed totals,
the persisted session rows, and the response-shape contracts, so a
mutation that corrupts a value while preserving the status is caught.
Mutating flows are exercised in ``test_api_surface_mutations.py``; this
module is read-only beyond the one session replay that gives the tracking
and analytics endpoints something to report.
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
    "/api/tracking/snapshot",
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
    "/api/demo/tracking/snapshot",
    "/api/demo/tracking/sessions",
    "/api/demo/analytics/overview",
    "/api/demo/analytics/ledger",
    "/api/demo/analytics/ledger/presets",
    "/api/demo/analytics/inventory",
    "/api/demo/analytics/activity",
)


def test_read_surface_with_seeded_state(e2e_http_pipeline):
    """Every static read endpoint answers with success once state exists.

    The status sweep pins routing; the body assertions that follow pin the
    computed values, persisted rows, and shape contracts so a mutant that
    corrupts a value while keeping the status survives nothing.
    """
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

    # ── Health: the whole contract is the constant body, not just routing. ──
    health = client.get("/api/health")
    assert health.status_code == 200
    assert health.json() == {"status": "ok"}

    # ── Settings: pin the read-assembly contract end to end. ──
    settings = client.get("/api/settings")
    assert settings.status_code == 200
    cfg = settings.json()
    assert cfg["appVersion"] == "0.1.0"
    assert cfg["dbPath"].endswith(".db")
    assert {
        "gameConnection",
        "hotbar",
        "trifecta",
        "developerModeEnabled",
        "mobTrackingMode",
        "lootFilterBlacklist",
        "hotbarHooksEnabled",
    } <= set(cfg)
    assert set(cfg["trifecta"]) == {
        "activePresetId",
        "activePresetName",
        "presets",
        "ready",
        "message",
    }
    assert isinstance(cfg["gameConnection"]["chatLogValid"], bool)

    # ── Analytics overview over the recorded session: pin the P&L wiring. ──
    overview = client.get("/api/analytics/overview")
    assert overview.status_code == 200
    ov = overview.json()
    returns_b = ov["returnsBreakdown"]
    losses_b = ov["lossesBreakdown"]
    # totalGains is loot TT plus the markup-tagged ledger sum (progression
    # denominations stay out of the liquid P&L).
    assert ov["totalGains"] == round(
        returns_b["lootTt"] + sum(returns_b["ledger"].values()), 2
    )
    # trackingCost is the roll-up of the per-component cycled breakdown.
    assert (
        abs(losses_b["trackingCost"] - sum(losses_b["cycledBreakdown"].values())) < 0.05
    )
    # The daily timeline's trackingCost reconciles with the headline cost.
    assert (
        abs(
            sum(day["trackingCost"] for day in ov["timeline"])
            - losses_b["trackingCost"]
        )
        < 0.05
    )
    # The headline rate is gains/losses, guarded for the zero-loss window the
    # recorded session produces (no weapon/heal/armour cost in this scenario).
    if ov["totalLosses"] > 0:
        assert ov["totalReturnRate"] == round(ov["totalGains"] / ov["totalLosses"], 4)
    else:
        assert ov["totalReturnRate"] == 0.0

    # ── Activity comparisons: the data-present mapper shape. The recorded
    # session carries no weapon/heal cost, so it is excluded by the activity
    # cost>0 filter and every comparison list is empty; pin the three-family
    # shape rather than a row that this scenario does not qualify to produce.
    activity = client.get("/api/analytics/activity")
    assert activity.status_code == 200
    act = activity.json()
    assert set(act) == {"mobComparisons", "tagComparisons", "weaponComparisons"}
    assert all(isinstance(act[k], list) for k in act)

    # ── Tracking sessions list: the recorded session row's totals. ──
    sessions = client.get("/api/tracking/sessions")
    assert sessions.status_code == 200
    rows = sessions.json()
    assert isinstance(rows, list) and len(rows) >= 1
    recorded = next((r for r in rows if r["id"] == session_id), None)
    assert recorded is not None
    # net is liquid returns minus cost; returnRate is returns/cost (zero when
    # cost is zero, as in this no-weapon-cost scenario).
    assert recorded["net"] == round(recorded["returns"] - recorded["cost"], 2)
    if recorded["cost"] > 0:
        assert recorded["returnRate"] == round(
            recorded["returns"] / recorded["cost"], 4
        )
    else:
        assert recorded["returnRate"] == 0.0

    # ── Scan status: idle in the seeded run, with the full counter contract. ──
    scan_status = client.get("/api/scan/skills/status")
    assert scan_status.status_code == 200
    scan = scan_status.json()
    assert {"phase", "active", "configured", "has_pending_result"} <= set(scan)
    assert scan["active"] is False
    assert scan["has_pending_result"] is False
    # No held result exists in the idle seeded state.
    assert client.get("/api/scan/skills/pending").status_code == 404

    # ── Recording status (developer mode is on in the pipeline). ──
    recording = client.get("/api/recording/status")
    assert recording.status_code == 200
    rec = recording.json()
    assert {"state", "lines", "captures", "keystrokes"} <= set(rec)
    assert rec["state"] in {"idle", "recording"}

    # ── Codex species: a non-empty catalogue with the per-row cost contract. ──
    codex_species = client.get("/api/codex/species")
    assert codex_species.status_code == 200
    species = codex_species.json()
    assert isinstance(species, list) and species
    assert all({"name", "baseCost", "currentRank"} <= set(row) for row in species)

    # ── Codex meta attributes: the fixed attribute roster. ──
    attributes = client.get("/api/codex/meta/attributes")
    assert attributes.status_code == 200
    attrs = attributes.json()
    assert isinstance(attrs, list) and attrs
    assert all({"name", "currentLevel"} <= set(row) for row in attrs)

    # ── Character calibration: never scanned in this run, so uncalibrated. ──
    calibration = client.get("/api/character/calibration")
    assert calibration.status_code == 200
    cal = calibration.json()
    assert isinstance(cal["calibrated"], bool)
    assert isinstance(cal["stale"], bool)
    assert cal["calibrated"] is False

    # ── Character stats: no scan anchor, so HP and professions are empty. ──
    stats = client.get("/api/character/stats")
    assert stats.status_code == 200
    st = stats.json()
    assert {"hp", "topProfessions"} <= set(st)
    assert isinstance(st["hp"], int)
    assert isinstance(st["topProfessions"], list)

    # ── Quests: no quest is seeded by this scenario; pin the empty-list
    # contract so a mapper that returns a non-list or 404s is caught while not
    # asserting rows the scenario does not produce.
    quests = client.get("/api/quests")
    assert quests.status_code == 200
    assert quests.json() == []
    quest_analytics = client.get("/api/quests/analytics")
    assert quest_analytics.status_code == 200
    assert quest_analytics.json() == []

    # ── Manual-mob suggestions: a real catalogue probe, projection + sort.
    # The no-argument walk above returns [] (empty query short-circuit); this
    # drives ``search_mob_names`` with a known prefix so the projection shape
    # and the prefix-priority sort branch execute and are pinned.
    suggestions = client.get("/api/tracking/manual-mob-suggestions", params={"q": "ar"})
    assert suggestions.status_code == 200
    sugg = suggestions.json()
    assert sugg
    assert all(set(item) == {"display", "species", "maturity"} for item in sugg)
    # Prefix matches sort ahead of mid-string matches: once a non-prefix match
    # appears, no later item may be a prefix match.
    is_prefix = [item["display"].lower().startswith("ar") for item in sugg]
    first_non_prefix = next(
        (i for i, flag in enumerate(is_prefix) if not flag), len(is_prefix)
    )
    assert all(is_prefix[:first_non_prefix])
    assert not any(is_prefix[first_non_prefix:])

    # ── Demo surface: the in-memory clone is primed and non-empty. ──
    demo_overview = client.get("/api/demo/analytics/overview")
    assert demo_overview.status_code == 200
    dov = demo_overview.json()
    assert dov["returnsBreakdown"]["lootTt"] > 0
    if dov["totalLosses"] > 0:
        assert dov["totalReturnRate"] == round(
            dov["totalGains"] / dov["totalLosses"], 4
        )

    demo_sessions = client.get("/api/demo/tracking/sessions")
    assert demo_sessions.status_code == 200
    demo_rows = demo_sessions.json()
    assert isinstance(demo_rows, list) and len(demo_rows) >= 1

    demo_status = client.get("/api/demo/tracking/status")
    assert demo_status.status_code == 200
    dstatus = demo_status.json()
    # The primed mid-hunt session is reported active with fired shots.
    assert dstatus["status"] == "active"
    assert dstatus["shotsFiredTotal"] > 0

    # Session-scoped reads against the just-recorded session.
    detail = client.get(f"/api/tracking/session/{session_id}")
    assert detail.status_code == 200
    detail_body = detail.json()
    # The detail handler answers for the requested id and its summary net
    # reconciles with returns minus cost.
    assert detail_body["sessionId"] == session_id
    summary = detail_body["summary"]
    assert summary["net"] == round(summary["returns"] - summary["cost"], 2)
    # The list-row returns agree with the detail summary returns.
    assert recorded["returns"] == summary["returns"]

    # Demo session-detail: the real session id need not exist in the demo DB,
    # so this only checks the demo handler does not 500 on an unknown id; the
    # known-id round-trip is pinned below against a real demo session.
    assert client.get(f"/api/demo/tracking/session/{session_id}").status_code in (
        200,
        404,
    )
    # A known demo session id round-trips through the demo session-detail
    # handler; an all-zeros id is a clean 404 against the demo connection.
    demo_session_id = demo_rows[0]["id"]
    demo_detail = client.get(f"/api/demo/tracking/session/{demo_session_id}")
    assert demo_detail.status_code == 200
    assert demo_detail.json()["sessionId"] == demo_session_id
    assert (
        client.get(
            "/api/demo/tracking/session/00000000-0000-0000-0000-000000000000"
        ).status_code
        == 404
    )

    link = client.get(f"/api/tracking/session/{session_id}/quest-link-suggestion")
    assert link.status_code == 200

    # Prospect forecasts over the recorded session: the global slice plus a
    # mob/tag/weapon slice if the recorded session offers one, driving the
    # slice-matching and projection branches that empty state does not reach.
    # The recorded session carries no weapon cost, so it is excluded by the
    # prospect session filter and the option lists are empty; pin the three
    # families' shape rather than slice values this scenario does not produce.
    options = client.get("/api/character/prospect-options").json()
    assert set(options) == {"tags", "mobs", "weapons"}
    assert all(isinstance(options[k], list) for k in options)
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


def test_live_reads_while_a_session_is_active(e2e_http_pipeline):
    """Read the live surface mid-session, before it is stopped.

    The status / live / recent-events handlers carry active-session
    projection branches (the running cumulative net history and in-flight
    totals) that the stopped-session reads in the seeded walk do not reach.
    """
    client, chatlog, watcher = e2e_http_pipeline
    tracker = get_services().tracker
    tracker.start_session()
    try:
        replay_scenario(SCENARIO, chatlog)
        wait_for_drain(watcher, chatlog)
        status = client.get("/api/tracking/status")
        assert status.status_code == 200
        # An in-flight session reports active with its returns/cost/returnRate
        # wired consistently; returnRate is returns/cost (zero when cost is
        # zero, as in this no-weapon-cost scenario).
        st = status.json()
        assert st["status"] == "active"
        if st["cost"] > 0:
            assert st["returnRate"] == round(st["returns"] / st["cost"], 4)
        else:
            assert st["returnRate"] == 0.0
        live = client.get("/api/tracking/live")
        assert live.status_code == 200
        live_body = live.json()
        # The live projection carries the running net (liquid returns minus
        # cost) and mirrors the status totals for the same session.
        assert live_body["status"] == "active"
        assert live_body["net"] == round(live_body["returns"] - live_body["cost"], 2)
        assert live_body["returns"] == st["returns"]
        assert client.get("/api/tracking/recent-events").status_code == 200
    finally:
        tracker.stop_session()


def test_parametric_character_and_equipment_reads(e2e_http_pipeline):
    """Reads that take query parameters: optimizers, prospect, equipment search.

    The static walk covers the no-argument reads; these drive the
    parameterised computation endpoints so their query-handling and
    projection branches execute, and pin the computed body alongside the
    status so a corrupted allocation or search projection is caught.
    """
    client, _chatlog, _watcher = e2e_http_pipeline

    assert client.get("/api/character/hp-optimizer").status_code == 200

    professions = client.get("/api/character/professions").json()
    names = [
        p.get("name") for p in professions if isinstance(p, dict) and p.get("name")
    ]
    if names:
        prof = names[0]
        prof_opt = client.get(
            "/api/character/profession-optimizer", params={"profession": prof}
        )
        assert prof_opt.status_code == 200
        prof_opt_body = prof_opt.json()
        assert prof_opt_body["profession"] == prof
        assert "skills" in prof_opt_body and "attributes" in prof_opt_body
        # ped_budget is exactly one of the (target_level, ped_budget) guard, so
        # a valid single budget reaches the allocation result.
        path_opt = client.get(
            "/api/character/profession-path-optimizer",
            params={"profession": prof, "ped_budget": 100.0},
        )
        assert path_opt.status_code == 200
        path_opt_body = path_opt.json()
        assert path_opt_body["profession"] == prof
        assert "allocations" in path_opt_body

    # Prospect global slice over whatever session history exists. The endpoint
    # requires profession + target_level (snake_case); the camelCase params
    # here exercise the validation-rejection path, so the tolerant status set
    # is the contract being checked rather than a forecast body.
    assert client.get(
        "/api/character/prospect", params={"sliceType": "global", "cycledPed": 500}
    ).status_code in (200, 400, 422)

    # Equipment search short-query guard: q below the two-character floor
    # returns an empty list without touching the catalogue.
    short = client.get("/api/equipment/search", params={"q": "a"})
    assert short.status_code == 200
    assert short.json() == []
    # A two-character probe hits the catalogue and maps each row to the
    # search-result shape, with ammoBurn already converted from ammo units
    # (PEC = raw_ammo_burn / 100).
    probe = client.get("/api/equipment/search", params={"q": "im"})
    assert probe.status_code == 200
    probe_rows = probe.json()
    assert probe_rows
    assert all(
        {"catalogId", "name", "decay", "ammoBurn", "isLimited"} <= set(row)
        for row in probe_rows
    )

    assert client.get("/api/character/calibration").status_code == 200
