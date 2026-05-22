"""Integration tests: full tracking pipeline via event bus.

Tests the kills model: shots accumulate, loot creates kill records,
session end creates dangling cost.
"""

import sqlite3
import time
import uuid
from datetime import datetime, timedelta

import pytest

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_ENHANCER_BREAK,
    EVENT_GLOBAL,
    EVENT_LOOT_GROUP,
)
from backend.tracking.tracker import HuntTracker
from backend.tracking.schema import init_tracking_tables


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
        session = tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.5, "timestamp": now})
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now})
        bus.publish(EVENT_COMBAT, {"type": "critical_hit", "amount": 30.0, "timestamp": now})

        # Accumulator should have stats
        acc = tracker.current_accumulator
        assert acc.shots_fired == 3
        assert acc.damage_dealt == 55.5
        assert acc.critical_hits == 1

        result = tracker.stop_session()
        assert len(result.kills) == 0
        # Dangling cost persisted (weapon cost = 0 since no equipment lookup)
        assert result.dangling_cost == 0.0  # No cost_per_shot configured

    def test_loot_creates_kill(self, pipeline):
        """Damage then loot → kill record created with correct stats."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "type": "loot",
            "items": [
                {"item_name": "Shrapnel", "quantity": 50, "value_ped": 0.50},
                {"item_name": "Animal Oil Residue", "quantity": 3, "value_ped": 0.03},
            ],
            "total_ped": 0.53,
            "timestamp": now,
        })

        result = tracker.stop_session()
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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
            "total_ped": 0.01,
            "timestamp": now,
        })

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
            bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": t})
            bus.publish(EVENT_LOOT_GROUP, {
                "items": [{"item_name": "Shrapnel", "quantity": 10, "value_ped": 0.10}],
                "total_ped": 0.10,
                "timestamp": t + timedelta(seconds=1),
            })

        result = tracker.stop_session()
        assert len(result.kills) == 3

        # Verify DB
        db_kills = db.execute(
            "SELECT id FROM kills WHERE session_id = ?", (result.id,),
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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now})

        result = tracker.stop_session()
        assert len(result.kills) == 0
        assert abs(result.dangling_cost - 1.00) < 1e-6  # 2 shots × 0.50

        # Verify in DB
        row = db.execute(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?", (result.id,),
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
        assert result.dangling_cost == 0.50

        row = db.execute(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?", (result.id,),
        ).fetchone()
        assert abs(row[0] - 0.50) < 1e-6

    def test_blacklisted_loot_filtered(self, pipeline):
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Universal Ammo", "quantity": 100, "value_ped": 1.0}],
            "total_ped": 1.0,
            "timestamp": now,
        })

        result = tracker.stop_session()
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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [
                {"item_name": "Animal Oil Residue", "quantity": 3, "value_ped": 0.03},
                {"item_name": "Shrapnel", "quantity": 5, "value_ped": 0.05},
            ],
            "total_ped": 0.08,
            "timestamp": now,
        })

        result = tracker.stop_session()
        kill = result.kills[0]
        assert len(kill.loot_items) == 1
        assert kill.loot_items[0].item_name == "Shrapnel"
        assert kill.loot_total_ped == 0.05

    def test_unknown_mob_when_no_lock(self, pipeline):
        """Kill gets 'Unknown' mob when no manual mob/tag is set."""
        bus, tracker, db = pipeline
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
            "total_ped": 0.01,
            "timestamp": now,
        })

        result = tracker.stop_session()
        kill = result.kills[0]
        assert kill.mob_name == "Unknown"

    def test_tool_stats_merge_unknown(self, pipeline):
        """Unknown tool stats merge into real tool on detection."""
        bus, tracker, db = pipeline
        tracker.start_session()

        from backend.core.events import EVENT_ACTIVE_TOOL_CHANGED
        now = datetime.now(tz=None)

        # Shots before tool detection go to "Unknown"
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 15.0, "timestamp": now})

        # Tool detected → merge
        bus.publish(EVENT_ACTIVE_TOOL_CHANGED, {"tool_name": "Opalo"})

        # More shots under real tool
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 20.0, "timestamp": now})

        # Loot → creates kill
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Shrapnel", "quantity": 1, "value_ped": 0.01}],
            "total_ped": 0.01,
            "timestamp": now,
        })

        result = tracker.stop_session()
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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [
                {"item_name": "Shrapnel", "quantity": 1000, "value_ped": 10.00},
                {"item_name": "Animal Oil Residue", "quantity": 5, "value_ped": 0.05},
            ],
            "total_ped": 10.05,
            "timestamp": now,
        })

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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_ENHANCER_BREAK, {
            "enhancer_name": "T1 Weapon Damage Enhancer",
            "item_name": "Electric Attack Nanochip 9",
            "remaining": 3,
            "shrapnel_ped": 0.80,
        })
        bus.publish(EVENT_LOOT_GROUP, {
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
        })

        result = tracker.stop_session()

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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Animal Oil Residue", "quantity": 5, "value_ped": 0.05}],
            "total_ped": 0.05,
            "timestamp": now,
        })

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
        loot_cols = {row[1] for row in db.execute("PRAGMA table_info(kill_loot_items)").fetchall()}
        assert "is_enhancer_shrapnel" in loot_cols
        assert "deactivated_at" in loot_cols


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
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": now})
    bus.publish(EVENT_LOOT_GROUP, {
        "items": [{"item_name": "Shrapnel", "quantity": _kill_counter, "value_ped": ped}],
        "total_ped": ped,
        "timestamp": now,
    })


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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 50.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [
                {"item_name": "Shrapnel", "quantity": 5000, "value_ped": 50.00},
                {"item_name": "Blazar Fragment", "quantity": 1, "value_ped": 2.50},
            ],
            "total_ped": 52.50,
            "timestamp": now,
        })

        bus.publish(EVENT_GLOBAL, {
            "type": "global_kill",
            "player": "TestPlayer",
            "creature": "Atrox Provider",
            "value": 52.50,
            "timestamp": now,
        })

        result = tracker.stop_session()
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
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 100.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Shrapnel", "quantity": 10000, "value_ped": 100.00}],
            "total_ped": 100.00,
            "timestamp": now,
        })
        bus.publish(EVENT_GLOBAL, {
            "type": "hof_kill",
            "player": "TestPlayer",
            "creature": "Atrox Stalker",
            "value": 100.00,
            "timestamp": now,
        })

        result = tracker.stop_session()
        kill = result.kills[0]
        assert kill.is_global is True
        assert kill.is_hof is True

    def test_other_player_global_ignored(self, pipeline_with_player):
        bus, tracker, db = pipeline_with_player
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 50.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Shrapnel", "quantity": 5000, "value_ped": 50.00}],
            "total_ped": 50.00,
            "timestamp": now,
        })
        bus.publish(EVENT_GLOBAL, {
            "type": "global_kill",
            "player": "SomeoneElse",
            "creature": "Atrox",
            "value": 50.00,
            "timestamp": now,
        })

        result = tracker.stop_session()
        kill = result.kills[0]
        assert kill.is_global is False

    def test_notable_event_has_kill_id(self, pipeline_with_player):
        """Notable event record has the correct kill_id."""
        bus, tracker, db = pipeline_with_player
        tracker.start_session()

        now = datetime.now(tz=None)
        bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 30.0, "timestamp": now})
        bus.publish(EVENT_LOOT_GROUP, {
            "items": [{"item_name": "Shrapnel", "quantity": 3000, "value_ped": 30.00}],
            "total_ped": 30.00,
            "timestamp": now,
        })
        bus.publish(EVENT_GLOBAL, {
            "type": "global_kill",
            "player": "TestPlayer",
            "creature": "Atrox",
            "value": 30.00,
            "timestamp": now,
        })

        result = tracker.stop_session()
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

        assert db.execute(
            "SELECT is_active FROM tracking_sessions WHERE id = ?", (old_id,),
        ).fetchone()[0] == 0

        session = tracker.start_session()
        assert tracker.is_tracking
        tracker.stop_session()
        assert not tracker.is_tracking


# ── Loot-entry deactivation (sessions-editing recoverability shape) ───
#
# These tests pin the substrate behaviour that the post-hoc loot-entry
# Deactivate/Activate affordance on the analytics → sessions tab relies on:
# the `deactivated_at` nullable timestamp on `kill_loot_items` paired with
# atomic mutation of the denormalised `kills.loot_total_ped`. The API
# surface that exposes these operations to the frontend is a subsequent
# round; this test class verifies the schema substrate plus the SQL
# manoeuvre that the API will wrap.
#
# Operation in full (per `.planning/situational/sessions-editing-recoverability.md`):
#   Deactivate(loot_id):
#     UPDATE kill_loot_items SET deactivated_at = unixepoch('now') WHERE id = ?
#     UPDATE kills SET loot_total_ped = loot_total_ped - <value_ped> WHERE id = <kill_id>
#     (cache invalidation on session_summaries is the API layer's concern)
#   Activate(loot_id): inverse, clearing `deactivated_at` and adding value_ped back.


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
        cols = {row[1]: row for row in db.execute("PRAGMA table_info(kill_loot_items)").fetchall()}
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
            "SELECT loot_total_ped FROM kills WHERE id = ?", (kill_id,),
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
            "SELECT loot_total_ped FROM kills WHERE id = ?", (kill_id,),
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
            "SELECT loot_total_ped FROM kills WHERE id = ?", (kill_id,),
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
        session_id, kill_id, (loot_a_id, _), (loot_a_value, loot_b_value) = (
            self._seed_session_with_loot(db)
        )

        db.execute(
            "UPDATE kill_loot_items SET deactivated_at = ? WHERE id = ?",
            (time.time(), loot_a_id),
        )
        db.commit()

        # Mirror of routers/tracking.py::get_session_impl's loot breakdown
        # query, plus the filter clause the R2 API work will add.
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
        seed.execute(
            "INSERT INTO db_metadata (key, value) VALUES ('version', '29')"
        )
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
            assert int(version) == 30
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
        seed.execute(
            "INSERT INTO db_metadata (key, value) VALUES ('version', '29')"
        )
        seed.commit()
        seed.close()

        # Should not raise.
        app_db = AppDatabase(db_path)
        try:
            version = app_db.conn.execute(
                "SELECT value FROM db_metadata WHERE key = 'version'"
            ).fetchone()[0]
            assert int(version) == 30
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
        seed.execute(
            "INSERT INTO db_metadata (key, value) VALUES ('version', '29')"
        )
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
            assert int(version) == 30
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
            assert int(version) == 30
        finally:
            second.close()
