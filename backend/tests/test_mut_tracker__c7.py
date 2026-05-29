"""Mutation-hardening tests for cluster tracker__c7.

Targets three HuntTracker methods:
  * _create_shrapnel_ledger_entry
  * _create_enhancer_rebate_ledger_entry
  * _on_combat

The tests drive the real ``backend.tracking.tracker`` against an in-memory
SQLite database and an in-process event bus, exercising the exact lines the
surviving mutants alter and asserting the behaviour each mutation breaks.
"""

import sqlite3
import uuid
from datetime import datetime, timedelta

import pytest

from backend.core.event_bus import EventBus
from backend.tracking.schema import init_tracking_tables
from backend.tracking.tracker import HuntTracker, _Accumulator


# ---------------------------------------------------------------------------
# Fixtures / helpers
# ---------------------------------------------------------------------------


def _make_tracker(**kwargs) -> tuple[HuntTracker, sqlite3.Connection]:
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db, **kwargs)
    return tracker, db


def _seed_session(db: sqlite3.Connection, session_id: str) -> None:
    db.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES (?, ?, 0)",
        (session_id, 1000.0),
    )


def _seed_kill(db: sqlite3.Connection, session_id: str, kill_id: str) -> None:
    db.execute(
        "INSERT INTO kills (id, session_id, timestamp) VALUES (?, ?, ?)",
        (kill_id, session_id, 1000.0),
    )


def _seed_loot_item(
    db: sqlite3.Connection,
    kill_id: str,
    *,
    item_name: str,
    value_ped: float,
    is_enhancer_shrapnel: int = 0,
    deactivated_at: float | None = None,
) -> None:
    db.execute(
        "INSERT INTO kill_loot_items "
        "(kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel, deactivated_at) "
        "VALUES (?, ?, 1, ?, ?, ?)",
        (kill_id, item_name, value_ped, is_enhancer_shrapnel, deactivated_at),
    )


def _ledger_rows(db: sqlite3.Connection, tag: str) -> list[tuple]:
    return db.execute(
        "SELECT id, date, type, description, amount, tag FROM ledger_entries "
        "WHERE tag = ?",
        (tag,),
    ).fetchall()


# ===========================================================================
# _create_shrapnel_ledger_entry
# ===========================================================================


