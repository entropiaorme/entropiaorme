"""Walk over the write surface of the HTTP API.

The contract suite is GET-only, so the routers' mutation handlers (create
/ update / delete and their validation branches) are otherwise unexercised
end to end. Each test boots the full app and drives a create-read-mutate
-delete lifecycle through the live HTTP surface, so the adapter paths, the
service calls behind them, and the obvious validation errors all execute
against a real database.
"""

from __future__ import annotations

from pathlib import Path

from backend.dependencies import get_services
from backend.testing.replay import replay_scenario, wait_for_drain

E2E_DIR = Path(__file__).parent
SCENARIO = E2E_DIR / "corpus" / "scripted" / "multi_mob_hunt_loot_grouping"


def test_settings_mutations(e2e_http_pipeline):
    """PATCH / PUT / reset settings, including the validation branches."""
    client, _chatlog, _watcher = e2e_http_pipeline
    # State-changing methods require an allowed Origin (the app's origin guard);
    # the Tauri webview always sends one, so mirror it here.
    client.headers["Origin"] = "tauri://localhost"

    # No-op patch is rejected.
    assert client.patch("/api/settings", json={}).status_code == 400

    # A field-level update round-trips.
    patched = client.patch(
        "/api/settings",
        json={
            "player_name": "Walker",
            "hotbar_hooks_enabled": True,
            "repair_ocr_enabled": True,
            "mob_tracking_mode": "tag",
            "mob_tracking_tag": "Atrox",
            "loot_filter_blacklist": ["Shrapnel"],
        },
    )
    assert patched.status_code == 200
    assert patched.json()["gameConnection"]["playerName"] == "Walker"

    # Unknown tracking mode is a 400.
    assert (
        client.patch(
            "/api/settings", json={"mob_tracking_mode": "nonsense"}
        ).status_code
        == 400
    )
    # A chat.log path that does not resolve is a 400.
    assert (
        client.patch(
            "/api/settings", json={"chatlog_path": "/nope/not-a-chat.log"}
        ).status_code
        == 400
    )

    # Overlay position round-trips.
    assert (
        client.put(
            "/api/settings/overlay-position", json={"x": 12, "y": 34}
        ).status_code
        == 200
    )
    pos = client.get("/api/settings/overlay-position")
    assert pos.json() == {"x": 12, "y": 34}

    # Reset returns the rebuilt settings shape.
    assert client.post("/api/settings/reset").status_code == 200


def test_quest_and_playlist_lifecycle(e2e_http_pipeline):
    """Create, read, update, run, and delete a quest and a playlist."""
    client, _chatlog, _watcher = e2e_http_pipeline
    # State-changing methods require an allowed Origin (the app's origin guard);
    # the Tauri webview always sends one, so mirror it here.
    client.headers["Origin"] = "tauri://localhost"

    created = client.post(
        "/api/quests",
        json={"name": "Walker Hunt", "mobs": ["Atrox"], "reward_ped": 1.5},
    )
    assert created.status_code == 200
    quest_id = created.json()["id"]

    assert client.get(f"/api/quests/{quest_id}").status_code == 200
    assert client.get("/api/quests/99999").status_code == 404

    assert (
        client.put(
            f"/api/quests/{quest_id}", json={"notes": "edited", "cooldown_hours": 24}
        ).status_code
        == 200
    )

    assert client.post(f"/api/quests/{quest_id}/start").status_code == 200
    assert client.post(f"/api/quests/{quest_id}/complete").status_code == 200
    assert (
        client.post(
            f"/api/quests/{quest_id}/cancel", json={"undo_reward": True}
        ).status_code
        == 200
    )

    playlist = client.post(
        "/api/quests/playlists",
        json={"name": "Walker Playlist", "quest_ids": [quest_id]},
    )
    assert playlist.status_code == 200
    playlist_id = playlist.json()["id"]
    assert (
        client.put(
            f"/api/quests/playlists/{playlist_id}", json={"estimated_minutes": 45}
        ).status_code
        == 200
    )
    assert client.get("/api/quests/playlists/analytics").status_code == 200
    assert client.delete(f"/api/quests/playlists/{playlist_id}").status_code in (
        200,
        204,
    )
    assert client.delete(f"/api/quests/{quest_id}").status_code in (200, 204)


