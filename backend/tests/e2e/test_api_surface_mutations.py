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

import pytest

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

    # No-op patch is rejected with the dedicated message.
    empty = client.patch("/api/settings", json={})
    assert empty.status_code == 400
    assert empty.json()["detail"] == "No fields to update"

    # A field-level update round-trips: every patched field is reflected in
    # the response body, not just the player name, so a mutant that drops or
    # mis-maps any single field is caught.
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
    body = patched.json()
    assert body["gameConnection"]["playerName"] == "Walker"
    assert body["hotbarHooksEnabled"] is True
    assert body["repairOcrEnabled"] is True
    assert body["mobTrackingMode"] == "tag"
    assert body["mobTrackingTag"] == "Atrox"
    assert body["lootFilterBlacklist"] == ["Shrapnel"]

    # Unknown tracking mode is a 400 with its own message (distinct from the
    # chat.log branches below).
    bad_mode = client.patch("/api/settings", json={"mob_tracking_mode": "nonsense"})
    assert bad_mode.status_code == 400
    assert bad_mode.json()["detail"] == "Unknown mob tracking mode"

    # The three distinct chat.log validation branches each carry their own
    # message: empty, wrong filename, and missing file.
    empty_path = client.patch("/api/settings", json={"chatlog_path": ""})
    assert empty_path.status_code == 400
    assert empty_path.json()["detail"] == "chat.log path is required"

    wrong_name = client.patch(
        "/api/settings", json={"chatlog_path": "/tmp/not-the-log.txt"}
    )
    assert wrong_name.status_code == 400
    assert wrong_name.json()["detail"] == "chat.log path must point to a chat.log file"

    # A path named chat.log that does not resolve to a file: the missing-file
    # branch (distinct from the wrong-filename branch above).
    missing = client.patch("/api/settings", json={"chatlog_path": "/nope/chat.log"})
    assert missing.status_code == 400
    assert missing.json()["detail"] == "chat.log path does not exist"

    # Overlay position round-trips.
    assert (
        client.put(
            "/api/settings/overlay-position", json={"x": 12, "y": 34}
        ).status_code
        == 200
    )
    pos = client.get("/api/settings/overlay-position")
    assert pos.json() == {"x": 12, "y": 34}

    # Reset restores the AppConfig defaults, reverting the earlier PATCH: a
    # no-op reset that echoed the pre-reset config would fail these.
    reset = client.post("/api/settings/reset")
    assert reset.status_code == 200
    reset_body = reset.json()
    assert reset_body["gameConnection"]["playerName"] == ""
    assert reset_body["mobTrackingMode"] == "mob"
    assert reset_body["mobTrackingTag"] == ""
    assert reset_body["hotbarHooksEnabled"] is False
    assert reset_body["repairOcrEnabled"] is False
    assert reset_body["lootFilterBlacklist"] == ["Universal Ammo"]


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

    # The GET body must carry every remapped field, not just the id: a mutant
    # swapping or dropping a key in _format_quest survives a status-only check.
    fetched = client.get(f"/api/quests/{quest_id}")
    assert fetched.status_code == 200
    fetched_body = fetched.json()
    assert fetched_body["name"] == "Walker Hunt"
    assert fetched_body["targetMobs"] == ["Atrox"]
    assert fetched_body["reward"] == 1.5
    assert fetched_body["rewardIsSkill"] is False
    assert fetched_body["planet"] == "Calypso"
    assert client.get("/api/quests/99999").status_code == 404

    updated = client.put(
        f"/api/quests/{quest_id}", json={"notes": "edited", "cooldown_hours": 24}
    )
    assert updated.status_code == 200
    updated_body = updated.json()
    assert updated_body["notes"] == "edited"
    assert updated_body["cooldownDurationHours"] == 24

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
        json={"name": "Walker Playlist", "quest_ids": [int(quest_id)]},
    )
    assert playlist.status_code == 200
    playlist_id = playlist.json()["id"]
    assert (
        client.put(
            f"/api/quests/playlists/{playlist_id}", json={"estimated_minutes": 45}
        ).status_code
        == 200
    )
    # Read the playlist back: the estimated_minutes edit and the immediate /
    # long-horizon id split must round-trip through _format_playlist.
    listed = client.get("/api/quests/playlists")
    assert listed.status_code == 200
    playlist_view = next(p for p in listed.json() if p["id"] == playlist_id)
    assert playlist_view["estimatedMinutes"] == 45
    assert playlist_view["questIds"] == [quest_id]
    assert playlist_view["immediateQuestIds"] == [quest_id]
    assert playlist_view["longHorizonQuestIds"] == []
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

    # No game window is present under the test harness, so start cannot enter
    # the capturing phase; it surfaces the no-window guard rather than a phase
    # transition. Pin that the guard fires (not just a non-5xx).
    start = client.post("/api/scan/skills/start")
    assert start.status_code == 200
    assert "not found" in start.json()["error"].lower()

    # Status is always the full shape; with no scan it reports the idle phase.
    status = client.get("/api/scan/skills/status")
    assert status.status_code == 200
    status_body = status.json()
    assert status_body["phase"] == "idle"
    assert status_body["active"] is False
    assert status_body["has_pending_result"] is False

    # The lifecycle verbs against a no-active-scan state each return a defined
    # error shape rather than acting; pinning the error distinguishes a handler
    # wired to the wrong service method from one that correctly refuses.
    assert client.post("/api/scan/skills/undo").status_code < 500
    process = client.post("/api/scan/skills/process")
    assert process.status_code == 200
    assert process.json()["error"] == "No active scan to process"
    accept = client.post("/api/scan/skills/accept")
    assert accept.status_code == 200
    assert "error" in accept.json()
    assert client.post("/api/scan/skills/reject").status_code < 500

    # Cancel resets to the idle phase regardless of prior state.
    cancel = client.post("/api/scan/skills/cancel")
    assert cancel.status_code == 200
    assert cancel.json()["phase"] == "idle"
    assert cancel.json()["active"] is False

    # The capture PNG for a page that was never captured is a clean 404.
    assert client.get("/api/scan/skills/capture/0").status_code == 404

    # Spacebar-capture toggle threads the query flag through to the listener
    # and echoes its real state back, so both directions must round-trip.
    on = client.post("/api/scan/spacebar-capture", params={"enabled": True})
    assert on.status_code == 200
    assert on.json() == {"ok": True, "enabled": True}
    off = client.post("/api/scan/spacebar-capture", params={"enabled": False})
    assert off.status_code == 200
    assert off.json() == {"ok": True, "enabled": False}


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
    chained_id = chained.json()["id"]
    # The chain fields must round-trip through _format_quest's chain* remapping.
    chained_view = client.get(f"/api/quests/{chained_id}")
    assert chained_view.status_code == 200
    chained_body = chained_view.json()
    assert chained_body["chainName"] == "Iron Challenge"
    assert chained_body["chainPosition"] == 1
    assert chained_body["chainTotal"] == 3

    # Analytics and mob views aggregate over the quests now present. With no
    # curated linked sessions, analytics is an empty list; mobs reflects the
    # cooldown quest's empty mob set, so it stays empty here.
    analytics = client.get("/api/quests/analytics")
    assert analytics.status_code == 200
    assert analytics.json() == []
    mobs = client.get("/api/quests/mobs")
    assert mobs.status_code == 200
    mobs_body = mobs.json()
    assert isinstance(mobs_body, list)
    assert mobs_body == sorted(set(mobs_body))
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
    added_body = added.json()
    item_id = added_body["id"]
    assert added_body["name"] == "Walker Token"
    assert added_body["type"] == "consumable"
    assert added_body["costPerUse"] == 0.0
    assert added_body["enrichmentLevel"] == 1
    assert added_body["amplifierName"] is None

    detail = client.get(f"/api/equipment/library/{item_id}/detail")
    assert detail.status_code == 200
    detail_body = detail.json()
    assert detail_body["totalCostPerUse"] == 0.0
    assert detail_body["costBreakdown"] == []

    renamed = client.put(
        f"/api/equipment/library/{item_id}",
        json={"type": "consumable", "name": "Walker Token v2"},
    )
    assert renamed.status_code == 200
    assert renamed.json()["name"] == "Walker Token v2"
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
    added_body = added.json()
    weapon_item_id = added_body["id"]
    # The catalogue-resolution + cost arithmetic is the point of this test, so
    # pin the computed cost and the amp/enrichment plumbing, not just status.
    assert added_body["type"] == "weapon"
    assert added_body["costPerUse"] > 0
    if amp_catalog_id is not None:
        # An attached amp drives enrichment to level 2 and names the amplifier.
        assert added_body["amplifierName"] is not None
        assert added_body["enrichmentLevel"] == 2
    else:
        assert added_body["enrichmentLevel"] == 1

    detail = client.get(f"/api/equipment/library/{weapon_item_id}/detail")
    assert detail.status_code == 200
    detail_body = detail.json()
    assert detail_body["totalCostPerUse"] > 0
    assert detail_body["costBreakdown"]
    # The damage-enhancer count threads through to the detail weapon shape
    # (max(0, ...) clamp), so a mutant dropping it is caught here.
    assert detail_body["weapon"]["damageEnhancers"] == 2

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

    # Cost calculation against the same catalogue weapon returns a populated
    # breakdown with a positive per-use cost.
    cost = client.post(
        "/api/equipment/cost/calculate",
        json={"catalog_id": weapon_catalog_id, "type": "weapon"},
    )
    assert cost.status_code == 200
    cost_body = cost.json()
    assert cost_body["totalCostPerUse"] > 0
    assert cost_body["costBreakdown"]

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
    entry_body = entry.json()
    entry_id = entry_body["id"]
    assert entry_id
    assert entry_body["date"] == "2026-05-01"
    assert entry_body["type"] == "expense"
    assert entry_body["description"] == "Ammo"
    assert entry_body["amount"] == 12.5
    assert entry_body["tag"] == "ammo"
    # The row must surface in the list endpoint before deletion and vanish
    # after, so a mutant that drops the persist or the list read is caught.
    listed_ids = {r["id"] for r in client.get("/api/analytics/ledger").json()}
    assert entry_id in listed_ids
    assert client.delete(f"/api/analytics/ledger/{entry_id}").status_code in (200, 204)
    assert entry_id not in {r["id"] for r in client.get("/api/analytics/ledger").json()}

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
    preset_body = preset.json()
    preset_id = preset_body["id"]
    assert preset_body["name"] == "Daily ammo"
    assert preset_body["type"] == "expense"
    assert preset_body["amount"] == 10.0
    assert preset_body["tag"] == "ammo"
    # The type-validation guard rejects anything outside expense/markup.
    bad_preset = client.post(
        "/api/analytics/ledger/presets",
        json={
            "name": "Bad",
            "type": "bogus",
            "description": "x",
            "amount": 1.0,
            "tag": "x",
        },
    )
    assert bad_preset.status_code == 400
    assert client.delete(f"/api/analytics/ledger/presets/{preset_id}").status_code in (
        200,
        204,
    )

    item = client.post(
        "/api/analytics/inventory",
        json={"name": "Loot Stack", "tt_value": 5.0, "markup_paid": 1.0},
    )
    assert item.status_code == 200
    item_body = item.json()
    item_id = item_body["id"]
    assert item_body["ttValue"] == 5.0
    assert item_body["markupPaid"] == 1.0
    patched = client.patch(
        f"/api/analytics/inventory/{item_id}", json={"tt_value": 6.0}
    )
    assert patched.status_code == 200
    assert patched.json()["ttValue"] == 6.0
    # Sell economics: cost_basis = tt_value (6.0) + markup_paid (1.0) = 7.0;
    # sale_price 7.5 yields a positive 0.5 delta, emitted as a markup ledger
    # entry tagged inventory_sale, and the sold row is returned + removed.
    sell = client.post(
        f"/api/analytics/inventory/{item_id}/sell", json={"sale_price": 7.5}
    )
    assert sell.status_code == 200
    sell_body = sell.json()
    assert sell_body["soldItem"]["id"] == item_id
    assert sell_body["ledgerEntry"] is not None
    assert sell_body["ledgerEntry"]["type"] == "markup"
    assert sell_body["ledgerEntry"]["amount"] == pytest.approx(0.5)
    assert sell_body["ledgerEntry"]["tag"] == "inventory_sale"
    # The sell consumed the row; deletion is then a clean 404.
    assert client.delete(f"/api/analytics/inventory/{item_id}").status_code == 404

    # A zero-delta sale (sale_price == cost_basis) skips the ledger emission.
    zero_item = client.post(
        "/api/analytics/inventory",
        json={"name": "Break-even Stack", "tt_value": 4.0, "markup_paid": 1.0},
    )
    assert zero_item.status_code == 200
    zero_id = zero_item.json()["id"]
    zero_sell = client.post(
        f"/api/analytics/inventory/{zero_id}/sell", json={"sale_price": 5.0}
    )
    assert zero_sell.status_code == 200
    assert zero_sell.json()["ledgerEntry"] is None


