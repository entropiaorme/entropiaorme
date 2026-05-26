"""Integration tests: full tracking pipeline via event bus.

Tests the kills model: shots accumulate, loot creates kill records,
session end creates dangling cost.
"""

import sqlite3
import time
import uuid
from datetime import datetime, timedelta

import pytest
from fastapi import HTTPException

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
)
from backend.routers.tracking import (
    _bulk_activate_loot_item_impl,
    _bulk_deactivate_loot_item_impl,
    _rename_session_mob_impl,
    _restore_session_mob_impl,
    get_session_impl,
)
from backend.tracking.schema import init_tracking_tables
from backend.tracking.tracker import HuntTracker


@pytest.fixture
def pipeline():
    """Set up a full tracking pipeline with in-memory DB."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db)
    return bus, tracker, db


class TestFullPipeline:
    def test_start_stop_empty_session(self, pipeline):
        bus, tracker, db = pipeline
        session = tracker.start_session()
        assert tracker.is_tracking
        assert session.id

        result = tracker.stop_session()
        assert result is not None
        assert not tracker.is_tracking
        assert result.end_time is not None
        assert len(result.kills) == 0

        # Verify DB
        row = db.execute(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            (session.id,),
        ).fetchone()
        assert row[0] == 0

    def test_combat_accumulates_stats(self, pipeline):
        """Combat without loot → no kills, stats in dangling cost."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.5, "timestamp": now}
        )
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now}
        )
        bus.publish(
            EVENT_COMBAT, {"type": "critical_hit", "amount": 30.0, "timestamp": now}
        )

        # Accumulator should have stats
        acc = tracker.current_accumulator
        assert acc.shots_fired == 3
        assert acc.damage_dealt == 55.5
        assert acc.critical_hits == 1

        result = tracker.stop_session()
        assert result is not None
        assert len(result.kills) == 0
        # Dangling cost persisted (weapon cost = 0 since no equipment lookup)
        assert result.dangling_cost == 0.0  # No cost_per_shot configured

    def test_loot_creates_kill(self, pipeline):
        """Damage then loot → kill record created with correct stats."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "type": "loot",
                "items": [
                    {"item_name": "Shrapnel", "quantity": 50, "value_ped": 0.50},
                    {
                        "item_name": "Animal Oil Residue",
                        "quantity": 3,
                        "value_ped": 0.03,
                    },
                ],
                "total_ped": 0.53,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        assert len(result.kills) == 1

        kill = result.kills[0]
        assert kill.loot_total_ped == 0.53
        assert kill.shots_fired == 2
        assert kill.damage_dealt == 25.0

        # Check loot in DB
        loot_rows = db.execute(
            "SELECT item_name, quantity, value_ped FROM kill_loot_items WHERE kill_id = ?",
            (kill.id,),
        ).fetchall()
        assert len(loot_rows) == 2
        names = {r[0] for r in loot_rows}
        assert "Shrapnel" in names

    def test_accumulator_resets_on_loot(self, pipeline):
        """After a kill, accumulator starts fresh."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
                "total_ped": 0.01,
                "timestamp": now,
            },
        )

        # Accumulator should be reset
        acc = tracker.current_accumulator
        assert acc.shots_fired == 0
        assert acc.damage_dealt == 0.0

        tracker.stop_session()

    def test_multiple_kills_in_session(self, pipeline):
        """Multiple loot events create multiple kill records."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        for i in range(3):
            t = now + timedelta(seconds=i * 5)
            bus.publish(
                EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": t}
            )
            bus.publish(
                EVENT_LOOT_GROUP,
                {
                    "items": [
                        {"item_name": "Shrapnel", "quantity": 10, "value_ped": 0.10}
                    ],
                    "total_ped": 0.10,
                    "timestamp": t + timedelta(seconds=1),
                },
            )

        result = tracker.stop_session()
        assert result is not None
        assert len(result.kills) == 3

        # Verify DB
        db_kills = db.execute(
            "SELECT id FROM kills WHERE session_id = ?",
            (result.id,),
        ).fetchall()
        assert len(db_kills) == 3

    def test_dangling_cost_with_equipment(self, pipeline):
        """Shots without loot → dangling cost includes weapon cost."""
        db = sqlite3.connect(":memory:")
        bus = EventBus()
        # Equipment lookup returns cost per shot
        tracker = HuntTracker(bus, db, equipment_cost_lookup=lambda _: 0.50)
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now}
        )

        result = tracker.stop_session()
        assert result is not None
        assert len(result.kills) == 0
        assert abs(result.dangling_cost - 1.00) < 1e-6  # 2 shots × 0.50

        # Verify in DB
        row = db.execute(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
            (result.id,),
        ).fetchone()
        assert abs(row[0] - 1.00) < 1e-6

    def test_countered_shot_counts_cost_in_standard_mode(self, pipeline):
        """Countered attacks still consume a shot and cost in standard mode."""
        db = sqlite3.connect(":memory:")
        bus = EventBus()
        tracker = HuntTracker(bus, db, equipment_cost_lookup=lambda _: 0.50)
        tracker.start_session()

        from backend.core.events import EVENT_ACTIVE_TOOL_CHANGED

        now = datetime.now(tz=None)
        bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": "Opalo"})
        bus.publish(EVENT_COMBAT, {"type": "target_jam", "timestamp": now})

        result = tracker.stop_session()
        assert result is not None
        assert result.dangling_cost == 0.50

        row = db.execute(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
            (result.id,),
        ).fetchone()
        assert abs(row[0] - 0.50) < 1e-6

    def test_blacklisted_loot_filtered(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {"item_name": "Universal Ammo", "quantity": 100, "value_ped": 1.0}
                ],
                "total_ped": 1.0,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert len(kill.loot_items) == 0  # Universal Ammo is blacklisted
        assert kill.loot_total_ped == 0.0

    def test_blacklist_refreshes_before_session_start(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        blacklist = ["Universal Ammo"]
        tracker = HuntTracker(
            bus,
            db,
            loot_filter_blacklist=blacklist.copy(),
            loot_filter_blacklist_provider=lambda: blacklist,
        )

        blacklist.append("Animal Oil Residue")
        tracker.reload_config()
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {
                        "item_name": "Animal Oil Residue",
                        "quantity": 3,
                        "value_ped": 0.03,
                    },
                    {"item_name": "Shrapnel", "quantity": 5, "value_ped": 0.05},
                ],
                "total_ped": 0.08,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert len(kill.loot_items) == 1
        assert kill.loot_items[0].item_name == "Shrapnel"
        assert kill.loot_total_ped == 0.05

    def test_unknown_mob_when_no_lock(self, pipeline):
        """Kill gets 'Unknown' mob when no manual mob/tag is set."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
                "total_ped": 0.01,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert kill.mob_name == "Unknown"

    def test_tool_stats_merge_unknown(self, pipeline):
        """Unknown tool stats merge into real tool on detection."""
        bus, tracker, db = pipeline
        tracker.start_session()

        from backend.core.events import EVENT_ACTIVE_TOOL_CHANGED

        now = datetime.now(tz=None)

        # Shots before tool detection go to "Unknown"
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now}
        )

        # Tool detected → merge
        bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": "Opalo"})

        # More shots under real tool
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 20.0, "timestamp": now}
        )

        # Loot → creates kill
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
                "total_ped": 0.01,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert "Unknown" not in kill.tool_stats
        assert "Opalo" in kill.tool_stats
        assert kill.tool_stats["Opalo"].shots_fired == 3
        assert kill.tool_stats["Opalo"].damage_dealt == 45.0

    def test_shrapnel_conversion_ledger_entry(self, pipeline):
        """Shrapnel looted during a session creates a 1% margin ledger entry."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {"item_name": "Shrapnel", "quantity": 1000, "value_ped": 10.00},
                    {
                        "item_name": "Animal Oil Residue",
                        "quantity": 5,
                        "value_ped": 0.05,
                    },
                ],
                "total_ped": 10.05,
                "timestamp": now,
            },
        )

        tracker.stop_session()

        ledger = db.execute(
            "SELECT type, description, amount, tag FROM ledger_entries"
        ).fetchall()
        assert len(ledger) == 1
        entry = ledger[0]
        assert entry[0] == "markup"
        assert entry[1] == "Shrapnel Conversion"
        assert abs(entry[2] - 0.10) < 0.001
        assert entry[3] == "convert"

    def test_enhancer_refund_creates_rebate_and_skips_conversion_margin(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_ENHANCER_BREAK,
            {
                "enhancer_name": "T1 Weapon Damage Enhancer",
                "item_name": "Electric Attack Nanochip 9",
                "remaining": 3,
                "shrapnel_ped": 0.80,
            },
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {
                        "item_name": "Shrapnel",
                        "quantity": 8000,
                        "value_ped": 0.80,
                        "is_enhancer_shrapnel": True,
                    },
                    {"item_name": "Shrapnel", "quantity": 1000, "value_ped": 10.00},
                ],
                "total_ped": 10.80,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None

        assert len(result.kills) == 1
        assert result.kills[0].loot_total_ped == 10.00

        ledger = db.execute(
            "SELECT description, amount, tag FROM ledger_entries ORDER BY description"
        ).fetchall()
        assert ledger == [
            ("Enhancer Shrapnel Rebate", 0.8, "enhancer"),
            ("Shrapnel Conversion", 0.1, "convert"),
        ]

    def test_no_shrapnel_no_ledger_entry(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {
                        "item_name": "Animal Oil Residue",
                        "quantity": 5,
                        "value_ped": 0.05,
                    }
                ],
                "total_ped": 0.05,
                "timestamp": now,
            },
        )

        tracker.stop_session()

        ledger = db.execute("SELECT * FROM ledger_entries").fetchall()
        assert len(ledger) == 0

    def test_schema_created(self, pipeline):
        _, _, db = pipeline
        tables = db.execute(
            "SELECT name FROM sqlite_master WHERE type='table'",
        ).fetchall()
        table_names = {t[0] for t in tables}
        assert "tracking_sessions" in table_names
        assert "kills" in table_names
        assert "kill_tool_stats" in table_names
        assert "kill_loot_items" in table_names
        assert "ledger_entries" in table_names
        loot_cols = {
            row[1]
            for row in db.execute("PRAGMA table_info(kill_loot_items)").fetchall()
        }
        assert "is_enhancer_shrapnel" in loot_cols
        assert "deactivated_at" in loot_cols
        kill_cols = {
            row[1] for row in db.execute("PRAGMA table_info(kills)").fetchall()
        }
        assert "original_mob_name" in kill_cols


# ── Mob Lock Confirmation & Retrofit ──────────────────────────────────

_kill_counter = 0


def _make_kill(bus, now=None):
    """Publish combat + loot to create a kill record.

    Uses a counter to vary total_ped slightly, avoiding loot deduplication.
    """
    global _kill_counter
    _kill_counter += 1
    now = now or datetime.now(tz=None)
    ped = 0.01 * _kill_counter
    bus.publish(
        EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now}
    )
    bus.publish(
        EVENT_LOOT_GROUP,
        {
            "items": [
                {"item_name": "Shrapnel", "quantity": _kill_counter, "value_ped": ped}
            ],
            "total_ped": ped,
            "timestamp": now,
        },
    )


class TestTrifectaInferredManualMobLock:
    def test_prearmed_trifecta_inferred_mob_is_active_from_session_start(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            weapon_attribution_trifecta_provider=lambda: True,
            manual_mob_provider=lambda: ("Atrox", "Young"),
        )
        tracker.start_session()

        _make_kill(bus)
        result = tracker.stop_session()
        assert result is not None

        assert result.kills[0].mob_name == "Young Atrox"
        assert result.kills[0].mob_species == "Atrox"
        assert result.kills[0].mob_maturity == "Young"

    def test_prearmed_standard_manual_mob_is_active_from_session_start(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            weapon_attribution_trifecta_provider=lambda: False,
            manual_mob_entry_enabled_provider=lambda: True,
            manual_mob_provider=lambda: ("Atrox", "Young"),
        )
        tracker.start_session()

        _make_kill(bus)
        result = tracker.stop_session()
        assert result is not None

        assert result.kills[0].mob_name == "Young Atrox"
        assert result.kills[0].mob_species == "Atrox"
        assert result.kills[0].mob_maturity == "Young"

    def test_manual_lock_stamps_future_kills_without_retrofit(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()

        _make_kill(bus)
        assert tracker.session.kills[0].mob_name == "Unknown"

        tracker.set_manual_mob("Young Atrox", "Atrox", "Young")
        _make_kill(bus)

        result = tracker.stop_session()
        assert result is not None
        assert [kill.mob_name for kill in result.kills] == ["Unknown", "Young Atrox"]
        assert result.kills[1].mob_species == "Atrox"
        assert result.kills[1].mob_maturity == "Young"

    def test_manual_release_returns_to_unknown(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()

        tracker.set_manual_mob("Young Atrox", "Atrox", "Young")
        _make_kill(bus)
        assert tracker.release_current_mob() == "Young Atrox"
        _make_kill(bus)

        result = tracker.stop_session()
        assert result is not None
        assert [kill.mob_name for kill in result.kills] == ["Young Atrox", "Unknown"]


class TestSessionTagMode:
    def test_prearmed_tag_is_active_from_session_start(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "Easter Mayhem",
        )
        tracker.start_session()

        _make_kill(bus)
        result = tracker.stop_session()
        assert result is not None

        assert result.kills[0].mob_name == "Easter Mayhem"
        assert result.kills[0].mob_species == ""
        assert result.kills[0].mob_maturity == ""

    def test_tag_mode_defaults_to_unknown_until_tag_is_set(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "",
        )
        tracker.start_session()

        _make_kill(bus)
        result = tracker.stop_session()
        assert result is not None

        assert result.kills[0].mob_name == "Unknown"
        assert result.kills[0].mob_species == ""
        assert result.kills[0].mob_maturity == ""

    def test_tag_mode_stamps_future_kills_after_tag_is_set(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "",
        )
        tracker.start_session()
        tracker.set_manual_tag("Easter Mayhem")

        _make_kill(bus)
        result = tracker.stop_session()
        assert result is not None

        assert result.kills[0].mob_name == "Easter Mayhem"
        assert result.kills[0].mob_species == ""
        assert result.kills[0].mob_maturity == ""

    def test_tag_release_returns_to_unknown(self):
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "tag",
            mob_tracking_tag_provider=lambda: "",
        )
        tracker.start_session()

        tracker.set_manual_tag("Easter Mayhem")
        _make_kill(bus)
        assert tracker.release_current_mob() == "Easter Mayhem"
        _make_kill(bus)

        result = tracker.stop_session()
        assert result is not None
        assert [kill.mob_name for kill in result.kills] == ["Easter Mayhem", "Unknown"]


# ── Global/HoF correlation ──────────────────────────────────────────────


@pytest.fixture
def pipeline_with_player():
    """Pipeline with player_name set; needed for global event filtering."""
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db, player_name="TestPlayer")
    return bus, tracker, db


class TestGlobalCorrelation:
    def test_global_flags_recent_kill(self, pipeline_with_player):
        """Global event correlates to the most recently created kill."""
        bus, tracker, db = pipeline_with_player
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 50.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {"item_name": "Shrapnel", "quantity": 5000, "value_ped": 50.00},
                    {"item_name": "Blazar Fragment", "quantity": 1, "value_ped": 2.50},
                ],
                "total_ped": 52.50,
                "timestamp": now,
            },
        )

        bus.publish(
            EVENT_GLOBAL,
            {
                "type": "global_kill",
                "player": "TestPlayer",
                "creature": "Atrox Provider",
                "value": 52.50,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert kill.is_global is True
        assert kill.is_hof is False

        row = db.execute(
            "SELECT is_global, is_hof FROM kills WHERE id = ?", (kill.id,)
        ).fetchone()
        assert row[0] == 1
        assert row[1] == 0

    def test_hof_flags_kill(self, pipeline_with_player):
        """HoF event sets both is_global and is_hof."""
        bus, tracker, db = pipeline_with_player
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 100.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {"item_name": "Shrapnel", "quantity": 10000, "value_ped": 100.00}
                ],
                "total_ped": 100.00,
                "timestamp": now,
            },
        )
        bus.publish(
            EVENT_GLOBAL,
            {
                "type": "hof_kill",
                "player": "TestPlayer",
                "creature": "Atrox Stalker",
                "value": 100.00,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert kill.is_global is True
        assert kill.is_hof is True

    def test_other_player_global_ignored(self, pipeline_with_player):
        bus, tracker, db = pipeline_with_player
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 50.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {"item_name": "Shrapnel", "quantity": 5000, "value_ped": 50.00}
                ],
                "total_ped": 50.00,
                "timestamp": now,
            },
        )
        bus.publish(
            EVENT_GLOBAL,
            {
                "type": "global_kill",
                "player": "SomeoneElse",
                "creature": "Atrox",
                "value": 50.00,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]
        assert kill.is_global is False

    def test_notable_event_has_kill_id(self, pipeline_with_player):
        """Notable event record has the correct kill_id."""
        bus, tracker, db = pipeline_with_player
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(
            EVENT_COMBAT, {"type": "damage_dealt", "amount": 30.0, "timestamp": now}
        )
        bus.publish(
            EVENT_LOOT_GROUP,
            {
                "items": [
                    {"item_name": "Shrapnel", "quantity": 3000, "value_ped": 30.00}
                ],
                "total_ped": 30.00,
                "timestamp": now,
            },
        )
        bus.publish(
            EVENT_GLOBAL,
            {
                "type": "global_kill",
                "player": "TestPlayer",
                "creature": "Atrox",
                "value": 30.00,
                "timestamp": now,
            },
        )

        result = tracker.stop_session()
        assert result is not None
        kill = result.kills[0]

        notable = db.execute(
            "SELECT kill_id FROM notable_events WHERE event_type = 'global_kill'"
        ).fetchone()
        assert notable[0] == kill.id


# ── Crash recovery ─────────────────────────────────────────────────────


def _setup_orphan_db():
    """Create a DB with tracking tables and return it; no HuntTracker yet."""
    db = sqlite3.connect(":memory:")
    init_tracking_tables(db)
    return db


class TestCrashRecovery:
    def test_orphaned_session_with_kills_recovered(self):
        """Orphaned session with persisted kills is closed with correct end time."""
        db = _setup_orphan_db()
        session_id = str(uuid.uuid4())
        started_at = time.time() - 3600

        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES (?, ?, 1)",
            (session_id, started_at),
        )
        # Two persisted kills
        kill1_id = str(uuid.uuid4())
        kill1_ts = started_at + 60
        db.execute(
            """INSERT INTO kills (id, session_id, mob_name, timestamp,
               shots_fired, damage_dealt, loot_total_ped, cost_ped)
               VALUES (?, ?, 'Atrox', ?, 10, 100.0, 1.50, 5.0)""",
            (kill1_id, session_id, kill1_ts),
        )
        kill2_id = str(uuid.uuid4())
        kill2_ts = started_at + 180
        db.execute(
            """INSERT INTO kills (id, session_id, mob_name, timestamp,
               shots_fired, damage_dealt, loot_total_ped, cost_ped)
               VALUES (?, ?, 'Atrox', ?, 20, 200.0, 3.00, 10.0)""",
            (kill2_id, session_id, kill2_ts),
        )
        # Add shrapnel loot for ledger tests
        db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) VALUES (?, 'Shrapnel', 500, 5.00, 0)",
            (kill1_id,),
        )
        db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) VALUES (?, 'Shrapnel', 80, 0.80, 1)",
            (kill2_id,),
        )
        db.commit()

        bus = EventBus()
        HuntTracker(bus, db)

        row = db.execute(
            "SELECT is_active, ended_at FROM tracking_sessions WHERE id = ?",
            (session_id,),
        ).fetchone()
        assert row[0] == 0
        assert row[1] == kill2_ts  # ended_at = latest kill timestamp

        ledger = db.execute(
            "SELECT description, amount, tag FROM ledger_entries ORDER BY description"
        ).fetchall()
        assert ledger == [
            ("Enhancer Shrapnel Rebate", 0.8, "enhancer"),
            ("Shrapnel Conversion", 0.05, "convert"),
        ]

    def test_orphaned_session_no_kills(self):
        """Orphaned session with no kills closes with started_at as end time."""
        db = _setup_orphan_db()
        session_id = str(uuid.uuid4())
        started_at = time.time() - 600

        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES (?, ?, 1)",
            (session_id, started_at),
        )
        db.commit()

        bus = EventBus()
        HuntTracker(bus, db)

        row = db.execute(
            "SELECT is_active, ended_at FROM tracking_sessions WHERE id = ?",
            (session_id,),
        ).fetchone()
        assert row[0] == 0
        assert row[1] == started_at

    def test_already_closed_session_untouched(self):
        db = _setup_orphan_db()
        session_id = str(uuid.uuid4())
        started_at = time.time() - 3600
        ended_at = started_at + 1800

        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost) VALUES (?, ?, ?, 0, 1.5)",
            (session_id, started_at, ended_at),
        )
        db.commit()

        bus = EventBus()
        HuntTracker(bus, db)

        row = db.execute(
            "SELECT is_active, ended_at, heal_cost FROM tracking_sessions WHERE id = ?",
            (session_id,),
        ).fetchone()
        assert row[0] == 0
        assert row[1] == ended_at
        assert row[2] == 1.5

    def test_new_session_after_recovery(self):
        db = _setup_orphan_db()
        old_id = str(uuid.uuid4())
        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES (?, ?, 1)",
            (old_id, time.time() - 600),
        )
        db.commit()

        bus = EventBus()
        tracker = HuntTracker(bus, db)

        assert (
            db.execute(
                "SELECT is_active FROM tracking_sessions WHERE id = ?",
                (old_id,),
            ).fetchone()[0]
            == 0
        )

        tracker.start_session()
        assert tracker.is_tracking
        tracker.stop_session()
        assert not tracker.is_tracking


# Substrate invariant: `kill_loot_items.deactivated_at` and the
# denormalised `kills.loot_total_ped` must mutate atomically; the API
# layer wraps the same SQL pattern under transaction + cache
# invalidation.


class TestLootDeactivation:
    def _seed_session_with_loot(self, db):
        """Insert one session with one kill carrying two loot items.

        Returns (session_id, kill_id, [loot_row_id, loot_row_id], [value_ped, value_ped]).
        kills.loot_total_ped is the sum of the two loot rows so the
        denormalisation invariant is established up-front.
        """
        session_id = str(uuid.uuid4())
        kill_id = str(uuid.uuid4())
        now = time.time()

        loot_a_value = 4.20
        loot_b_value = 1.30
        loot_total = loot_a_value + loot_b_value

        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
            "VALUES (?, ?, ?, 0)",
            (session_id, now - 600, now),
        )
        db.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, "
            "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
            "VALUES (?, ?, 'Atrox', ?, 10, 100.0, ?, 5.0)",
            (kill_id, session_id, now - 60, loot_total),
        )
        cursor_a = db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, 'Mob-Drop-A', 1, ?, 0)",
            (kill_id, loot_a_value),
        )
        loot_a_id = cursor_a.lastrowid
        cursor_b = db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, 'Mob-Drop-B', 1, ?, 0)",
            (kill_id, loot_b_value),
        )
        loot_b_id = cursor_b.lastrowid
        db.commit()
        return session_id, kill_id, (loot_a_id, loot_b_id), (loot_a_value, loot_b_value)

    def test_schema_column_present_and_defaults_null(self):
        """Fresh-init tracking schema lands kill_loot_items.deactivated_at as nullable."""
        db = _setup_orphan_db()
        cols = {
            row[1]: row
            for row in db.execute("PRAGMA table_info(kill_loot_items)").fetchall()
        }
        assert "deactivated_at" in cols
        # row shape: (cid, name, type, notnull, dflt_value, pk)
        notnull_flag = cols["deactivated_at"][3]
        assert notnull_flag == 0, "deactivated_at must be nullable"

        # New inserts default to NULL; verifies the column is truly nullable
        # at the row level, not just by schema declaration.
        _, _, (loot_a_id, _), _ = self._seed_session_with_loot(db)
        row = db.execute(
            "SELECT deactivated_at FROM kill_loot_items WHERE id = ?",
            (loot_a_id,),
        ).fetchone()
        assert row[0] is None

    def test_deactivate_reduces_denormalised_kill_total(self):
        """Deactivating a loot row shrinks the per-kill total to match
        the remaining active loot, and the per-session aggregate
        (the read path used by `list_sessions_impl`) reflects the change."""
        db = _setup_orphan_db()
        session_id, kill_id, (loot_a_id, _), (loot_a_value, loot_b_value) = (
            self._seed_session_with_loot(db)
        )
        initial_total = loot_a_value + loot_b_value

        # Confirm the starting invariant: kills.loot_total_ped equals the
        # sum of kill_loot_items rows. This is the denormalisation that the
        # deactivation manoeuvre maintains.
        kills_total_before = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (kill_id,),
        ).fetchone()[0]
        assert kills_total_before == pytest.approx(initial_total)

        # Deactivate loot_a. The atomic two-statement manoeuvre below is
        # what future API + service code will execute inside one transaction.
        db.execute(
            "UPDATE kill_loot_items SET deactivated_at = ? WHERE id = ?",
            (time.time(), loot_a_id),
        )
        db.execute(
            "UPDATE kills SET loot_total_ped = loot_total_ped - ? WHERE id = ?",
            (loot_a_value, kill_id),
        )
        db.commit()

        # Per-kill denormalised total tracks the remaining active loot.
        kills_total_after = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (kill_id,),
        ).fetchone()[0]
        assert kills_total_after == pytest.approx(loot_b_value)

        # Per-session aggregate: this is the exact query shape used by
        # `routers/tracking.py::list_sessions_impl` for the session list's
        # returns column, and by the analytics surface for cross-session
        # rollups. It reads from `kills.loot_total_ped` directly and so
        # picks up the deactivation without needing a filter clause.
        session_returns = db.execute(
            "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
            (session_id,),
        ).fetchone()[0]
        assert session_returns == pytest.approx(loot_b_value)

        # The deactivated row is still present in kill_loot_items (soft
        # delete, not destructive) and carries the timestamp.
        row = db.execute(
            "SELECT deactivated_at, value_ped FROM kill_loot_items WHERE id = ?",
            (loot_a_id,),
        ).fetchone()
        assert row[0] is not None
        assert row[1] == pytest.approx(loot_a_value)

    def test_reactivate_restores_denormalised_kill_total(self):
        """Reactivating restores the per-kill total and clears the flag."""
        db = _setup_orphan_db()
        session_id, kill_id, (loot_a_id, _), (loot_a_value, loot_b_value) = (
            self._seed_session_with_loot(db)
        )

        # Deactivate then reactivate; landing state should equal the
        # pre-deactivation state.
        db.execute(
            "UPDATE kill_loot_items SET deactivated_at = ? WHERE id = ?",
            (time.time(), loot_a_id),
        )
        db.execute(
            "UPDATE kills SET loot_total_ped = loot_total_ped - ? WHERE id = ?",
            (loot_a_value, kill_id),
        )
        db.commit()

        db.execute(
            "UPDATE kill_loot_items SET deactivated_at = NULL WHERE id = ?",
            (loot_a_id,),
        )
        db.execute(
            "UPDATE kills SET loot_total_ped = loot_total_ped + ? WHERE id = ?",
            (loot_a_value, kill_id),
        )
        db.commit()

        kills_total = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (kill_id,),
        ).fetchone()[0]
        assert kills_total == pytest.approx(loot_a_value + loot_b_value)

        row = db.execute(
            "SELECT deactivated_at FROM kill_loot_items WHERE id = ?",
            (loot_a_id,),
        ).fetchone()
        assert row[0] is None

        session_returns = db.execute(
            "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
            (session_id,),
        ).fetchone()[0]
        assert session_returns == pytest.approx(loot_a_value + loot_b_value)

    def test_per_kill_loot_breakdown_filter_isolates_active_rows(self):
        """The session-detail loot breakdown query (one of the three sites
        in the production code that needs the `deactivated_at IS NULL`
        filter) returns only active rows for its item-name rollup,
        without losing them from the underlying table."""
        db = _setup_orphan_db()
        session_id, kill_id, (loot_a_id, _), (_, loot_b_value) = (
            self._seed_session_with_loot(db)
        )

        db.execute(
            "UPDATE kill_loot_items SET deactivated_at = ? WHERE id = ?",
            (time.time(), loot_a_id),
        )
        db.commit()

        # Mirror of routers/tracking.py::get_session_impl's loot breakdown
        # query, plus the filter clause the deactivate/activate API
        # surface will add.
        active_breakdown = db.execute(
            "SELECT l.item_name, SUM(l.quantity), SUM(l.value_ped) "
            "FROM kill_loot_items l "
            "JOIN kills k ON k.id = l.kill_id "
            "WHERE k.session_id = ? "
            "AND COALESCE(l.is_enhancer_shrapnel, 0) = 0 "
            "AND l.deactivated_at IS NULL "
            "GROUP BY l.item_name",
            (session_id,),
        ).fetchall()
        assert len(active_breakdown) == 1
        assert active_breakdown[0][0] == "Mob-Drop-B"
        assert active_breakdown[0][2] == pytest.approx(loot_b_value)

        # The deactivated row is still queryable without the filter; the
        # frontend's greyed-out section will read it this way.
        all_rows = db.execute(
            "SELECT item_name, deactivated_at FROM kill_loot_items "
            "WHERE kill_id = ? ORDER BY item_name",
            (kill_id,),
        ).fetchall()
        assert len(all_rows) == 2
        assert all_rows[0][0] == "Mob-Drop-A"
        assert all_rows[0][1] is not None  # deactivated
        assert all_rows[1][0] == "Mob-Drop-B"
        assert all_rows[1][1] is None  # active


class TestV30Migration:
    """The version-counter migration that lands deactivated_at on existing
    DBs (v29 → v30 forward-migrate). Fresh installs land the column via
    the canonical tracking schema; this class pins the in-place upgrade
    path and its defensive cases."""

    def test_upgrade_existing_v29_kill_loot_items(self, tmp_path):
        """A v29-shaped DB with kill_loot_items present picks up
        deactivated_at on AppDatabase open."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        # Stand up a v29-shaped DB by hand: app metadata at version 29,
        # kill_loot_items present without the new column.
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '29')")
        seed.execute(
            "CREATE TABLE kill_loot_items ("
            "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
            "  kill_id TEXT NOT NULL,"
            "  item_name TEXT NOT NULL,"
            "  quantity INTEGER DEFAULT 1,"
            "  value_ped REAL NOT NULL,"
            "  is_enhancer_shrapnel INTEGER NOT NULL DEFAULT 0"
            ")"
        )
        seed.commit()
        seed.close()

        # Opening AppDatabase triggers the version-bump migration.
        app_db = AppDatabase(db_path)
        try:
            cols = {
                row[1]
                for row in app_db.conn.execute(
                    "PRAGMA table_info(kill_loot_items)"
                ).fetchall()
            }
            assert "deactivated_at" in cols

            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_without_tracking_tables(self, tmp_path):
        """A v29 install that never started tracking has no
        kill_loot_items table; the migration tolerates that and the
        column will land via init_tracking_tables on first Tracker
        start. Defensive-path coverage."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '29')")
        seed.commit()
        seed.close()

        # Should not raise.
        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_idempotent_when_column_already_present(self, tmp_path):
        """Partial-run safety: the column already exists (e.g. a Tracker
        ran against this DB ahead of the migration in a debug path), and
        the migration's duplicate-column branch swallows the error."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '29')")
        # Column already present at v29.
        seed.execute(
            "CREATE TABLE kill_loot_items ("
            "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
            "  kill_id TEXT NOT NULL,"
            "  item_name TEXT NOT NULL,"
            "  quantity INTEGER DEFAULT 1,"
            "  value_ped REAL NOT NULL,"
            "  is_enhancer_shrapnel INTEGER NOT NULL DEFAULT 0,"
            "  deactivated_at REAL"
            ")"
        )
        seed.commit()
        seed.close()

        # Should not raise.
        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_reopen_at_v30_is_noop(self, tmp_path):
        """Opening twice in a row keeps version at 30 and doesn't
        re-attempt the ALTER; basic version-counter sanity."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"

        first = AppDatabase(db_path)
        first.close()

        second = AppDatabase(db_path)
        try:
            version = second.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            second.close()


# ── Loot deactivate / activate endpoints (aggregate-row affordance) ─
#
# These tests cover the FastAPI-facing impl functions for the post-hoc
# loot-edit affordance: the wholesale-by-item-name wrapper transactions,
# the 400 / 404 / 409 boundary behaviour, the cache-invalidation footprint
# on session_summaries, partial-state semantics, the active-session
# block, and the deactivatedLootBreakdown extension on the session-detail
# response. The underlying SQL manoeuvre is already pinned by
# TestLootDeactivation above; this class focuses on the API contract
# layered on top.


class TestBulkLootEditEndpoints:
    def _seed_multi_item_session(self, db, *, is_active: int = 0):
        """Seed a session with three Nanocube drops across two kills
        plus one Mob-Drop-Other in a third kill, so cross-kill bulk
        flips and untouched-cohort isolation can both be observed."""
        session_id = str(uuid.uuid4())
        kill1_id = str(uuid.uuid4())
        kill2_id = str(uuid.uuid4())
        kill3_id = str(uuid.uuid4())
        now = time.time()
        nano_values = (2.50, 3.00, 1.25)
        other_value = 4.75

        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
            "VALUES (?, ?, ?, ?)",
            (session_id, now - 600, None if is_active else now, is_active),
        )
        # Kill 1: two Nanocube drops.
        db.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, "
            "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
            "VALUES (?, ?, 'Atrox', ?, 5, 50.0, ?, 2.0)",
            (kill1_id, session_id, now - 300, nano_values[0] + nano_values[1]),
        )
        nano1_id = db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, 'Nanocube', 1, ?, 0)",
            (kill1_id, nano_values[0]),
        ).lastrowid
        nano2_id = db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, 'Nanocube', 1, ?, 0)",
            (kill1_id, nano_values[1]),
        ).lastrowid
        # Kill 2: one Nanocube drop.
        db.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, "
            "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
            "VALUES (?, ?, 'Atrox', ?, 4, 40.0, ?, 2.0)",
            (kill2_id, session_id, now - 200, nano_values[2]),
        )
        nano3_id = db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, 'Nanocube', 1, ?, 0)",
            (kill2_id, nano_values[2]),
        ).lastrowid
        # Kill 3: one Mob-Drop-Other, untouched by Nanocube bulk flips.
        db.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, "
            "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
            "VALUES (?, ?, 'Atrox', ?, 6, 60.0, ?, 2.0)",
            (kill3_id, session_id, now - 100, other_value),
        )
        other_id = db.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, 'Mob-Drop-Other', 1, ?, 0)",
            (kill3_id, other_value),
        ).lastrowid
        db.commit()
        return {
            "session_id": session_id,
            "kill1_id": kill1_id,
            "kill2_id": kill2_id,
            "kill3_id": kill3_id,
            "nano_ids": (nano1_id, nano2_id, nano3_id),
            "nano_values": nano_values,
            "other_id": other_id,
            "other_value": other_value,
            "kill1_initial_total": nano_values[0] + nano_values[1],
            "kill2_initial_total": nano_values[2],
            "kill3_initial_total": other_value,
            "session_initial_returns": sum(nano_values) + other_value,
        }

    def _seed_summary_row(self, db, session_id):
        db.execute(
            "INSERT INTO session_summaries ("
            "session_id, summary_version, started_at, ended_at, duration_hours, "
            "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
            "dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, "
            "regular_skill_tt, attribute_levels_total"
            ") VALUES (?, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, '{}', '{}', 0, 0)",
            (session_id,),
        )
        db.commit()

    # ── Happy paths ──────────────────────────────────────────────────

    def test_bulk_deactivate_flips_all_matching_rows_atomically(self):
        """Bulk-deactivate Nanocube flips all three matching rows
        across two kills in one shot; each parent kill's
        loot_total_ped reduces by the sum of its matching rows;
        Mob-Drop-Other is untouched."""
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)

        result = _bulk_deactivate_loot_item_impl(
            db,
            seed["session_id"],
            "Nanocube",
        )

        assert result["sessionId"] == seed["session_id"]
        assert result["itemName"] == "Nanocube"
        assert result["affectedRows"] == 3
        assert result["totalValueDelta"] == pytest.approx(-sum(seed["nano_values"]))
        # Only Mob-Drop-Other remains as live loot.
        assert result["sessionTotalReturns"] == pytest.approx(
            seed["other_value"], rel=1e-4
        )

        # All three Nanocube rows carry deactivated_at; other untouched.
        flags = [
            db.execute(
                "SELECT deactivated_at FROM kill_loot_items WHERE id = ?",
                (i,),
            ).fetchone()[0]
            for i in seed["nano_ids"]
        ]
        assert all(f is not None for f in flags)
        other_flag = db.execute(
            "SELECT deactivated_at FROM kill_loot_items WHERE id = ?",
            (seed["other_id"],),
        ).fetchone()[0]
        assert other_flag is None

        # Per-kill loot_total_ped mutated correctly.
        kill1_total = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (seed["kill1_id"],),
        ).fetchone()[0]
        kill2_total = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (seed["kill2_id"],),
        ).fetchone()[0]
        kill3_total = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (seed["kill3_id"],),
        ).fetchone()[0]
        assert kill1_total == pytest.approx(0.0)
        assert kill2_total == pytest.approx(0.0)
        assert kill3_total == pytest.approx(seed["other_value"])

    def test_bulk_activate_restores_all_matching_rows(self):
        """Bulk-deactivate followed by bulk-activate returns every row
        and per-kill total to pre-edit state."""
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)

        _bulk_deactivate_loot_item_impl(db, seed["session_id"], "Nanocube")
        result = _bulk_activate_loot_item_impl(db, seed["session_id"], "Nanocube")

        assert result["affectedRows"] == 3
        assert result["totalValueDelta"] == pytest.approx(sum(seed["nano_values"]))
        assert result["sessionTotalReturns"] == pytest.approx(
            seed["session_initial_returns"],
            rel=1e-4,
        )

        flags = [
            db.execute(
                "SELECT deactivated_at FROM kill_loot_items WHERE id = ?",
                (i,),
            ).fetchone()[0]
            for i in seed["nano_ids"]
        ]
        assert all(f is None for f in flags)

        kill1_total = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (seed["kill1_id"],),
        ).fetchone()[0]
        kill2_total = db.execute(
            "SELECT loot_total_ped FROM kills WHERE id = ?",
            (seed["kill2_id"],),
        ).fetchone()[0]
        assert kill1_total == pytest.approx(seed["kill1_initial_total"])
        assert kill2_total == pytest.approx(seed["kill2_initial_total"])

    def test_bulk_deactivate_clears_session_summaries_cache(self):
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)
        self._seed_summary_row(db, seed["session_id"])

        _bulk_deactivate_loot_item_impl(db, seed["session_id"], "Nanocube")

        cached = db.execute(
            "SELECT 1 FROM session_summaries WHERE session_id = ?",
            (seed["session_id"],),
        ).fetchone()
        assert cached is None

    def test_bulk_partial_state_only_flips_eligible_rows(self):
        """If one Nanocube row is already deactivated (e.g. a future
        more-granular affordance flipped just one capture), the bulk
        endpoint flips only the remaining active rows; the eligibility
        clause is `deactivated_at IS NULL` rather than "every row for
        this item." Mirrors the inverse property for bulk-activate.

        Partial state is set up by writing the flag directly via SQL
        rather than through an API path. The schema column carries the
        load-bearing semantics; the wholesale-by-item-name endpoint is
        the only API shape that surfaces those semantics today, so this
        guards the case where partial state arrives through some
        out-of-band route."""
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)

        # Pre-deactivate one row directly to set up partial state.
        db.execute(
            "UPDATE kill_loot_items SET deactivated_at = unixepoch('now') WHERE id = ?",
            (seed["nano_ids"][0],),
        )
        db.execute(
            "UPDATE kills SET loot_total_ped = loot_total_ped - ? WHERE id = ?",
            (seed["nano_values"][0], seed["kill1_id"]),
        )
        db.commit()

        result = _bulk_deactivate_loot_item_impl(
            db,
            seed["session_id"],
            "Nanocube",
        )
        assert result["affectedRows"] == 2
        # Now all three are deactivated.
        flags = [
            db.execute(
                "SELECT deactivated_at FROM kill_loot_items WHERE id = ?",
                (i,),
            ).fetchone()[0]
            for i in seed["nano_ids"]
        ]
        assert all(f is not None for f in flags)

    # ── 404 / 409 boundaries ────────────────────────────────────────

    def test_bulk_deactivate_missing_session_is_404(self):
        db = _setup_orphan_db()
        # No seed; just call against a fabricated id.
        with pytest.raises(HTTPException) as excinfo:
            _bulk_deactivate_loot_item_impl(
                db,
                "missing-session",
                "Nanocube",
            )
        assert excinfo.value.status_code == 404

    def test_bulk_deactivate_item_not_in_session_is_404(self):
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)
        with pytest.raises(HTTPException) as excinfo:
            _bulk_deactivate_loot_item_impl(
                db,
                seed["session_id"],
                "Not-In-Session",
            )
        assert excinfo.value.status_code == 404
        assert "no loot named" in excinfo.value.detail.lower()

    def test_bulk_deactivate_all_already_deactivated_is_409(self):
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)
        _bulk_deactivate_loot_item_impl(db, seed["session_id"], "Nanocube")
        with pytest.raises(HTTPException) as excinfo:
            _bulk_deactivate_loot_item_impl(
                db,
                seed["session_id"],
                "Nanocube",
            )
        assert excinfo.value.status_code == 409
        assert "already" in excinfo.value.detail.lower()

    def test_bulk_activate_all_already_active_is_409(self):
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)
        with pytest.raises(HTTPException) as excinfo:
            _bulk_activate_loot_item_impl(
                db,
                seed["session_id"],
                "Nanocube",
            )
        assert excinfo.value.status_code == 409

    def test_bulk_blank_item_name_is_400(self):
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db)
        with pytest.raises(HTTPException) as excinfo:
            _bulk_deactivate_loot_item_impl(db, seed["session_id"], "   ")
        assert excinfo.value.status_code == 400

    def test_bulk_active_session_is_409(self):
        """The active-session block from _validate_session_exists also
        guards the bulk endpoints, closing the parity gap the
        single-row endpoints carried as a parked follow-up."""
        db = _setup_orphan_db()
        seed = self._seed_multi_item_session(db, is_active=1)
        with pytest.raises(HTTPException) as excinfo:
            _bulk_deactivate_loot_item_impl(
                db,
                seed["session_id"],
                "Nanocube",
            )
        assert excinfo.value.status_code == 409
        assert "session" in excinfo.value.detail.lower()

    # ── Session-detail response: parallel aggregate ─────────────────

    def test_session_detail_carries_deactivated_loot_breakdown(self, tmp_path):
        """GET /api/tracking/session/{id} surfaces a
        deactivatedLootBreakdown aggregate parallel to lootBreakdown so
        the frontend can render the greyed section with the inverse
        activate affordance per item name."""
        from backend.db.app_database import AppDatabase

        app_db = AppDatabase(tmp_path / "app.db")
        db = app_db.conn
        init_tracking_tables(db)
        try:
            seed = self._seed_multi_item_session(db)
            _bulk_deactivate_loot_item_impl(
                db,
                seed["session_id"],
                "Nanocube",
            )
            detail = get_session_impl(db, seed["session_id"])
        finally:
            app_db.close()

        # Active aggregate contains only Mob-Drop-Other; deactivated
        # aggregate contains Nanocube rolled up.
        active_names = {row["name"] for row in detail["lootBreakdown"]}
        deactivated_names = {row["name"] for row in detail["deactivatedLootBreakdown"]}
        assert active_names == {"Mob-Drop-Other"}
        assert deactivated_names == {"Nanocube"}

        nano_row = detail["deactivatedLootBreakdown"][0]
        assert nano_row["quantity"] == 3
        assert nano_row["ttValue"] == pytest.approx(
            round(sum(seed["nano_values"]), 2),
        )

    def test_session_detail_empty_deactivated_breakdown_when_no_edits(self, tmp_path):
        from backend.db.app_database import AppDatabase

        app_db = AppDatabase(tmp_path / "app.db")
        db = app_db.conn
        init_tracking_tables(db)
        try:
            seed = self._seed_multi_item_session(db)
            detail = get_session_impl(db, seed["session_id"])
        finally:
            app_db.close()
        assert detail["deactivatedLootBreakdown"] == []


# ── Session-metadata-edit endpoints (rename-mob / restore-mob) ───────
#
# Mass-rename overlay: editing a session's attributed mob name rewrites
# `kills.mob_name` for matching kills in the session and preserves the
# pre-edit value into `kills.original_mob_name` via COALESCE on the
# first rename. Subsequent renames don't overwrite the preservation
# column (COALESCE keeps the first original), so a single restore
# always lands at the genuinely-original capture.


class TestMobEditEndpoints:
    def _seed_mixed_mob_session(self, db):
        """Insert one session with kills split across two mob_names so
        the targeted-rename behaviour can be observed against the
        untouched cohort. Returns the session id + mob names + counts."""
        session_id = str(uuid.uuid4())
        now = time.time()
        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
            "VALUES (?, ?, ?, 0)",
            (session_id, now - 600, now),
        )
        for i in range(3):
            db.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, "
                "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
                "VALUES (?, ?, 'Caboria Old', ?, 5, 50.0, 1.0, 2.0)",
                (str(uuid.uuid4()), session_id, now - 60 + i),
            )
        for i in range(2):
            db.execute(
                "INSERT INTO kills (id, session_id, mob_name, timestamp, "
                "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
                "VALUES (?, ?, 'Atrox', ?, 5, 50.0, 0.5, 2.0)",
                (str(uuid.uuid4()), session_id, now - 30 + i),
            )
        db.commit()
        return {
            "session_id": session_id,
            "from_mob": "Caboria Old",
            "from_mob_count": 3,
            "other_mob": "Atrox",
            "other_mob_count": 2,
        }

    def _seed_summary_row(self, db, session_id):
        """Drop a stand-in session_summaries row so cache-invalidation
        is observable. Columns mirror schema.py:114-136; only the
        primary key matters for the DELETE assertion."""
        db.execute(
            "INSERT INTO session_summaries ("
            "session_id, summary_version, started_at, ended_at, duration_hours, "
            "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
            "dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, "
            "regular_skill_tt, attribute_levels_total"
            ") VALUES (?, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, '{}', '{}', 0, 0)",
            (session_id,),
        )
        db.commit()

    # ── Rename happy paths ────────────────────────────────────────────

    def test_rename_rewrites_matching_kills_and_preserves_original(self):
        """Renaming 'Caboria Old' to 'Argonaut Old' rewrites every
        matching kill, preserves the pre-edit value into
        original_mob_name on each, and leaves non-matching kills
        untouched."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        result = _rename_session_mob_impl(
            db,
            seed["session_id"],
            seed["from_mob"],
            "Argonaut Old",
        )
        assert result["sessionId"] == seed["session_id"]
        assert result["mobName"] == "Argonaut Old"
        assert result["killCount"] == seed["from_mob_count"]

        renamed_rows = db.execute(
            "SELECT mob_name, original_mob_name FROM kills "
            "WHERE session_id = ? AND mob_name = 'Argonaut Old'",
            (seed["session_id"],),
        ).fetchall()
        assert len(renamed_rows) == seed["from_mob_count"]
        for current, original in renamed_rows:
            assert current == "Argonaut Old"
            assert original == "Caboria Old"

        # Non-matching cohort (Atrox) stays untouched, including a NULL
        # original_mob_name.
        untouched = db.execute(
            "SELECT mob_name, original_mob_name FROM kills "
            "WHERE session_id = ? AND mob_name = 'Atrox'",
            (seed["session_id"],),
        ).fetchall()
        assert len(untouched) == seed["other_mob_count"]
        for current, original in untouched:
            assert current == "Atrox"
            assert original is None

    def test_rename_into_existing_mob_reports_post_mutation_total(self):
        """Renaming `from` -> `to` where `to` already has kills in the
        session: the response `killCount` reflects the post-mutation
        total for the destination (affected + pre-existing), not just
        the affected count, so the frontend can re-render the session
        row without a refetch."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        # Rename Caboria Old -> Atrox; Atrox already has 2 kills, so the
        # post-mutation Atrox count is 3 (affected) + 2 (pre-existing) = 5.
        result = _rename_session_mob_impl(
            db,
            seed["session_id"],
            seed["from_mob"],
            "Atrox",
        )
        assert result["mobName"] == "Atrox"
        assert result["killCount"] == seed["from_mob_count"] + seed["other_mob_count"]

        # And verify the underlying DB matches.
        actual = db.execute(
            "SELECT COUNT(*) FROM kills WHERE session_id = ? AND mob_name = 'Atrox'",
            (seed["session_id"],),
        ).fetchone()[0]
        assert actual == seed["from_mob_count"] + seed["other_mob_count"]

    def test_rename_back_to_original_clears_preservation_column(self):
        """A round-trip rename (A -> B -> A) lands at the original name
        with the preservation column cleared, so the rows look identical
        to never-renamed rows. Without this, mobBreakdown would surface
        a bogus 'originally A' indicator and restore-mob would become a
        no-op metadata cleanup."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        _rename_session_mob_impl(db, seed["session_id"], "Caboria Old", "Argonaut Old")
        _rename_session_mob_impl(db, seed["session_id"], "Argonaut Old", "Caboria Old")

        rows = db.execute(
            "SELECT mob_name, original_mob_name FROM kills "
            "WHERE session_id = ? AND mob_name = 'Caboria Old'",
            (seed["session_id"],),
        ).fetchall()
        assert len(rows) == seed["from_mob_count"]
        for current, original in rows:
            assert current == "Caboria Old"
            assert original is None

    def test_rename_preserves_first_original_across_consecutive_renames(self):
        """Renaming A->B then B->C must keep original_mob_name = A
        (COALESCE first-original semantics); undo always lands at the
        genuinely-original capture."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        _rename_session_mob_impl(db, seed["session_id"], "Caboria Old", "Argonaut Old")
        _rename_session_mob_impl(db, seed["session_id"], "Argonaut Old", "Atrox Old")

        rows = db.execute(
            "SELECT mob_name, original_mob_name FROM kills "
            "WHERE session_id = ? AND mob_name = 'Atrox Old'",
            (seed["session_id"],),
        ).fetchall()
        assert len(rows) == seed["from_mob_count"]
        for current, original in rows:
            assert current == "Atrox Old"
            # First original is preserved; second rename did NOT clobber it.
            assert original == "Caboria Old"

    def test_rename_clears_session_summaries_cache(self):
        """Cache invalidation contract: a pre-existing summary row is
        deleted on rename so the next read recomputes."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        self._seed_summary_row(db, seed["session_id"])

        _rename_session_mob_impl(
            db, seed["session_id"], seed["from_mob"], "Argonaut Old"
        )

        row = db.execute(
            "SELECT 1 FROM session_summaries WHERE session_id = ?",
            (seed["session_id"],),
        ).fetchone()
        assert row is None

    # ── Rename 404 / 409 paths ────────────────────────────────────────

    def test_rename_missing_session_is_404(self):
        """Unknown session id yields 404."""
        db = _setup_orphan_db()
        with pytest.raises(HTTPException) as exc:
            _rename_session_mob_impl(db, "no-such-session", "X", "Y")
        assert exc.value.status_code == 404
        assert exc.value.detail == "Session not found"

    def test_rename_blank_input_is_400(self):
        """Whitespace-only mob names are rejected with 400 before any
        DB mutation, so empty strings can't persist into kills.mob_name."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        for from_val, to_val in [
            ("   ", "Argonaut Old"),
            ("Caboria Old", ""),
            ("", ""),
        ]:
            with pytest.raises(HTTPException) as exc:
                _rename_session_mob_impl(db, seed["session_id"], from_val, to_val)
            assert exc.value.status_code == 400
            assert "blank" in exc.value.detail.lower()

    def test_rename_active_session_is_409(self):
        """Renames on a live session create drift between SQLite and
        the tracker's in-memory state; the helper refuses with 409."""
        db = _setup_orphan_db()
        session_id = str(uuid.uuid4())
        now = time.time()
        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
            "VALUES (?, ?, NULL, 1)",
            (session_id, now - 60),
        )
        db.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, "
            "shots_fired, damage_dealt, loot_total_ped, cost_ped) "
            "VALUES (?, ?, 'Caboria Old', ?, 5, 50.0, 1.0, 2.0)",
            (str(uuid.uuid4()), session_id, now - 30),
        )
        db.commit()

        with pytest.raises(HTTPException) as exc:
            _rename_session_mob_impl(db, session_id, "Caboria Old", "Argonaut Old")
        assert exc.value.status_code == 409
        assert "after the session has ended" in exc.value.detail.lower()

    def test_rename_no_matching_kills_is_409(self):
        """from_mob value with no matching kills in the session is a
        409 rather than a silent no-op (the request expressed an
        intention against zero rows)."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        with pytest.raises(HTTPException) as exc:
            _rename_session_mob_impl(
                db, seed["session_id"], "Nonexistent", "Argonaut Old"
            )
        assert exc.value.status_code == 409
        assert "no kills" in exc.value.detail.lower()

    def test_rename_noop_same_name_is_409(self):
        """from == to is a 409 (silent success would leave the cache
        invalidated for no reason, plus signals client confusion)."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        with pytest.raises(HTTPException) as exc:
            _rename_session_mob_impl(
                db, seed["session_id"], seed["from_mob"], seed["from_mob"]
            )
        assert exc.value.status_code == 409
        assert "no-op" in exc.value.detail.lower()

    # ── Restore happy paths ───────────────────────────────────────────

    def test_restore_reverts_to_original_and_clears_preservation(self):
        """Restore inverts the rename: mob_name returns to the preserved
        original_mob_name, and the preservation column clears."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        _rename_session_mob_impl(db, seed["session_id"], "Caboria Old", "Argonaut Old")
        result = _restore_session_mob_impl(db, seed["session_id"], "Argonaut Old")
        assert result["sessionId"] == seed["session_id"]
        assert result["mobName"] == "Caboria Old"
        assert result["killCount"] == seed["from_mob_count"]

        # Restored kills carry the original name with the preservation
        # column cleared, ready for a fresh rename if needed.
        rows = db.execute(
            "SELECT mob_name, original_mob_name FROM kills "
            "WHERE session_id = ? AND mob_name = 'Caboria Old'",
            (seed["session_id"],),
        ).fetchall()
        assert len(rows) == seed["from_mob_count"]
        for current, original in rows:
            assert current == "Caboria Old"
            assert original is None

    def test_restore_after_consecutive_renames_lands_at_first_original(self):
        """A single restore after two renames jumps back to the
        first-captured name (consistent with COALESCE preservation)."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        _rename_session_mob_impl(db, seed["session_id"], "Caboria Old", "Argonaut Old")
        _rename_session_mob_impl(db, seed["session_id"], "Argonaut Old", "Atrox Old")
        result = _restore_session_mob_impl(db, seed["session_id"], "Atrox Old")
        assert result["mobName"] == "Caboria Old"
        assert result["killCount"] == seed["from_mob_count"]

    def test_restore_clears_session_summaries_cache(self):
        """Same cache-invalidation contract on the restore path."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        _rename_session_mob_impl(db, seed["session_id"], "Caboria Old", "Argonaut Old")

        self._seed_summary_row(db, seed["session_id"])
        _restore_session_mob_impl(db, seed["session_id"], "Argonaut Old")

        row = db.execute(
            "SELECT 1 FROM session_summaries WHERE session_id = ?",
            (seed["session_id"],),
        ).fetchone()
        assert row is None

    # ── Restore 404 / 409 paths ───────────────────────────────────────

    def test_restore_missing_session_is_404(self):
        """Unknown session id yields 404."""
        db = _setup_orphan_db()
        with pytest.raises(HTTPException) as exc:
            _restore_session_mob_impl(db, "no-such-session", "Argonaut Old")
        assert exc.value.status_code == 404

    def test_restore_blank_input_is_400(self):
        """Whitespace-only current_mob is rejected with 400 before any
        DB query, distinct from a 409 'nothing matches' which would
        otherwise mislead callers about what went wrong."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        with pytest.raises(HTTPException) as exc:
            _restore_session_mob_impl(db, seed["session_id"], "   ")
        assert exc.value.status_code == 400
        assert "blank" in exc.value.detail.lower()

    def test_restore_active_session_is_409(self):
        """Restores on a live session create the same drift hazard as
        renames; the same guard refuses with 409."""
        db = _setup_orphan_db()
        session_id = str(uuid.uuid4())
        now = time.time()
        db.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
            "VALUES (?, ?, NULL, 1)",
            (session_id, now - 60),
        )
        db.commit()
        with pytest.raises(HTTPException) as exc:
            _restore_session_mob_impl(db, session_id, "Argonaut Old")
        assert exc.value.status_code == 409
        assert "after the session has ended" in exc.value.detail.lower()

    def test_restore_no_eligible_kills_is_409(self):
        """No matching kills (either the current name never existed or
        the preservation column is empty) yields 409."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)
        # Atrox kills were never renamed, so their original_mob_name is
        # NULL. Asking to restore them is a 409.
        with pytest.raises(HTTPException) as exc:
            _restore_session_mob_impl(db, seed["session_id"], "Atrox")
        assert exc.value.status_code == 409
        assert "no restorable" in exc.value.detail.lower()

    def test_restore_ambiguous_when_two_originals_merged_is_409(self):
        """If two distinct prior names were renamed into the same
        current name (A->C, then B->C), the restore endpoint cannot
        unambiguously split the cohort back into A vs B with a
        single-result response shape. Refuse with 409 rather than
        arbitrarily picking one original."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        # Rename Caboria Old -> Argonaut Old, then Atrox -> Argonaut Old.
        # Now Argonaut Old has 5 kills with 2 distinct original_mob_name
        # values.
        _rename_session_mob_impl(db, seed["session_id"], "Caboria Old", "Argonaut Old")
        _rename_session_mob_impl(db, seed["session_id"], "Atrox", "Argonaut Old")

        with pytest.raises(HTTPException) as exc:
            _restore_session_mob_impl(db, seed["session_id"], "Argonaut Old")
        assert exc.value.status_code == 409
        assert "ambiguous" in exc.value.detail.lower()

        # And the kills stay at the post-merge state: nothing got partially
        # restored.
        merged = db.execute(
            "SELECT COUNT(*) FROM kills WHERE session_id = ? AND mob_name = 'Argonaut Old'",
            (seed["session_id"],),
        ).fetchone()[0]
        assert merged == seed["from_mob_count"] + seed["other_mob_count"]

    # ── Atomicity ─────────────────────────────────────────────────────

    def test_rename_rollback_on_mid_transaction_failure(self):
        """A SQL error mid-transaction reverts both UPDATE statements
        (preservation + rename) so the kill rows stay at their pre-edit
        state. Injects a failure on the rename UPDATE specifically (by
        SQL content rather than ordinal call count, so harmless internal
        query refactors don't break the test), landing mid-transaction
        after the preservation UPDATE has already executed."""
        db = _setup_orphan_db()
        seed = self._seed_mixed_mob_session(db)

        class FailingConn:
            def __init__(self, real, fail_on_sql_contains):
                self._real = real
                self._fail_on_sql_contains = fail_on_sql_contains

            def execute(self, *args, **kwargs):
                sql = args[0] if args else ""
                if isinstance(sql, str) and self._fail_on_sql_contains in sql:
                    raise sqlite3.OperationalError("simulated failure mid-transaction")
                return self._real.execute(*args, **kwargs)

            def commit(self):
                return self._real.commit()

            def rollback(self):
                return self._real.rollback()

        wrapped = FailingConn(db, fail_on_sql_contains="UPDATE kills SET mob_name")
        with pytest.raises(sqlite3.OperationalError):
            _rename_session_mob_impl(
                wrapped, seed["session_id"], "Caboria Old", "Argonaut Old"
            )

        # Post-rollback: nothing renamed, nothing preserved.
        rows = db.execute(
            "SELECT mob_name, original_mob_name FROM kills "
            "WHERE session_id = ? AND mob_name = 'Caboria Old'",
            (seed["session_id"],),
        ).fetchall()
        assert len(rows) == seed["from_mob_count"]
        for current, original in rows:
            assert current == "Caboria Old"
            assert original is None

    # ── Session-detail response extension ─────────────────────────────

    def test_session_detail_carries_mob_breakdown_with_original_name(self, tmp_path):
        """GET /api/tracking/session/{id} surfaces per-mob breakdown
        including current + original names so the frontend can render
        an 'originally X' indicator on renamed mobs."""
        from backend.db.app_database import AppDatabase

        app_db = AppDatabase(tmp_path / "app.db")
        db = app_db.conn
        init_tracking_tables(db)
        try:
            seed = self._seed_mixed_mob_session(db)
            _rename_session_mob_impl(
                db, seed["session_id"], "Caboria Old", "Argonaut Old"
            )

            detail = get_session_impl(db, seed["session_id"])
        finally:
            app_db.close()

        breakdown = detail["mobBreakdown"]
        assert len(breakdown) == 2

        # API contract: sorted by killCount descending. Pin explicitly
        # so reordering regressions surface here rather than as
        # frontend display bugs.
        assert [row["currentName"] for row in breakdown] == ["Argonaut Old", "Atrox"]
        assert [row["killCount"] for row in breakdown] == [
            seed["from_mob_count"],
            seed["other_mob_count"],
        ]

        by_current = {row["currentName"]: row for row in breakdown}
        renamed = by_current["Argonaut Old"]
        assert renamed["originalName"] == "Caboria Old"
        assert renamed["killCount"] == seed["from_mob_count"]

        untouched = by_current["Atrox"]
        assert untouched["originalName"] is None
        assert untouched["killCount"] == seed["other_mob_count"]


