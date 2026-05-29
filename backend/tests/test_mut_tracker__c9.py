"""Mutation-kill tests for ``HuntTracker._break_matches_active_weapon`` and
``HuntTracker._on_global`` (campaign cluster ``tracker__c9``).

Each test pins the exact observable behaviour a surviving mutant breaks:

* ``_break_matches_active_weapon`` is exercised through a direct call on the
  real instance with a hand-injected ``_DamageEnhancerState`` (the same public
  attributes ``_ensure_weapon_state`` populates). The substring-match contract
  is asserted across the tool-name, observed-name and case-folding paths.
* ``_on_global`` is driven through the production event bus, and the persisted
  ``notable_events`` / ``kills`` rows plus the in-memory ``Kill`` flags are
  asserted. The log-formatting mutants are pinned with ``caplog`` by rendering
  the captured ``LogRecord`` (``getMessage`` re-runs the ``%`` interpolation,
  so a dropped/None arg or a mangled format string is caught).
"""

import logging
import sqlite3
from datetime import datetime, timedelta

from backend.core.event_bus import EventBus
from backend.core.events import EVENT_COMBAT, EVENT_GLOBAL, EVENT_LOOT_GROUP
from backend.tracking.tracker import HuntTracker, _DamageEnhancerState

TRACKER_LOGGER = "backend.tracking.tracker"


def _tracker(player="Me"):
    db = sqlite3.connect(":memory:", check_same_thread=False)
    bus = EventBus()
    tracker = HuntTracker(bus, db, player_name=player)
    return bus, tracker, db


def _loot_group(value, item, ts):
    return {
        "items": [
            {
                "item_name": item,
                "quantity": 1,
                "value_ped": value,
                "is_enhancer_shrapnel": False,
            }
        ],
        "total_ped": value,
        "timestamp": ts,
    }


def _make_kill(bus, ts):
    """Drive one shot + loot group → creates a kill and sets ``_last_kill``."""
    bus.publish(EVENT_COMBAT, {"type": "damage_dealt", "amount": 10.0, "timestamp": ts})
    bus.publish(EVENT_LOOT_GROUP, _loot_group(50.0, "Hide", ts))


def _set_weapon(tracker, tool_name, observed_name):
    """Inject an active damage-enhancer weapon state the way the real tracker
    would after ``_ensure_weapon_state`` - no production seam added."""
    state = _DamageEnhancerState(tool_name=tool_name, props={}, stacks=[100])
    tracker._weapon_enhancer_states[tool_name] = state
    tracker._active_weapon_state_key = tool_name
    tracker._active_weapon_observed_name = observed_name


# ----------------------------------------------------------------------------
# _break_matches_active_weapon
# ----------------------------------------------------------------------------


def test_break_no_state_is_false():
    # mut_1 (state=None), mut_2 (is not None), mut_3 (return True on no state)
    _, tracker, _db = _tracker()
    tracker._active_weapon_state_key = None
    assert tracker._break_matches_active_weapon("Anything") is False
    _db.close()


def test_break_matching_item_with_state_is_true():
    # mut_1/mut_2 (state forced absent → would return False), mut_20 (bool(None))
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "Excalibur Sword", "Excalibur Sword")
    assert tracker._break_matches_active_weapon("Excalibur") is True
    _db.close()


def test_break_empty_item_is_false():
    # mut_4 (if item_name → False on truthy), mut_5 (not item_name → True)
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "Excalibur Sword", "Excalibur Sword")
    assert tracker._break_matches_active_weapon("") is False
    _db.close()


def test_break_nonmatching_item_is_false_not_truthy_passthrough():
    # mut_21 (item_norm or (...) - non-matching item would leak True)
    # mut_4 (truthy item short-circuits to False)
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "Excalibur Sword", "Excalibur Sword")
    assert tracker._break_matches_active_weapon("Hammer") is False
    _db.close()


def test_break_item_normalised_lowercase_and_alnum():
    # mut_6 (item_norm=None), mut_7 (join(None)), mut_8 ("XXXX".join),
    # mut_9 (ch.upper for item)
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "excaliburloaded", "excaliburloaded")
    # Mixed case + punctuation in the item, normalises to "excalibur"
    assert tracker._break_matches_active_weapon("Ex-Cali.Bur!") is True
    _db.close()