class TestShrapnelLedgerEntry:
    def _setup(self, value_ped: float):
        tracker, db = _make_tracker()
        sid = "sess-shrap"
        kid = "kill-shrap"
        _seed_session(db, sid)
        _seed_kill(db, sid, kid)
        _seed_loot_item(db, kid, item_name="Shrapnel", value_ped=value_ped)
        return tracker, db, sid

    def test_shrapnel_below_one_ped_still_creates_entry(self):
        # Kills `_28` (<= 0 -> <= 1): a sub-PED positive shrapnel value must
        # still produce a conversion entry.
        tracker, db, sid = self._setup(0.5)
        tracker._create_shrapnel_ledger_entry(sid, datetime(2024, 1, 1, 12, 0, 0))
        rows = _ledger_rows(db, "convert")
        assert len(rows) == 1

    def test_shrapnel_margin_is_exactly_one_percent_rounded_to_four_dp(self):
        # Kills `_36` (round 4 -> 5): value chosen so the margin differs at the
        # fifth decimal place. 12.3456 * 0.01 = 0.123456 -> 0.1235 at 4dp,
        # 0.12346 at 5dp.
        tracker, db, sid = self._setup(12.3456)
        tracker._create_shrapnel_ledger_entry(sid, datetime(2024, 1, 1, 12, 0, 0))
        rows = _ledger_rows(db, "convert")
        assert len(rows) == 1
        amount = rows[0][4]
        assert amount == pytest.approx(0.1235)
        # And explicitly not the 5dp value the mutant would store.
        assert amount != pytest.approx(0.12346)

    def test_shrapnel_entry_id_is_a_valid_unique_uuid(self):
        # Kills `_38` (entry_id = None) and `_39` (entry_id = str(None)).
        tracker, db, sid = self._setup(100.0)
        tracker._create_shrapnel_ledger_entry(sid, datetime(2024, 1, 1, 12, 0, 0))
        rows = _ledger_rows(db, "convert")
        assert len(rows) == 1
        entry_id = rows[0][0]
        assert entry_id is not None
        assert entry_id != "None"
        # Must parse as a UUID (rejects both None and the literal "None").
        uuid.UUID(entry_id)

    def test_shrapnel_entry_core_fields(self):
        # Pins type/description/tag (defends the INSERT column layout).
        tracker, db, sid = self._setup(100.0)
        tracker._create_shrapnel_ledger_entry(sid, datetime(2024, 1, 1, 12, 0, 0))
        rows = _ledger_rows(db, "convert")
        assert len(rows) == 1
        _id, date, typ, desc, amount, tag = rows[0]
        assert typ == "markup"
        assert desc == "Shrapnel Conversion"
        assert tag == "convert"
        assert date == datetime(2024, 1, 1, 12, 0, 0).isoformat()
        assert amount == pytest.approx(1.0)

    def test_shrapnel_query_filters_enhancer_and_deactivated_and_name(self):
        # Locks the WHERE clause semantics so the SQL stays correct: only
        # active, non-enhancer 'Shrapnel' rows contribute.
        tracker, db = _make_tracker()
        sid = "sess-filter"
        kid = "kill-filter"
        _seed_session(db, sid)
        _seed_kill(db, sid, kid)
        _seed_loot_item(db, kid, item_name="Shrapnel", value_ped=50.0)
        # Enhancer shrapnel: excluded.
        _seed_loot_item(
            db, kid, item_name="Shrapnel", value_ped=999.0, is_enhancer_shrapnel=1
        )
        # Deactivated: excluded.
        _seed_loot_item(
            db, kid, item_name="Shrapnel", value_ped=999.0, deactivated_at=5.0
        )
        # Different item name: excluded.
        _seed_loot_item(db, kid, item_name="Animal Oil Residue", value_ped=999.0)
        tracker._create_shrapnel_ledger_entry(sid, datetime(2024, 1, 1, 12, 0, 0))
        rows = _ledger_rows(db, "convert")
        assert len(rows) == 1
        # Only the 50.0 active non-enhancer Shrapnel row -> margin 0.5.
        assert rows[0][4] == pytest.approx(0.5)

    def test_shrapnel_zero_value_creates_no_entry(self):
        # Pins the early-return guard at the 0 boundary.
        tracker, db, sid = self._setup(0.0)
        tracker._create_shrapnel_ledger_entry(sid, datetime(2024, 1, 1, 12, 0, 0))
        assert _ledger_rows(db, "convert") == []


# ===========================================================================
# _create_enhancer_rebate_ledger_entry
# ===========================================================================