def test_skill_scan_lifecycle_endpoints(e2e_http_pipeline):
    """Drive the manual skill-scan control endpoints.

    The scan handlers are thin adapters over the scan service; calling them
    (without a real capture device) exercises the router endpoints and the
    service's no-active-scan handling. The capture endpoint itself is skipped
    here: it grabs a screen region and is covered by the OCR equivalence pair.
    """
    client, _chatlog, _watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"

    assert client.post("/api/scan/skills/start").status_code < 500
    assert client.get("/api/scan/skills/status").status_code == 200
    assert client.post("/api/scan/skills/undo").status_code < 500
    assert client.post("/api/scan/skills/process").status_code < 500
    assert client.post("/api/scan/skills/accept").status_code < 500
    assert client.post("/api/scan/skills/reject").status_code < 500
    assert client.post("/api/scan/skills/cancel").status_code < 500
    # The capture PNG for a page that was never captured is a clean 404.
    assert client.get("/api/scan/skills/capture/0").status_code == 404
    # Spacebar-capture toggle (a query-param flag).
    assert (
        client.post("/api/scan/spacebar-capture", params={"enabled": True}).status_code
        == 200
    )


def test_quest_cooldown_and_chain(e2e_http_pipeline):
    """Drive the cooldown and chain branches of the quest service.

    A quest with a cooldown that is completed and re-started exercises the
    cooldown gate, and a chained quest exercises the chain-position fields,
    neither of which the basic create-complete lifecycle reaches.
    """
    client, _chatlog, _watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"

    cooldown_quest = client.post(
        "/api/quests",
        json={"name": "Daily Sweat", "cooldown_hours": 24, "reward_ped": 0.5},
    )
    assert cooldown_quest.status_code == 200
    qid = cooldown_quest.json()["id"]
    assert client.post(f"/api/quests/{qid}/start").status_code == 200
    assert client.post(f"/api/quests/{qid}/complete").status_code == 200
    # A completed cooldown quest re-read reflects its cooldown state.
    assert client.get(f"/api/quests/{qid}").status_code == 200

    chained = client.post(
        "/api/quests",
        json={
            "name": "Chain Step 1",
            "chain_name": "Iron Challenge",
            "chain_position": 1,
            "chain_total": 3,
        },
    )
    assert chained.status_code == 200
    # Analytics and mob views aggregate over the quests now present.
    assert client.get("/api/quests/analytics").status_code == 200
    assert client.get("/api/quests/mobs").status_code == 200
    assert client.delete(f"/api/quests/{qid}").status_code in (200, 204)
    assert client.delete(f"/api/quests/{chained.json()['id']}").status_code in (
        200,
        204,
    )


def test_equipment_library_crud(e2e_http_pipeline):
    """Add a custom consumable, read its detail, edit it, then remove it."""
    client, _chatlog, _watcher = e2e_http_pipeline
    # State-changing methods require an allowed Origin (the app's origin guard);
    # the Tauri webview always sends one, so mirror it here.
    client.headers["Origin"] = "tauri://localhost"

    added = client.post(
        "/api/equipment/library",
        json={"type": "consumable", "name": "Walker Token"},
    )
    assert added.status_code == 200
    item_id = added.json()["id"]

    assert client.get(f"/api/equipment/library/{item_id}/detail").status_code == 200
    assert (
        client.put(
            f"/api/equipment/library/{item_id}",
            json={"type": "consumable", "name": "Walker Token v2"},
        ).status_code
        == 200
    )
    assert client.delete(f"/api/equipment/library/{item_id}").status_code in (200, 204)


def _first_catalog_id(client, search_type: str) -> str | None:
    """Find a real catalogue id for a search type, or None if the bundled
    data has nothing matching the probe substrings."""
    for probe in ("er", "on", "in", "al", "ar"):
        results = client.get(
            "/api/equipment/search", params={"q": probe, "type": search_type}
        ).json()
        if results:
            return results[0]["catalogId"]
    return None