def test_break_item_in_tool_substring():
    # mut_23 (A and B), mut_24 (item not in tool)
    _, tracker, _db = _tracker()
    # Observed name left empty so ONLY the tool-name path can satisfy the match
    # (an observed copy of the tool would rescue mut_23 via its own clause).
    _set_weapon(tracker, "Mega Blaster 9000", "")
    # item normalises to "blaster", a substring of "megablaster9000"; tool is
    # NOT a substring of item, and there is no observed-only path.
    assert tracker._break_matches_active_weapon("Blaster") is True
    _db.close()


def test_break_tool_in_item_substring():
    # mut_22 (precedence), mut_25 (tool not in item)
    _, tracker, _db = _tracker()
    # Observed empty so only the tool-in-item path satisfies the match.
    _set_weapon(tracker, "Blaster", "")
    # tool "blaster" is a substring of item "megablaster9000"; item is not a
    # substring of tool.
    assert tracker._break_matches_active_weapon("Mega Blaster 9000") is True
    _db.close()


def test_break_tool_name_normalised_lowercase_and_alnum():
    # mut_10 (tool_norm=None), mut_11 (join(None)), mut_12 ("XXXX".join),
    # mut_13 (ch.upper for tool)
    _, tracker, _db = _tracker()
    # Observed name empty so only the tool-name normalisation path can match
    # (otherwise an observed copy rescues the XXXX-join / upper-case mutants).
    _set_weapon(tracker, "Ex-Cali.Bur!", "")
    assert tracker._break_matches_active_weapon("excalibur") is True
    _db.close()


def test_break_observed_only_match_item_in_observed():
    # mut_14 (observed_norm=None), mut_15 (join(None)), mut_16 ("XXXX".join),
    # mut_17 (ch.upper observed), mut_18 (observed_name and ""), mut_26 (or),
    # mut_28 (item not in observed)
    _, tracker, _db = _tracker()
    # tool name deliberately unrelated; only the observed name matches.
    _set_weapon(tracker, "ZZZUnrelated", "Plasma Rifle Deluxe")
    # item "plasmarifle" is a substring of observed "plasmarifledeluxe"; not of
    # the tool, and observed is not a substring of item.
    assert tracker._break_matches_active_weapon("Plasma Rifle") is True
    _db.close()


def test_break_observed_only_match_observed_in_item():
    # mut_27 (item in observed and observed in item), mut_29 (observed not in item)
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "ZZZUnrelated", "Plasma")
    # observed "plasma" is a substring of item "plasmariflemk2"; item is not a
    # substring of observed.
    assert tracker._break_matches_active_weapon("Plasma Rifle Mk2") is True
    _db.close()


def test_break_no_match_when_observed_empty_and_no_tool_match():
    # mut_18 ((name and "") → observed always ""), mut_26 (observed or (...)),
    # mut_19 (or "XXXX" default leaks a match)
    _, tracker, _db = _tracker()
    # observed name is empty → observed clause must contribute nothing.
    _set_weapon(tracker, "ZZZUnrelated", "")
    assert tracker._break_matches_active_weapon("Plasma Rifle") is False
    _db.close()


def test_break_no_match_when_observed_name_is_none():
    # mut_19 (observed_name or "XXXX") - with a None observed name the default
    # must be the empty string so a "xxxx" item cannot spuriously match.
    _, tracker, _db = _tracker()
    state = _DamageEnhancerState(tool_name="ZZZUnrelated", props={}, stacks=[100])
    tracker._weapon_enhancer_states["ZZZUnrelated"] = state
    tracker._active_weapon_state_key = "ZZZUnrelated"
    tracker._active_weapon_observed_name = None
    assert tracker._break_matches_active_weapon("xxxx") is False
    _db.close()


def test_break_observed_in_item_returns_true_not_false():
    # mut_28 (item not in observed) reinforced - observed substring of item only.
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "ZZZUnrelated", "Imk")
    # observed "imk" in item "imkadalaxe"; item not in observed.
    assert tracker._break_matches_active_weapon("Imkadalaxe") is True
    _db.close()