class TestEnhancerRebateLedgerEntry:
    def _setup(self, value_ped: float):
        tracker, db = _make_tracker()
        sid = "sess-reb"
        kid = "kill-reb"
        _seed_session(db, sid)
        _seed_kill(db, sid, kid)
        _seed_loot_item(
            db, kid, item_name="Shrapnel", value_ped=value_ped, is_enhancer_shrapnel=1
        )
        return tracker, db, sid

    def test_enhancer_rebate_entry_id_is_valid_unique_uuid(self):
        # Kills `_26` (entry_id = None) and `_27` (entry_id = str(None)).
        tracker, db, sid = self._setup(10.0)
        tracker._create_enhancer_rebate_ledger_entry(
            sid, datetime(2024, 1, 1, 12, 0, 0)
        )
        rows = _ledger_rows(db, "enhancer")
        assert len(rows) == 1
        entry_id = rows[0][0]
        assert entry_id is not None
        assert entry_id != "None"
        uuid.UUID(entry_id)

    def test_enhancer_rebate_type_is_markup(self):
        # Kills `_37` ("markup" -> "XXmarkupXX") and `_38` ("markup" -> "MARKUP").
        tracker, db, sid = self._setup(10.0)
        tracker._create_enhancer_rebate_ledger_entry(
            sid, datetime(2024, 1, 1, 12, 0, 0)
        )
        rows = _ledger_rows(db, "enhancer")
        assert len(rows) == 1
        assert rows[0][2] == "markup"

    def test_enhancer_rebate_amount_rounded_to_four_dp(self):
        # Kills `_46` (round 4 -> 5). 1.234567 -> 1.2346 at 4dp.
        tracker, db, sid = self._setup(1.234567)
        tracker._create_enhancer_rebate_ledger_entry(
            sid, datetime(2024, 1, 1, 12, 0, 0)
        )
        rows = _ledger_rows(db, "enhancer")
        assert len(rows) == 1
        amount = rows[0][4]
        assert amount == pytest.approx(1.2346)
        assert amount != pytest.approx(1.23457)

    def test_enhancer_rebate_core_fields(self):
        tracker, db, sid = self._setup(10.0)
        tracker._create_enhancer_rebate_ledger_entry(
            sid, datetime(2024, 1, 1, 12, 0, 0)
        )
        rows = _ledger_rows(db, "enhancer")
        assert len(rows) == 1
        _id, date, typ, desc, amount, tag = rows[0]
        assert desc == "Enhancer Shrapnel Rebate"
        assert tag == "enhancer"
        assert date == datetime(2024, 1, 1, 12, 0, 0).isoformat()
        assert amount == pytest.approx(10.0)

    def test_enhancer_rebate_query_only_counts_enhancer_active_rows(self):
        # Only is_enhancer_shrapnel=1, active rows feed the rebate.
        tracker, db = _make_tracker()
        sid = "sess-reb2"
        kid = "kill-reb2"
        _seed_session(db, sid)
        _seed_kill(db, sid, kid)
        _seed_loot_item(
            db, kid, item_name="Shrapnel", value_ped=7.0, is_enhancer_shrapnel=1
        )
        # Plain (non-enhancer) shrapnel: excluded from the rebate.
        _seed_loot_item(db, kid, item_name="Shrapnel", value_ped=999.0)
        # Deactivated enhancer shrapnel: excluded.
        _seed_loot_item(
            db,
            kid,
            item_name="Shrapnel",
            value_ped=999.0,
            is_enhancer_shrapnel=1,
            deactivated_at=5.0,
        )
        tracker._create_enhancer_rebate_ledger_entry(
            sid, datetime(2024, 1, 1, 12, 0, 0)
        )
        rows = _ledger_rows(db, "enhancer")
        assert len(rows) == 1
        assert rows[0][4] == pytest.approx(7.0)

    def test_enhancer_rebate_zero_creates_no_entry(self):
        tracker, db, sid = self._setup(0.0)
        tracker._create_enhancer_rebate_ledger_entry(
            sid, datetime(2024, 1, 1, 12, 0, 0)
        )
        assert _ledger_rows(db, "enhancer") == []


# ===========================================================================
# _on_combat
# ===========================================================================


def _combat_ready_tracker(**kwargs):
    """Tracker primed with a fresh accumulator + session heal/warning state."""
    tracker, db = _make_tracker(**kwargs)
    tracker._accumulator = _Accumulator()
    tracker._session_heal_cost = 0.0
    tracker._session_warnings = []
    tracker._heal_warning_emitted = False
    tracker._last_heal_time = None
    return tracker, db