def test_equipment_weapon_and_cost_paths(e2e_http_pipeline):
    """Add a catalogue weapon and a healing tool, and price a weapon.

    Drives the catalogue-resolution branches of the library add / detail /
    cost-calculate handlers, which the custom-consumable path does not reach.
    """
    client, _chatlog, _watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"

    weapon_catalog_id = _first_catalog_id(client, "weapon")
    assert weapon_catalog_id is not None, (
        "bundled weapon catalogue is unexpectedly empty"
    )

    # Attach an amplifier if the catalogue has one, exercising the amp branch.
    amp_catalog_id = _first_catalog_id(client, "amp")
    weapon_body = {
        "type": "weapon",
        "catalog_id": weapon_catalog_id,
        "weapon_markup": 105,
        "damage_enhancers": 2,
    }
    if amp_catalog_id is not None:
        weapon_body["amp_catalog_id"] = amp_catalog_id
    added = client.post("/api/equipment/library", json=weapon_body)
    assert added.status_code == 200
    weapon_item_id = added.json()["id"]
    assert (
        client.get(f"/api/equipment/library/{weapon_item_id}/detail").status_code == 200
    )
    # Update the weapon in place (the PUT weapon-resolution branch).
    assert (
        client.put(
            f"/api/equipment/library/{weapon_item_id}",
            json={
                "type": "weapon",
                "catalog_id": weapon_catalog_id,
                "weapon_markup": 120,
            },
        ).status_code
        == 200
    )
    assert client.delete(f"/api/equipment/library/{weapon_item_id}").status_code in (
        200,
        204,
    )

    # Cost calculation against the same catalogue weapon.
    assert (
        client.post(
            "/api/equipment/cost/calculate",
            json={"catalog_id": weapon_catalog_id, "type": "weapon"},
        ).status_code
        == 200
    )

    heal_catalog_id = _first_catalog_id(client, "healer")
    if heal_catalog_id is not None:
        heal = client.post(
            "/api/equipment/library",
            json={"type": "healing", "catalog_id": heal_catalog_id},
        )
        assert heal.status_code == 200
        heal_id = heal.json()["id"]
        assert (
            client.put(
                f"/api/equipment/library/{heal_id}",
                json={
                    "type": "healing",
                    "catalog_id": heal_catalog_id,
                    "weapon_markup": 110,
                },
            ).status_code
            == 200
        )
        assert client.delete(f"/api/equipment/library/{heal_id}").status_code in (
            200,
            204,
        )


def test_analytics_ledger_and_inventory(e2e_http_pipeline):
    """Create and remove ledger entries, presets, and inventory items."""
    client, _chatlog, _watcher = e2e_http_pipeline
    # State-changing methods require an allowed Origin (the app's origin guard);
    # the Tauri webview always sends one, so mirror it here.
    client.headers["Origin"] = "tauri://localhost"

    entry = client.post(
        "/api/analytics/ledger",
        json={
            "date": "2026-05-01",
            "type": "expense",
            "description": "Ammo",
            "amount": 12.5,
            "tag": "ammo",
        },
    )
    assert entry.status_code == 200
    entry_id = entry.json()["id"]
    assert client.delete(f"/api/analytics/ledger/{entry_id}").status_code in (200, 204)

    preset = client.post(
        "/api/analytics/ledger/presets",
        json={
            "name": "Daily ammo",
            "type": "expense",
            "description": "Ammo",
            "amount": 10.0,
            "tag": "ammo",
        },
    )
    assert preset.status_code == 200
    preset_id = preset.json()["id"]
    assert client.delete(f"/api/analytics/ledger/presets/{preset_id}").status_code in (
        200,
        204,
    )

    item = client.post(
        "/api/analytics/inventory",
        json={"name": "Loot Stack", "tt_value": 5.0, "markup_paid": 1.0},
    )
    assert item.status_code == 200
    item_id = item.json()["id"]
    assert (
        client.patch(
            f"/api/analytics/inventory/{item_id}", json={"tt_value": 6.0}
        ).status_code
        == 200
    )
    assert client.post(
        f"/api/analytics/inventory/{item_id}/sell", json={"sale_price": 7.5}
    ).status_code in (200, 204)
    # The sell may have consumed the row; deletion is then a clean 404.
    assert client.delete(f"/api/analytics/inventory/{item_id}").status_code in (
        200,
        204,
        404,
    )


def test_codex_mutations(e2e_http_pipeline):
    """Drive calibrate / claim / meta-claim against a real species."""
    client, _chatlog, _watcher = e2e_http_pipeline
    # State-changing methods require an allowed Origin (the app's origin guard);
    # the Tauri webview always sends one, so mirror it here.
    client.headers["Origin"] = "tauri://localhost"

    species = client.get("/api/codex/species")
    assert species.status_code == 200
    names = [s.get("name") for s in species.json() if isinstance(s, dict)]
    if names:
        name = names[0]
        # Calibrate is a pure state write; claim needs a valid skill, so a
        # bad skill is allowed to surface as a handled 4xx rather than a 500.
        assert (
            client.post(
                "/api/codex/calibrate", json={"species_name": name, "rank": 1}
            ).status_code
            < 500
        )
        assert (
            client.post(
                "/api/codex/claim",
                json={"species_name": name, "rank": 1, "skill_name": "Courage"},
            ).status_code
            < 500
        )

    # The meta attribute claim drives the repeatable-reward path.
    assert (
        client.post(
            "/api/codex/meta/claim", json={"attribute_name": "Health"}
        ).status_code
        < 500
    )