# ── V31 migration: kills.original_mob_name ───────────────────────────


class TestV31Migration:
    """The version-counter migration that lands `original_mob_name` on
    `kills` for existing DBs (v30 to v31 forward-migrate). Fresh
    installs land the column via the canonical tracking schema; this
    class pins the in-place upgrade path and its defensive cases."""

    def test_upgrade_existing_v30_kills(self, tmp_path):
        """A v30-shaped DB with `kills` present picks up
        `original_mob_name` on AppDatabase open."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        # Stand up a v30-shaped DB by hand: app metadata at version 30,
        # `kills` present without the new column.
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '30')")
        seed.execute(
            "CREATE TABLE kills ("
            "  id TEXT PRIMARY KEY,"
            "  session_id TEXT NOT NULL,"
            "  mob_name TEXT,"
            "  timestamp REAL NOT NULL,"
            "  loot_total_ped REAL DEFAULT 0"
            ")"
        )
        seed.commit()
        seed.close()

        app_db = AppDatabase(db_path)
        try:
            cols = {
                row[1]
                for row in app_db.conn.execute("PRAGMA table_info(kills)").fetchall()
            }
            assert "original_mob_name" in cols

            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_without_kills_table(self, tmp_path):
        """A v30 install that never started tracking has no `kills`
        table; the v31 migration tolerates that and the column will land
        via init_tracking_tables on first Tracker start. Defensive-path
        coverage matching the v30 migration's shape."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '30')")
        seed.commit()
        seed.close()

        # Should not raise.
        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_idempotent_when_column_already_present(self, tmp_path):
        """Partial-run safety: the column already exists, and the
        migration's duplicate-column branch swallows the error."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '30')")
        seed.execute(
            "CREATE TABLE kills ("
            "  id TEXT PRIMARY KEY,"
            "  session_id TEXT NOT NULL,"
            "  mob_name TEXT,"
            "  timestamp REAL NOT NULL,"
            "  loot_total_ped REAL DEFAULT 0,"
            "  original_mob_name TEXT"
            ")"
        )
        seed.commit()
        seed.close()

        # Should not raise.
        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()


# ── V32 migration: tracking_sessions.mob_tracking_mode ──────────────


class TestV32Migration:
    """The version-counter migration that lands mob_tracking_mode on
    tracking_sessions for existing DBs (v31 to v32 forward-migrate).
    Fresh installs land the column via the canonical tracking schema;
    this class pins the in-place upgrade path and its defensive cases.

    The column is NOT NULL with DEFAULT 'mob', so pre-migration rows
    surface as mob-mode after the migration. This is the deliberate
    choice: undocumented tag-mode usage before this migration loses its
    mode flavour cosmetically; the underlying data is unaffected (the
    tag string is persisted into kills.mob_name in tag-mode sessions
    just as it is in mob-mode sessions).
    """

    def test_upgrade_existing_v31_tracking_sessions(self, tmp_path):
        """A v31-shaped DB with tracking_sessions present picks up
        mob_tracking_mode on AppDatabase open; pre-migration rows
        default to 'mob' per the column's DEFAULT clause."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '31')")
        seed.execute(
            "CREATE TABLE tracking_sessions ("
            "  id TEXT PRIMARY KEY,"
            "  started_at REAL NOT NULL,"
            "  ended_at REAL,"
            "  is_active INTEGER NOT NULL DEFAULT 1"
            ")"
        )
        pre_session_id = "pre-migration-session"
        seed.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
            "VALUES (?, ?, ?, 0)",
            (pre_session_id, time.time() - 600, time.time()),
        )
        seed.commit()
        seed.close()

        app_db = AppDatabase(db_path)
        try:
            cols = {
                row[1]
                for row in app_db.conn.execute(
                    "PRAGMA table_info(tracking_sessions)"
                ).fetchall()
            }
            assert "mob_tracking_mode" in cols

            mode = app_db.conn.execute(
                "SELECT mob_tracking_mode FROM tracking_sessions WHERE id = ?",
                (pre_session_id,),
            ).fetchone()[0]
            assert mode == "mob"

            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_without_tracking_sessions_table(self, tmp_path):
        """A v31 install that never started tracking has no
        tracking_sessions table; the migration tolerates that and the
        column will land via init_tracking_tables on first Tracker
        start. Defensive-path coverage matching the v30 / v31 shape."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '31')")
        seed.commit()
        seed.close()

        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_idempotent_when_column_already_present(self, tmp_path):
        """Partial-run safety: the column already exists, and the
        migration's duplicate-column branch swallows the error."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '31')")
        seed.execute(
            "CREATE TABLE tracking_sessions ("
            "  id TEXT PRIMARY KEY,"
            "  started_at REAL NOT NULL,"
            "  ended_at REAL,"
            "  is_active INTEGER NOT NULL DEFAULT 1,"
            "  mob_tracking_mode TEXT NOT NULL DEFAULT 'mob'"
            ")"
        )
        seed.commit()
        seed.close()

        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()