def test_break_case_difference_still_matches_via_normalisation():
    # mut_9 + mut_13: if either side stops folding to the same case the match
    # breaks. Item all-caps, tool all-lower.
    _, tracker, _db = _tracker()
    _set_weapon(tracker, "excaliburx", "excaliburx")
    assert tracker._break_matches_active_weapon("EXCALIBURX") is True
    _db.close()


# ----------------------------------------------------------------------------
# _on_global - player / session gating
# ----------------------------------------------------------------------------


def _notable_rows(db):
    return db.execute(
        "SELECT kill_id, event_type, mob_or_item, value_ped, timestamp "
        "FROM notable_events ORDER BY id"
    ).fetchall()


def test_global_missing_player_key_does_not_crash_and_inserts_nothing():
    # mut_4 (default None), mut_6 (default removed → None): a missing player key
    # must fall back to "" (mismatch → early return), never None.lower() crash.
    # Call _on_global directly (not via the bus, which swallows subscriber
    # exceptions) so the None.lower() AttributeError surfaces as a test error.
    bus, tracker, db = _tracker(player="Me")
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    # No "player" key at all - orig defaults to "" and returns cleanly.
    tracker._on_global(
        {"type": "global_kill", "creature": "Atrox", "value": 5.0, "timestamp": t0}
    )
    assert _notable_rows(db) == []  # mismatch → never reached the insert
    assert tracker.session.kills[-1].is_global is False
    tracker.stop_session()
    db.close()


def test_global_missing_player_key_default_is_empty_string():
    # mut_9 (default "XXXX"): with a tracker whose player_name is "xxxx", a
    # missing-player global would *spuriously correlate* if the default were
    # "XXXX" rather than "".
    bus, tracker, db = _tracker(player="xxxx")
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    bus.publish(
        EVENT_GLOBAL,
        {"type": "global_kill", "creature": "Atrox", "value": 5.0, "timestamp": t0},
    )
    # Default "" != "xxxx" → early return, nothing persisted.
    assert _notable_rows(db) == []
    tracker.stop_session()
    db.close()


def test_global_session_required():
    # mut_1 (if self._session: return) - without a session nothing persists.
    bus, tracker, db = _tracker()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    # No session started yet.
    bus.publish(
        EVENT_GLOBAL,
        {
            "player": "Me",
            "type": "global_kill",
            "creature": "Atrox",
            "value": 5.0,
            "timestamp": t0,
        },
    )
    assert _notable_rows(db) == []
    db.close()


# ----------------------------------------------------------------------------
# _on_global - event_type column
# ----------------------------------------------------------------------------


def test_global_event_type_default_empty_string_when_missing():
    # mut_17 (default None → NOT NULL violation), mut_19 (default removed → None),
    # mut_22 (default "XXXX").
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    bus.publish(
        EVENT_GLOBAL,
        {"player": "Me", "creature": "Atrox", "value": 5.0, "timestamp": t0},
    )
    rows = _notable_rows(db)
    assert len(rows) == 1
    assert rows[0][1] == ""  # event_type
    tracker.stop_session()
    db.close()


# ----------------------------------------------------------------------------
# _on_global - mob_or_item resolution
# ----------------------------------------------------------------------------


def _fire_global(bus, **payload):
    base = {"player": "Me", "type": "global_kill", "value": 5.0}
    base.update(payload)
    if "timestamp" not in base:
        base["timestamp"] = datetime(2025, 1, 1, 12, 0, 0)
    bus.publish(EVENT_GLOBAL, base)


def test_global_mob_or_item_prefers_creature():
    # mut_26 (data.get(None)), mut_27 ("XXcreatureXX"), mut_28 ("CREATURE"):
    # the creature key must be read so the creature wins.
    bus, tracker, db = _tracker()
    tracker.start_session()
    _fire_global(bus, creature="Atrox", value=5.0)
    rows = _notable_rows(db)
    assert rows[0][2] == "Atrox"  # mob_or_item
    tracker.stop_session()
    db.close()


def test_global_mob_or_item_falls_back_to_item_when_no_creature():
    # mut_24 (creature or (item and "Unknown")), mut_25 (creature and item or ...),
    # mut_29 (item via None key), mut_30 ("XXitemXX"), mut_31 ("ITEM").
    bus, tracker, db = _tracker()
    tracker.start_session()
    _fire_global(bus, type="global_item", item="Rare Sword", value=5.0)
    rows = _notable_rows(db)
    assert rows[0][2] == "Rare Sword"
    tracker.stop_session()
    db.close()