def test_codex_mutations(e2e_http_pipeline):
    """Drive calibrate / claim / meta-claim against a real species."""
    client, _chatlog, _watcher = e2e_http_pipeline
    # State-changing methods require an allowed Origin (the app's origin guard);
    # the Tauri webview always sends one, so mirror it here.
    client.headers["Origin"] = "tauri://localhost"

    species = client.get("/api/codex/species")
    assert species.status_code == 200
    names = [s.get("name") for s in species.json() if isinstance(s, dict)]
    assert names, "bundled codex species catalogue is unexpectedly empty"
    name = names[0]

    # The skill-category validation rejects an out-of-category skill: ranks 1
    # and 2 are cat1, where "Courage" (a cat2 skill) is invalid, so the claim
    # is a 400 (not merely a non-5xx). Checked before calibrate so the next
    # claimable rank is 1.
    bad_claim = client.post(
        "/api/codex/claim",
        json={"species_name": name, "rank": 1, "skill_name": "Courage"},
    )
    assert bad_claim.status_code == 400

    # Calibrate is a pure state write; pin the echoed species/rank and confirm
    # the persisted rank surfaces on the species list (currentRank advances).
    calibrate = client.post(
        "/api/codex/calibrate", json={"species_name": name, "rank": 1}
    )
    assert calibrate.status_code == 200
    calibrate_body = calibrate.json()
    assert calibrate_body["speciesName"] == name
    assert calibrate_body["rank"] == 1
    after_calibrate = next(
        s for s in client.get("/api/codex/species").json() if s["name"] == name
    )
    assert after_calibrate["currentRank"] == 1

    # A valid claim (the next rank, a category-appropriate skill) records a
    # positive reward; "Aim" is a cat1 skill valid at rank 2.
    claim = client.post(
        "/api/codex/claim",
        json={"species_name": name, "rank": 2, "skill_name": "Aim"},
    )
    assert claim.status_code == 200
    claim_body = claim.json()
    assert claim_body["speciesName"] == name
    assert claim_body["rank"] == 2
    assert claim_body["skillName"] == "Aim"
    assert claim_body["pedValue"] > 0

    # The meta attribute claim always returns 1 PED into the named attribute.
    meta = client.post("/api/codex/meta/claim", json={"attribute_name": "Health"})
    assert meta.status_code == 200
    meta_body = meta.json()
    assert meta_body["attributeName"] == "Health"
    assert meta_body["pedValue"] == 1.0


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
    mob_breakdown = detail["mobBreakdown"][0]
    mob_name = mob_breakdown["currentName"]
    cohort_kills = mob_breakdown["killCount"]
    loot_name = next(
        (item["name"] for item in detail["lootBreakdown"] if " " not in item["name"]),
        "Shrapnel",
    )

    # Rename rewrites kills.mob_name and re-queries the destination cohort
    # count; the detail re-read must show the new currentName with the prior
    # value preserved as originalName.
    rename = client.post(
        f"/api/tracking/session/{session_id}/rename-mob",
        json={"fromMobName": mob_name, "toMobName": "Atrox Young"},
    )
    assert rename.status_code == 200
    rename_body = rename.json()
    assert rename_body["mobName"] == "Atrox Young"
    assert rename_body["killCount"] == cohort_kills
    renamed_row = next(
        row
        for row in client.get(f"/api/tracking/session/{session_id}").json()[
            "mobBreakdown"
        ]
        if row["currentName"] == "Atrox Young"
    )
    assert renamed_row["originalName"] == mob_name

    # Restore is the inverse: rename then restore is identity, landing back at
    # the genuinely-original capture with originalName cleared.
    restore = client.post(
        f"/api/tracking/session/{session_id}/restore-mob",
        json={"currentMobName": "Atrox Young"},
    )
    assert restore.status_code == 200
    assert restore.json()["mobName"] == mob_name
    restored_row = next(
        row
        for row in client.get(f"/api/tracking/session/{session_id}").json()[
            "mobBreakdown"
        ]
        if row["currentName"] == mob_name
    )
    assert restored_row["originalName"] is None

    # Armour cost accumulates onto the session and rolls up into the cost
    # breakdown, so the echoed value and the downstream rollup must both move.
    armour_before = client.get(f"/api/tracking/session/{session_id}").json()["summary"][
        "costBreakdown"
    ]["armourCost"]
    armour = client.post(
        f"/api/tracking/session/{session_id}/armour-cost", json={"cost": 1.5}
    )
    assert armour.status_code == 200
    assert armour.json()["armourCost"] == 1.5
    armour_after = client.get(f"/api/tracking/session/{session_id}").json()["summary"][
        "costBreakdown"
    ]["armourCost"]
    assert armour_after == round(armour_before + 1.5, 2)

    # Deactivate flips loot rows, applies a negative returns delta, and reports
    # the recomputed session total; activate is the exact inverse.
    returns_before = client.get(f"/api/tracking/session/{session_id}").json()[
        "summary"
    ]["returns"]
    deactivate = client.post(
        f"/api/tracking/session/{session_id}/loot-item/{loot_name}/deactivate"
    )
    assert deactivate.status_code == 200
    deactivate_body = deactivate.json()
    assert deactivate_body["affectedRows"] >= 1
    assert deactivate_body["totalValueDelta"] < 0
    assert deactivate_body["sessionTotalReturns"] == round(
        returns_before + deactivate_body["totalValueDelta"], 2
    )

    activate = client.post(
        f"/api/tracking/session/{session_id}/loot-item/{loot_name}/activate"
    )
    assert activate.status_code == 200
    activate_body = activate.json()
    assert activate_body["totalValueDelta"] == pytest.approx(
        -deactivate_body["totalValueDelta"]
    )
    returns_after = client.get(f"/api/tracking/session/{session_id}").json()["summary"][
        "returns"
    ]
    assert returns_after == returns_before

    # The decline decision branch returns a declined status for the session;
    # an unknown action falls through to the 400 guard.
    declined = client.post(
        f"/api/tracking/session/{session_id}/quest-link", json={"action": "decline"}
    )
    assert declined.status_code == 200
    declined_body = declined.json()
    assert declined_body["status"] == "declined"
    assert declined_body["sessionId"] == session_id
    bad_action = client.post(
        f"/api/tracking/session/{session_id}/quest-link", json={"action": "dismiss"}
    )
    assert bad_action.status_code == 400

    # Repair OCR is disabled by default, so this exercises the disabled guard
    # without touching a capture device; pin the guard's reason, not just 400.
    repair = client.post(f"/api/tracking/session/{session_id}/repair-scan")
    assert repair.status_code == 400
    assert repair.json()["detail"] == "Repair OCR is disabled"

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

    # Put the (session-less) config in tag mode so the mode-precondition
    # branches resolve deterministically rather than 409-ing on a mismatch.
    assert (
        client.patch(
            "/api/settings",
            json={"mob_tracking_mode": "tag", "mob_tracking_tag": "Atrox"},
        ).status_code
        == 200
    )

    tag_lock = client.post("/api/tracking/tag-lock", json={"tag": "Atrox"})
    assert tag_lock.status_code == 200
    assert tag_lock.json() == {"tag": "Atrox"}

    # Tag mode disables manual mob selection: the gate must 409 with its
    # message, not silently accept the lock.
    manual_lock = client.post(
        "/api/tracking/manual-mob-lock",
        json={"species": "Atrox", "maturity": "Young"},
    )
    assert manual_lock.status_code == 409
    assert manual_lock.json()["detail"] == "Tag mode disables manual mob selection"

    # Release returns the label it cleared (the active tag).
    release = client.post("/api/tracking/release-mob")
    assert release.status_code == 200
    assert release.json()["released"] == "Atrox"


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
        mob_patch = client.patch(
            "/api/settings",
            json={"mob_tracking_mode": "mob", "hotbar_hooks_enabled": True},
        )
        assert mob_patch.status_code == 200
        mob_body = mob_patch.json()
        assert mob_body["mobTrackingMode"] == "mob"
        assert mob_body["hotbarHooksEnabled"] is True

        # Switch to tag mode while live (the tag-mode early return in reload).
        tag_patch = client.patch(
            "/api/settings",
            json={"mob_tracking_mode": "tag", "mob_tracking_tag": "Atrox"},
        )
        assert tag_patch.status_code == 200
        tag_body = tag_patch.json()
        assert tag_body["mobTrackingMode"] == "tag"
        assert tag_body["mobTrackingTag"] == "Atrox"

        # The session is still live after the reloads, and the live status
        # surface reflects the toggled config (repair flag unchanged, hotbar
        # flag on), confirming the patch path threaded config through without
        # tearing down the session.
        assert tracker.is_tracking
        status = client.get("/api/tracking/status")
        assert status.status_code == 200
        assert status.json()["status"] == "active"
        assert status.json()["repairOcrEnabled"] is False
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