class TestV33Migration:
    """The version-counter migration that drops the unused
    tt_curve_observations table for existing DBs (v32 to v33
    forward-migrate). The table was write-only in the v32 surface (the
    skill tracker recorded a row on every suppressed codex skill gain,
    but no read path consumed it); the cross-check it backed is retired
    now that the TT value curve is trusted. Fresh installs simply never
    create the table; this class pins the in-place drop path.
    """

    def test_upgrade_existing_v32_drops_tt_curve_observations(self, tmp_path):
        """A v32-shaped DB carrying tt_curve_observations has the table
        dropped on AppDatabase open."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '32')")
        seed.execute(
            "CREATE TABLE tt_curve_observations ("
            "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
            "  skill_name TEXT NOT NULL,"
            "  from_level REAL NOT NULL,"
            "  level_gain REAL NOT NULL,"
            "  known_ped REAL NOT NULL,"
            "  curve_ped REAL NOT NULL,"
            "  deviation REAL NOT NULL,"
            "  source TEXT NOT NULL DEFAULT 'codex',"
            "  observed_at REAL NOT NULL DEFAULT (unixepoch('now'))"
            ")"
        )
        seed.execute(
            "INSERT INTO tt_curve_observations "
            "(skill_name, from_level, level_gain, known_ped, curve_ped, deviation) "
            "VALUES ('Aim', 500.0, 0.1, 0.5, 0.48, 0.02)"
        )
        seed.commit()
        seed.close()

        app_db = AppDatabase(db_path)
        try:
            tables = {
                row[0]
                for row in app_db.conn.execute(
                    "SELECT name FROM sqlite_master WHERE type = 'table'"
                ).fetchall()
            }
            assert "tt_curve_observations" not in tables

            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()

    def test_upgrade_without_tt_curve_observations_table(self, tmp_path):
        """A v32 DB that never created tt_curve_observations migrates
        cleanly; DROP TABLE IF EXISTS tolerates the absent table."""
        from backend.db.app_database import AppDatabase

        db_path = tmp_path / "app.db"
        seed = sqlite3.connect(str(db_path))
        seed.execute(
            "CREATE TABLE db_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
        )
        seed.execute("INSERT INTO db_metadata (key, value) VALUES ('version', '32')")
        seed.commit()
        seed.close()

        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 33
        finally:
            app_db.close()


# ── Tracker + response: mob_tracking_mode at session start ──────────


class TestMobTrackingModePersistence:
    """The mob_tracking_mode column is written at session start by
    HuntTracker and surfaced through get_session_impl as mobEntryMode.
    Pins the round-trip from tracker write through the API contract."""

    def test_fresh_install_has_mob_tracking_mode_column(self):
        """init_tracking_tables on a fresh DB produces the column with
        the canonical default."""
        db = _setup_orphan_db()
        cols = {
            row[1]
            for row in db.execute("PRAGMA table_info(tracking_sessions)").fetchall()
        }
        assert "mob_tracking_mode" in cols

    def test_tracker_persists_mob_mode_at_session_start(self):
        """A tracker initialised with a 'mob' mode provider writes 'mob'
        to tracking_sessions.mob_tracking_mode at start_session."""
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "mob",
        )
        session = tracker.start_session()
        try:
            mode = db.execute(
                "SELECT mob_tracking_mode FROM tracking_sessions WHERE id = ?",
                (session.id,),
            ).fetchone()[0]
            assert mode == "mob"
        finally:
            tracker.stop_session()
            db.close()

    def test_tracker_persists_tag_mode_at_session_start(self):
        """A tracker initialised with a 'tag' mode provider writes 'tag'
        to tracking_sessions.mob_tracking_mode at start_session."""
        db = sqlite3.connect(":memory:", check_same_thread=False)
        bus = EventBus()
        tracker = HuntTracker(
            bus,
            db,
            mob_tracking_mode_provider=lambda: "tag",
        )
        session = tracker.start_session()
        try:
            mode = db.execute(
                "SELECT mob_tracking_mode FROM tracking_sessions WHERE id = ?",
                (session.id,),
            ).fetchone()[0]
            assert mode == "tag"
        finally:
            tracker.stop_session()
            db.close()

    def test_session_detail_response_surfaces_mob_mode(self, tmp_path):
        """GET /api/tracking/session/{id} carries mobEntryMode='mob' for
        a session written in mob mode."""
        from backend.db.app_database import AppDatabase

        app_db = AppDatabase(tmp_path / "app.db")
        db = app_db.conn
        init_tracking_tables(db)
        try:
            session_id = str(uuid.uuid4())
            now = time.time()
            db.execute(
                "INSERT INTO tracking_sessions "
                "(id, started_at, ended_at, is_active, mob_tracking_mode) "
                "VALUES (?, ?, ?, 0, 'mob')",
                (session_id, now - 60, now),
            )
            db.commit()
            detail = get_session_impl(db, session_id)
        finally:
            app_db.close()
        assert detail["mobEntryMode"] == "mob"

    def test_session_detail_response_surfaces_tag_mode(self, tmp_path):
        """GET /api/tracking/session/{id} carries mobEntryMode='tag' for
        a session written in tag mode."""
        from backend.db.app_database import AppDatabase

        app_db = AppDatabase(tmp_path / "app.db")
        db = app_db.conn
        init_tracking_tables(db)
        try:
            session_id = str(uuid.uuid4())
            now = time.time()
            db.execute(
                "INSERT INTO tracking_sessions "
                "(id, started_at, ended_at, is_active, mob_tracking_mode) "
                "VALUES (?, ?, ?, 0, 'tag')",
                (session_id, now - 60, now),
            )
            db.commit()
            detail = get_session_impl(db, session_id)
        finally:
            app_db.close()
        assert detail["mobEntryMode"] == "tag"