class TestOnCombatOffensive:
    def test_damage_dealt_records_shot_and_damage(self):
        tracker, _db = _combat_ready_tracker()
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 12.0, "timestamp": datetime(2024, 1, 1)}
        )
        acc = tracker.current_accumulator
        assert acc.shots_fired == 1
        assert acc.damage_dealt == pytest.approx(12.0)
        assert acc.critical_hits == 0

    def test_critical_hit_increments_critical_count(self):
        # Pins is_crit=(event_type == "critical_hit").
        tracker, _db = _combat_ready_tracker()
        tracker._on_combat(
            {"type": "critical_hit", "amount": 20.0, "timestamp": datetime(2024, 1, 1)}
        )
        acc = tracker.current_accumulator
        assert acc.shots_fired == 1
        assert acc.damage_dealt == pytest.approx(20.0)
        assert acc.critical_hits == 1

    def test_damage_dealt_uses_zero_amount_default(self):
        # Kills `_12`/`_14` (amount default None -> TypeError on `amount > 0`)
        # and `_17` (default 1.0). With no "amount" key the default must be the
        # falsy 0.0: the shot is recorded but no damage accrues.
        tracker, _db = _combat_ready_tracker()
        tracker._on_combat({"type": "damage_dealt", "timestamp": datetime(2024, 1, 1)})
        acc = tracker.current_accumulator
        assert acc.shots_fired == 1
        assert acc.damage_dealt == pytest.approx(0.0)

    def test_dodge_records_a_zero_damage_shot(self):
        # target_dodge/evade/jam -> a recorded shot, no damage, no crit.
        tracker, _db = _combat_ready_tracker()
        tracker._on_combat(
            {"type": "target_dodge", "amount": 5.0, "timestamp": datetime(2024, 1, 1)}
        )
        acc = tracker.current_accumulator
        assert acc.shots_fired == 1
        assert acc.damage_dealt == pytest.approx(0.0)
        assert acc.critical_hits == 0

    def test_damage_received_accumulates_into_damage_taken(self):
        tracker, _db = _combat_ready_tracker()
        tracker._on_combat(
            {
                "type": "damage_received",
                "amount": 8.0,
                "timestamp": datetime(2024, 1, 1),
            }
        )
        acc = tracker.current_accumulator
        assert acc.damage_taken == pytest.approx(8.0)
        # Defensive events are not shots.
        assert acc.shots_fired == 0

    def test_unknown_event_type_is_ignored(self):
        # Kills `_4`/`_6`/`_9` only indirectly; primarily defends dispatch:
        # an event whose type matches nothing leaves the accumulator pristine.
        tracker, _db = _combat_ready_tracker()
        tracker._on_combat(
            {"type": "something_else", "amount": 9.0, "timestamp": datetime(2024, 1, 1)}
        )
        acc = tracker.current_accumulator
        assert acc.shots_fired == 0
        assert acc.damage_dealt == 0.0
        assert acc.damage_taken == 0.0


def _trifecta_tracker():
    """Tracker in weapon-attribution-trifecta mode with two weapon profiles."""
    tracker, db = _combat_ready_tracker(
        weapon_attribution_trifecta_provider=lambda: True
    )
    tracker._damage_attributor.add_weapon_profile(
        name="Big Gun",
        min_damage=90.0,
        max_damage=110.0,
        base_damage=100.0,
        cost_per_shot=1.5,
        role="big_weapon",
    )
    tracker._trifecta_unmatched_warning_emitted = False
    return tracker, db