def test_global_mob_or_item_creature_only_keeps_creature():
    # mut_25 (creature and item or "Unknown" → "Unknown" when item missing).
    bus, tracker, db = _tracker()
    tracker.start_session()
    _fire_global(bus, creature="Atrox", value=5.0)  # no item key
    rows = _notable_rows(db)
    assert rows[0][2] == "Atrox"
    tracker.stop_session()
    db.close()


def test_global_mob_or_item_defaults_to_unknown_exactly():
    # mut_32 ("XXUnknownXX"), mut_33 ("unknown"), mut_34 ("UNKNOWN").
    bus, tracker, db = _tracker()
    tracker.start_session()
    _fire_global(bus, value=5.0)  # neither creature nor item
    rows = _notable_rows(db)
    assert rows[0][2] == "Unknown"
    tracker.stop_session()
    db.close()


# ----------------------------------------------------------------------------
# _on_global - value_ped
# ----------------------------------------------------------------------------


def test_global_value_ped_read_from_value_key():
    # mut_36 (data.get(None)), mut_40 ("XXvalueXX"), mut_41 ("VALUE").
    bus, tracker, db = _tracker()
    tracker.start_session()
    _fire_global(bus, creature="Atrox", value=1234.5)
    rows = _notable_rows(db)
    assert rows[0][3] == 1234.5  # value_ped
    tracker.stop_session()
    db.close()


def test_global_value_ped_defaults_to_zero_when_missing():
    # mut_37 (default None → NOT NULL violation), mut_39 (default removed),
    # mut_42 (default 1.0).
    bus, tracker, db = _tracker()
    tracker.start_session()
    # Publish directly with NO "value" key (the _fire_global helper always
    # supplies one).
    bus.publish(
        EVENT_GLOBAL,
        {
            "player": "Me",
            "type": "global_kill",
            "creature": "Atrox",
            "timestamp": datetime(2025, 1, 1, 12, 0, 0),
        },
    )
    rows = _notable_rows(db)
    assert rows[0][3] == 0.0
    tracker.stop_session()
    db.close()


# ----------------------------------------------------------------------------
# _on_global - HoF flag
# ----------------------------------------------------------------------------


def test_global_hof_item_sets_is_hof():
    # mut_47 ("XXhof_itemXX"), mut_48 ("HOF_ITEM"): "hof_item" must be recognised.
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    _fire_global(bus, type="hof_item", item="Rare", value=9.0, timestamp=t0)
    kill = tracker.session.kills[-1]
    assert kill.is_hof is True
    db_row = db.execute(
        "SELECT is_global, is_hof FROM kills WHERE id = ?", (kill.id,)
    ).fetchone()
    assert db_row == (1, 1)
    tracker.stop_session()
    db.close()


def test_non_hof_global_does_not_set_is_hof():
    # Guards the hof tuple from over-matching (companion to mut_47/mut_48).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    _fire_global(bus, type="global_kill", creature="Atrox", value=9.0, timestamp=t0)
    kill = tracker.session.kills[-1]
    assert kill.is_global is True
    assert kill.is_hof is False
    db_row = db.execute(
        "SELECT is_global, is_hof FROM kills WHERE id = ?", (kill.id,)
    ).fetchone()
    assert db_row == (1, 0)
    tracker.stop_session()
    db.close()


# ----------------------------------------------------------------------------
# _on_global - kill_id default & staleness window
# ----------------------------------------------------------------------------


def test_global_with_no_recent_kill_records_null_kill_id():
    # mut_54 (kill_id = "" instead of None).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    # No kill at all in this session → uncorrelated global.
    _fire_global(bus, creature="Atrox", value=5.0, timestamp=t0)
    rows = _notable_rows(db)
    assert len(rows) == 1
    assert rows[0][0] is None  # kill_id must be SQL NULL, not ""
    tracker.stop_session()
    db.close()