def test_tracking_session_mutations(e2e_http_pipeline):
    """Drive the per-session edit endpoints against a recorded session.

    Exercises the rename / restore / armour-cost / loot-toggle / quest-link
    / repair-scan / delete surface, which runs the tracking router's session
    handlers and the tracker mutators behind them.
    """
    client, chatlog, watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"

    tracker = get_services().tracker
    session = tracker.start_session()
    replay_scenario(SCENARIO, chatlog)
    wait_for_drain(watcher, chatlog)
    tracker.stop_session()
    session_id = session.id

    detail = client.get(f"/api/tracking/session/{session_id}").json()
    mob_name = detail["mobBreakdown"][0]["currentName"]
    loot_name = next(
        (item["name"] for item in detail["lootBreakdown"] if " " not in item["name"]),
        "Shrapnel",
    )

    assert (
        client.post(
            f"/api/tracking/session/{session_id}/rename-mob",
            json={"fromMobName": mob_name, "toMobName": "Atrox Young"},
        ).status_code
        == 200
    )
    assert (
        client.post(
            f"/api/tracking/session/{session_id}/restore-mob",
            json={"currentMobName": "Atrox Young"},
        ).status_code
        == 200
    )
    assert (
        client.post(
            f"/api/tracking/session/{session_id}/armour-cost", json={"cost": 1.5}
        ).status_code
        == 200
    )

    assert (
        client.post(
            f"/api/tracking/session/{session_id}/loot-item/{loot_name}/deactivate"
        ).status_code
        == 200
    )
    assert (
        client.post(
            f"/api/tracking/session/{session_id}/loot-item/{loot_name}/activate"
        ).status_code
        == 200
    )

    # Valid body reaches the handler; the decision branch runs regardless of
    # whether a suggestion exists to dismiss.
    assert (
        client.post(
            f"/api/tracking/session/{session_id}/quest-link", json={"action": "dismiss"}
        ).status_code
        < 500
    )
    # Repair OCR is disabled by default, so this exercises the disabled guard
    # without touching a capture device.
    assert (
        client.post(f"/api/tracking/session/{session_id}/repair-scan").status_code
        == 400
    )

    assert client.delete(f"/api/tracking/session/{session_id}").status_code in (
        200,
        204,
    )


def test_tracking_live_mob_controls(e2e_http_pipeline):
    """Exercise the live mob-lock / tag-lock / release controls.

    These check tracking state internally, so a valid request body reaches
    the handler whether or not a session is live; the bar is that the
    handlers run without a server error.
    """
    client, _chatlog, _watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"

    assert (
        client.post("/api/tracking/tag-lock", json={"tag": "Atrox"}).status_code < 500
    )
    assert (
        client.post(
            "/api/tracking/manual-mob-lock",
            json={"species": "Atrox", "maturity": "Young"},
        ).status_code
        < 500
    )
    assert client.post("/api/tracking/release-mob").status_code < 500


def test_settings_change_during_active_session_reloads_tracker(e2e_http_pipeline):
    """A settings PATCH while a session is live re-applies tracker config.

    Drives ``HuntTracker.reload_config``'s active-session branches (the
    standard-mode attribution reset and the manual-mob refresh), which the
    no-session settings PATCH short-circuits past.
    """
    client, _chatlog, _watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"

    tracker = get_services().tracker
    tracker.start_session()
    try:
        # Standard (mob) mode reload: clears the attributor and weapon state.
        assert (
            client.patch(
                "/api/settings",
                json={"mob_tracking_mode": "mob", "hotbar_hooks_enabled": True},
            ).status_code
            == 200
        )
        # Switch to tag mode while live (the tag-mode early return in reload).
        assert (
            client.patch(
                "/api/settings",
                json={"mob_tracking_mode": "tag", "mob_tracking_tag": "Atrox"},
            ).status_code
            == 200
        )
    finally:
        tracker.stop_session()


def test_tracking_session_mutations_on_unknown_session_are_404(e2e_http_pipeline):
    """Every per-session edit rejects an unknown session id with a clean 404."""
    client, _chatlog, _watcher = e2e_http_pipeline
    client.headers["Origin"] = "tauri://localhost"
    missing = "00000000-0000-0000-0000-000000000000"
    base = f"/api/tracking/session/{missing}"

    assert client.get(base).status_code == 404
    assert client.get(f"{base}/quest-link-suggestion").status_code == 404
    assert client.delete(base).status_code == 404
    assert (
        client.post(
            f"{base}/rename-mob", json={"fromMobName": "A", "toMobName": "B"}
        ).status_code
        == 404
    )
    assert (
        client.post(f"{base}/restore-mob", json={"currentMobName": "B"}).status_code
        == 404
    )
    assert client.post(f"{base}/armour-cost", json={"cost": 1.0}).status_code == 404
    assert (
        client.post(f"{base}/quest-link", json={"action": "dismiss"}).status_code == 404
    )
    assert client.post(f"{base}/loot-item/Shrapnel/deactivate").status_code == 404