class TestOnCombatTrifectaInference:
    def test_damage_dealt_attributes_tool_in_trifecta_mode(self):
        # Kills `_29` (allow_damage_inference True -> None) and `_36`
        # (True -> False): both reroute attribution to the (empty)
        # last-offensive tool, leaving the shot keyed as "Unknown" instead of
        # the matched weapon.
        tracker, _db = _trifecta_tracker()
        tracker._on_combat(
            {"type": "damage_dealt", "amount": 100.0, "timestamp": datetime(2024, 1, 1)}
        )
        acc = tracker.current_accumulator
        # The matched weapon must own the shot stats.
        assert "Big Gun" in acc.tool_stats
        assert "Unknown" not in acc.tool_stats
        assert tracker._last_offensive_tool_name == "Big Gun"

    def test_dodge_does_not_run_damage_inference_in_trifecta_mode(self):
        # Kills `_52` (dodge branch allow_damage_inference False -> True): with
        # inference wrongly enabled, the zero-damage dodge is fed to the
        # attributor, which misses and emits the trifecta-unmatched warning.
        tracker, _db = _trifecta_tracker()
        tracker._on_combat(
            {"type": "target_dodge", "amount": 0.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._trifecta_unmatched_warning_emitted is False
        assert tracker._session_warnings == []


class TestOnCombatSelfHealDispatch:
    def test_self_heal_adds_heal_cost(self):
        # Kills `_58` (== -> !=), `_59`/`_60` (self_heal literal mutated),
        # `_61` (is_new_heal_activation = None): each prevents the heal branch
        # from running, so the per-use cost is never accrued.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)
        assert tracker._last_heal_time == datetime(2024, 1, 1)

    def test_non_heal_event_does_not_run_heal_branch(self):
        # Companion to `_58` (== -> !=): a damage_received event must NOT enter
        # the heal branch, so heal cost stays zero and last_heal_time unset.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._on_combat(
            {
                "type": "damage_received",
                "amount": 8.0,
                "timestamp": datetime(2024, 1, 1),
            }
        )
        assert tracker._session_heal_cost == pytest.approx(0.0)
        assert tracker._last_heal_time is None

    def test_self_heal_without_timestamp_is_skipped(self):
        # Kills `_18` (timestamp = None), `_19` (data.get(None)),
        # `_20`/`_21` (wrong timestamp key): each nulls out the timestamp, so
        # the `if timestamp:` guard skips heal processing entirely. With a real
        # timestamp the heal IS processed.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        # The real timestamp path processed the heal and recorded the time.
        assert tracker._session_heal_cost == pytest.approx(0.5)
        assert tracker._last_heal_time == datetime(2024, 1, 1)


class TestOnCombatHealDedup:
    def test_second_heal_within_reload_window_is_deduplicated(self):
        # Kills `_64` (timestamp - last -> timestamp + last: datetime+datetime
        # is a TypeError on the 2nd heal), `_65` (>= -> >: boundary changes the
        # dedup decision), and `_87` (last_heal_time = timestamp -> None: dedup
        # state lost so the 2nd heal re-charges). Reload window is 2.5s.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._heal_reload_seconds = 2.5
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        tracker._on_combat({"type": "self_heal", "amount": 50.0, "timestamp": t0})
        # Second tick 1s later -> inside the 2.5s window -> deduplicated.
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": t0 + timedelta(seconds=1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)

    def test_second_heal_after_reload_window_is_a_new_activation(self):
        # Kills `_61` again (None dedup flag) and `_85` (+= -> =) and
        # `_86` (+= -> -=): two genuine activations must accumulate to 1.0.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._heal_reload_seconds = 2.5
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        tracker._on_combat({"type": "self_heal", "amount": 50.0, "timestamp": t0})
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": t0 + timedelta(seconds=5)}
        )
        assert tracker._session_heal_cost == pytest.approx(1.0)

    def test_heal_at_exact_reload_boundary_is_a_new_activation(self):
        # Kills `_65` (>= -> >): a heal exactly `reload` seconds later must be
        # treated as a NEW activation (>=), charging a second use.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._heal_reload_seconds = 2.5
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        tracker._on_combat({"type": "self_heal", "amount": 50.0, "timestamp": t0})
        tracker._on_combat(
            {
                "type": "self_heal",
                "amount": 50.0,
                "timestamp": t0 + timedelta(seconds=2.5),
            }
        )
        assert tracker._session_heal_cost == pytest.approx(1.0)

    def test_first_heal_does_not_raise_and_charges_once(self):
        # Kills `_62` (or -> and) and `_63` (is None -> is not None): both make
        # the first heal evaluate `timestamp - None`, which raises TypeError.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)