def test_global_exactly_five_seconds_does_not_correlate():
    # mut_59 (<= 5.0): the window is strictly < 5.0, so 5.0 exactly must NOT tag.
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    _fire_global(
        bus, creature="Atrox", value=5.0, timestamp=t0 + timedelta(seconds=5)
    )
    kill = tracker.session.kills[-1]
    assert kill.is_global is False
    # The notable event still persists, uncorrelated.
    rows = _notable_rows(db)
    assert rows[-1][0] is None
    tracker.stop_session()
    db.close()


def test_global_at_five_point_five_seconds_does_not_correlate():
    # mut_60 (< 6.0): 5.5s is inside a 6.0 window but outside the real 5.0 one.
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    _fire_global(
        bus, creature="Atrox", value=5.0, timestamp=t0 + timedelta(seconds=5.5)
    )
    assert tracker.session.kills[-1].is_global is False
    tracker.stop_session()
    db.close()


def test_global_within_window_correlates():
    # Positive companion: a global 2s after the kill must correlate (pins the
    # TRUE branch of the < window alongside mut_59/mut_60).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    _fire_global(
        bus, creature="Atrox", value=5.0, timestamp=t0 + timedelta(seconds=2)
    )
    kill = tracker.session.kills[-1]
    assert kill.is_global is True
    rows = _notable_rows(db)
    assert rows[-1][0] == kill.id
    tracker.stop_session()
    db.close()


def test_global_correlation_persists_to_kills_table():
    # Anchors the UPDATE effect (the keyword/identifier case mutants mut_71/72
    # are SQLite-equivalent; this still guards that the UPDATE runs and flips
    # both columns).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    _fire_global(bus, type="hof_kill", creature="Atrox", value=5.0, timestamp=t0)
    kill = tracker.session.kills[-1]
    db_row = db.execute(
        "SELECT is_global, is_hof FROM kills WHERE id = ?", (kill.id,)
    ).fetchone()
    assert db_row == (1, 1)
    tracker.stop_session()
    db.close()


# ----------------------------------------------------------------------------
# _on_global - log messages
# ----------------------------------------------------------------------------


def _correlated_record(caplog):
    recs = [
        r
        for r in caplog.records
        if r.name == TRACKER_LOGGER and "correlated" in str(r.msg).lower()
    ]
    assert recs, "expected a correlated INFO record"
    return recs[-1]


def test_global_correlated_info_log_message_rendered(caplog):
    # mut_74 (None fmt), mut_75/76/77 (None args), mut_78/79/80/81 (dropped args),
    # mut_82/83/84 (format text/case), mut_85 (target.id[:9]).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    _make_kill(bus, t0)
    kill = tracker.session.kills[-1]
    with caplog.at_level(logging.INFO, logger=TRACKER_LOGGER):
        _fire_global(
            bus, type="global_kill", creature="Atrox", value=42.0, timestamp=t0
        )
    record = _correlated_record(caplog)
    # getMessage re-applies the % interpolation; a None/dropped arg or a None
    # format string raises here, and a mangled format text changes the result.
    rendered = record.getMessage()
    assert rendered == (
        "Global/HoF correlated: global_kill 42.00 PED → kill %s" % kill.id[:8]
    )
    # mut_85 ([:9]) - exactly the 8-char prefix must be present, not 9.
    assert kill.id[:8] in rendered
    assert kill.id[:9] not in rendered
    tracker.stop_session()
    db.close()


def test_global_uncorrelated_warning_log_message_rendered(caplog):
    # mut_86 (None fmt), mut_87/88 (None args), mut_89/90/91 (dropped args),
    # mut_92/93/94 (format text/case).
    bus, tracker, db = _tracker()
    tracker.start_session()
    t0 = datetime(2025, 1, 1, 12, 0, 0)
    # No kill in window → the warning branch fires.
    with caplog.at_level(logging.WARNING, logger=TRACKER_LOGGER):
        _fire_global(
            bus, type="global_kill", creature="Atrox", value=7.0, timestamp=t0
        )
    recs = [
        r
        for r in caplog.records
        if r.name == TRACKER_LOGGER and r.levelno == logging.WARNING
    ]
    assert recs, "expected a WARNING record"
    rendered = recs[-1].getMessage()
    assert rendered == (
        "Global/HoF with no recent kill to correlate: global_kill 7.00 PED"
    )
    tracker.stop_session()
    db.close()