class TestOnCombatHealCostGuard:
    def test_zero_cost_heal_tool_charges_nothing(self):
        # Companion guard for the heal-cost branch.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.0
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.0)

    def test_positive_cost_below_one_is_charged(self):
        # Kills `_84` (> 0 -> > 1): a sub-1 PED per-use cost must still charge.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)


class TestOnCombatHealWarning:
    def test_heal_without_tool_emits_a_single_warning(self):
        # Kills `_70` (is None -> is not None), `_71` (not emitted -> emitted),
        # `_72`/`_76` (msg/append None), `_73`/`_74`/`_75` (warning text
        # mutated): with no heal tool equipped the precise warning string must
        # be appended exactly once.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = None
        tracker._heal_cost_per_use_ped = 0.0
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_warnings == [
            "Healing detected \u2014 no heal tool equipped via hotbar"
        ]
        assert tracker._heal_warning_emitted is True

    def test_heal_with_tool_emits_no_warning(self):
        # Kills `_69` (and -> or) and `_70` (is None -> is not None): with a
        # heal tool equipped, no missing-tool warning must be raised.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_warnings == []

    def test_warning_emitted_only_once_across_activations(self):
        # Kills `_71` (not emitted -> emitted), `_77` (emitted = None) and
        # `_78` (emitted = False): the emitted flag must latch True so a second
        # tool-less activation does NOT re-append the warning.
        tracker, _db = _combat_ready_tracker()
        tracker._active_heal_tool_name = None
        tracker._heal_cost_per_use_ped = 0.0
        tracker._heal_reload_seconds = 2.5
        t0 = datetime(2024, 1, 1, 0, 0, 0)
        tracker._on_combat({"type": "self_heal", "amount": 50.0, "timestamp": t0})
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": t0 + timedelta(seconds=5)}
        )
        assert tracker._session_warnings == [
            "Healing detected \u2014 no heal tool equipped via hotbar"
        ]


class TestOnCombatTrifectaHealMatch:
    def _heal_trifecta(self):
        tracker, db = _combat_ready_tracker(
            weapon_attribution_trifecta_provider=lambda: True
        )
        tracker._active_heal_tool_name = "Trifecta FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._heal_amount_min = 40.0
        tracker._heal_amount_max = 60.0
        return tracker, db

    def test_in_range_heal_is_charged_in_trifecta_mode(self):
        # Kills `_67` (not match -> match) and `_68` (matches(amount) ->
        # matches(None) -> TypeError): an in-range heal must be processed and
        # charged.
        tracker, _db = self._heal_trifecta()
        tracker._on_combat(
            {"type": "self_heal", "amount": 50.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)

    def test_out_of_range_heal_is_skipped_in_trifecta_mode(self):
        # Kills `_66` (and -> or): an out-of-range heal must be skipped (no
        # charge) when trifecta heal-amount gating is active.
        tracker, _db = self._heal_trifecta()
        tracker._on_combat(
            {"type": "self_heal", "amount": 500.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.0)

    def test_out_of_range_heal_is_charged_when_not_in_trifecta_mode(self):
        # Companion for `_66` (and -> or): outside trifecta mode the
        # heal-amount gate is inert; the `and` short-circuits to False so the
        # heal is still charged even though the amount is "out of range".
        tracker, _db = _combat_ready_tracker(
            weapon_attribution_trifecta_provider=lambda: False
        )
        tracker._active_heal_tool_name = "FAP"
        tracker._heal_cost_per_use_ped = 0.5
        tracker._heal_amount_min = 40.0
        tracker._heal_amount_max = 60.0
        tracker._on_combat(
            {"type": "self_heal", "amount": 500.0, "timestamp": datetime(2024, 1, 1)}
        )
        assert tracker._session_heal_cost == pytest.approx(0.5)
