"""DSL round-trip property tests.

For every builder in :mod:`backend.testing.dsl`, the emitted chat
line is parsed back through :mod:`backend.services.chatlog_parser`
and the resulting :class:`ChatEvent` is asserted to carry the
data the builder claimed to emit. The test acts as a
drift-detector against MindArk format changes: if the parser's
regex surface shifts away from what the DSL emits, the round
trip stops, the test fails, and the DSL is updated to match.

Exemplar-driven coverage: hypothesis-style property generation
is overkill for an enumerable EventType surface, so each builder
gets one representative invocation that exercises its
data-extraction path. Loot is the one exception with two
invocations (single-item and quantity-bearing) because the
parser branches on the quantity pattern.
"""

from __future__ import annotations

from datetime import datetime

import pytest

from backend.services.chatlog_parser import EventType, parse_line
from backend.testing.dsl import Scenario

# === fixture ========================================================


@pytest.fixture
def scenario() -> Scenario:
    """Build a fresh scenario anchored at a fixed timestamp.

    The fixed anchor (``2026-05-19 10:00:00``) lets per-line round
    trips assert on exact ``timestamp`` values when relevant.
    """

    return Scenario(name="round_trip").at("2026-05-19 10:00:00")


def _only(scenario: Scenario):
    """Parse the scenario's single line and return the
    :class:`ChatEvent`.

    Builders all emit exactly one line; helper hides the
    line-extraction boilerplate at the call sites.
    """

    lines = scenario.lines()
    assert len(lines) == 1, f"expected 1 line, got {len(lines)}: {lines}"
    parsed = parse_line(lines[0])
    assert parsed is not None, f"parser returned None for {lines[0]!r}"
    return parsed


# === combat =========================================================


def test_combat_damage_dealt(scenario: Scenario) -> None:
    scenario.combat.damage_dealt(42.0)
    event = _only(scenario)
    assert event.type is EventType.DAMAGE_DEALT
    assert event.data["amount"] == pytest.approx(42.0)


def test_combat_critical_hit(scenario: Scenario) -> None:
    scenario.combat.critical_hit(99.5)
    event = _only(scenario)
    assert event.type is EventType.CRITICAL_HIT
    assert event.data["amount"] == pytest.approx(99.5)


def test_combat_target_dodge(scenario: Scenario) -> None:
    scenario.combat.target_dodge()
    assert _only(scenario).type is EventType.TARGET_DODGE


def test_combat_target_evade(scenario: Scenario) -> None:
    scenario.combat.target_evade()
    assert _only(scenario).type is EventType.TARGET_EVADE


def test_combat_target_jam(scenario: Scenario) -> None:
    scenario.combat.target_jam()
    assert _only(scenario).type is EventType.TARGET_JAM


def test_combat_damage_received(scenario: Scenario) -> None:
    scenario.combat.damage_received(11.5)
    event = _only(scenario)
    assert event.type is EventType.DAMAGE_RECEIVED
    assert event.data["amount"] == pytest.approx(11.5)


def test_combat_player_dodge(scenario: Scenario) -> None:
    scenario.combat.player_dodge()
    assert _only(scenario).type is EventType.PLAYER_DODGE


def test_combat_player_evade(scenario: Scenario) -> None:
    scenario.combat.player_evade()
    assert _only(scenario).type is EventType.PLAYER_EVADE


def test_combat_player_jam(scenario: Scenario) -> None:
    scenario.combat.player_jam()
    assert _only(scenario).type is EventType.PLAYER_JAM


def test_combat_mob_miss(scenario: Scenario) -> None:
    scenario.combat.mob_miss()
    assert _only(scenario).type is EventType.MOB_MISS


def test_combat_deflect(scenario: Scenario) -> None:
    scenario.combat.deflect()
    assert _only(scenario).type is EventType.DEFLECT


def test_combat_self_heal(scenario: Scenario) -> None:
    scenario.combat.self_heal(15.0)
    event = _only(scenario)
    assert event.type is EventType.SELF_HEAL
    assert event.data["amount"] == pytest.approx(15.0)


# === loot ============================================================


def test_loot_single_item(scenario: Scenario) -> None:
    scenario.loot.received("Animal Muscle Oil", value_ped=0.12)
    event = _only(scenario)
    assert event.type is EventType.LOOT
    assert event.data["item_name"] == "Animal Muscle Oil"
    assert event.data["quantity"] == 1
    assert event.data["value"] == pytest.approx(0.12)


def test_loot_quantity_bearing(scenario: Scenario) -> None:
    scenario.loot.received("Shrapnel", value_ped=5.00, quantity=500)
    event = _only(scenario)
    assert event.type is EventType.LOOT
    assert event.data["item_name"] == "Shrapnel"
    assert event.data["quantity"] == 500
    assert event.data["value"] == pytest.approx(5.00)


# === skill ===========================================================


def test_skill_gained_modern_format(scenario: Scenario) -> None:
    scenario.skill.gained(0.0500, "Bioregenesis")
    event = _only(scenario)
    assert event.type is EventType.SKILL_GAIN
    assert event.data["amount"] == pytest.approx(0.0500)
    assert event.data["skill_name"] == "Bioregenesis"


# === enhancer ========================================================


def test_enhancer_broken(scenario: Scenario) -> None:
    scenario.enhancer.broken(
        enhancer_name="Weapon Damage Enhancer 1",
        item_name="ArMatrix LR-5",
        shrapnel_ped=0.83,
        remaining=2,
    )
    event = _only(scenario)
    assert event.type is EventType.ENHANCER_BREAK
    assert event.data["enhancer_name"] == "Weapon Damage Enhancer 1"
    assert event.data["item_name"] == "ArMatrix LR-5"
    assert event.data["remaining"] == 2
    assert event.data["shrapnel_ped"] == pytest.approx(0.83)


# === globals =========================================================


def test_globals_kill_non_hof(scenario: Scenario) -> None:
    scenario.globals.kill(
        player="TestPlayer",
        creature="Argonaut Stalker",
        value_ped=130.00,
    )
    event = _only(scenario)
    assert event.type is EventType.GLOBAL_KILL
    assert event.data["player"] == "TestPlayer"
    assert event.data["creature"] == "Argonaut Stalker"
    assert event.data["value"] == pytest.approx(130.00)


def test_globals_kill_hof(scenario: Scenario) -> None:
    scenario.globals.kill(
        player="TestPlayer",
        creature="Argonaut Stalker",
        value_ped=2500.00,
        hof=True,
    )
    event = _only(scenario)
    assert event.type is EventType.HOF_KILL
    assert event.data["player"] == "TestPlayer"
    assert event.data["creature"] == "Argonaut Stalker"
    assert event.data["value"] == pytest.approx(2500.00)


def test_globals_item_non_hof(scenario: Scenario) -> None:
    scenario.globals.item(
        player="TestPlayer",
        item="Mod Merc",
        value_ped=950.00,
    )
    event = _only(scenario)
    assert event.type is EventType.GLOBAL_ITEM
    assert event.data["player"] == "TestPlayer"
    assert event.data["item"] == "Mod Merc"
    assert event.data["value"] == pytest.approx(950.00)


def test_globals_item_hof(scenario: Scenario) -> None:
    scenario.globals.item(
        player="TestPlayer",
        item="Mod Merc",
        value_ped=5000.00,
        hof=True,
    )
    event = _only(scenario)
    assert event.type is EventType.HOF_ITEM
    assert event.data["player"] == "TestPlayer"
    assert event.data["item"] == "Mod Merc"
    assert event.data["value"] == pytest.approx(5000.00)


# === mission =========================================================


def test_mission_received(scenario: Scenario) -> None:
    scenario.mission.received("Codex Argonaut Stage 1")
    event = _only(scenario)
    assert event.type is EventType.MISSION_RECEIVED
    assert event.data["mission_name"] == "Codex Argonaut Stage 1"


def test_mission_completed(scenario: Scenario) -> None:
    scenario.mission.completed("Codex Argonaut Stage 1")
    event = _only(scenario)
    assert event.type is EventType.MISSION_COMPLETE
    assert event.data["mission_name"] == "Codex Argonaut Stage 1"


# === timestamp handling =============================================


def test_scenario_at_string_and_datetime_equivalent() -> None:
    """``Scenario.at`` accepts both ``str`` and ``datetime``."""

    s_str = Scenario(name="ts").at("2026-05-19 10:00:00")
    s_str.combat.damage_dealt(10.0)

    s_dt = Scenario(name="ts").at(datetime(2026, 5, 19, 10, 0, 0))
    s_dt.combat.damage_dealt(10.0)

    assert s_str.lines() == s_dt.lines()


def test_scenario_tick_advances_by_one_second() -> None:
    """``Scenario.tick`` advances the current timestamp by one second."""

    s = Scenario(name="tick").at("2026-05-19 10:00:00")
    s.combat.damage_dealt(10.0)
    s.tick()
    s.combat.damage_dealt(11.0)
    s.tick(seconds=5)
    s.combat.damage_dealt(12.0)

    timestamps = [line.split(" [System]", 1)[0] for line in s.lines()]
    assert timestamps == [
        "2026-05-19 10:00:00",
        "2026-05-19 10:00:01",
        "2026-05-19 10:00:06",
    ]


def test_emit_before_at_raises() -> None:
    """Builder calls before any ``Scenario.at`` raise RuntimeError."""

    s = Scenario(name="missing_at")
    with pytest.raises(RuntimeError, match=r"Scenario\.at"):
        s.combat.damage_dealt(10.0)


def test_tick_before_at_raises() -> None:
    """``Scenario.tick`` before any ``Scenario.at`` raises."""

    s = Scenario(name="missing_at")
    with pytest.raises(RuntimeError, match=r"Scenario\.at"):
        s.tick()


def test_tick_with_non_positive_seconds_raises() -> None:
    """``Scenario.tick(seconds)`` rejects values < 1.

    Zero would stall the clock and negative values would reverse
    it; either case breaks the monotonic-timestamp guarantee the
    surrounding harness relies on (kill flushes are partitioned
    by ascending timestamps, and the DB-snapshot SQL catalogue
    orders rows by the parsed-timestamp + rowid tiebreaker).
    """

    s = Scenario(name="non_positive_tick").at("2026-05-19 10:00:00")
    with pytest.raises(ValueError, match=r"seconds >= 1"):
        s.tick(seconds=0)
    with pytest.raises(ValueError, match=r"seconds >= 1"):
        s.tick(seconds=-5)


# === keystroke ======================================================


def test_keystroke_press_records_at_current_timestamp() -> None:
    """A ``keystroke.press`` records (key, kind, offset_s, wall) at the
    scenario's current timestamp, with ``offset_s`` measured from the
    epoch (first :meth:`Scenario.at` call)."""
    s = Scenario("ks").at("2026-05-28 12:00:00")
    s.keystroke.press("1")
    s.tick(3)
    s.keystroke.press("2")

    records = s.keystrokes()
    assert records == [
        {
            "key": "1",
            "kind": "press",
            "offset_s": 0.0,
            "wall": "2026-05-28T12:00:00+00:00",
        },
        {
            "key": "2",
            "kind": "press",
            "offset_s": 3.0,
            "wall": "2026-05-28T12:00:03+00:00",
        },
    ]


def test_keystroke_release_records_release_kind() -> None:
    """``keystroke.release`` records ``kind="release"``."""
    s = Scenario("ks").at("2026-05-28 12:00:00")
    s.keystroke.press("space")
    s.keystroke.release("space")

    records = s.keystrokes()
    assert [(r["key"], r["kind"]) for r in records] == [
        ("space", "press"),
        ("space", "release"),
    ]


def test_keystroke_before_at_raises() -> None:
    """Building a keystroke without a timestamp anchor fails fast,
    mirroring the chat-line builders' contract."""
    s = Scenario("ks")
    with pytest.raises(RuntimeError, match=r"before Scenario\.at"):
        s.keystroke.press("1")


def test_scenario_write_emits_keystrokes_jsonl(tmp_path) -> None:
    """``Scenario.write`` writes a recorder-shaped ``keystrokes.jsonl``
    next to ``chat_replay.log`` when any keystrokes were recorded."""
    import json

    s = Scenario("ks").at("2026-05-28 12:00:00")
    s.keystroke.press("1")
    s.tick()
    s.keystroke.press("2")
    s.write(tmp_path / "scenario")

    keystrokes_path = tmp_path / "scenario" / "keystrokes.jsonl"
    assert keystrokes_path.exists()
    lines = keystrokes_path.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 2
    first = json.loads(lines[0])
    # Recorder writes records with sort_keys=True; mirror that here.
    assert list(first.keys()) == ["key", "kind", "offset_s", "wall"]
    assert first["key"] == "1"
    assert first["kind"] == "press"


def test_scenario_write_skips_keystrokes_jsonl_when_unused(tmp_path) -> None:
    """No ``keystrokes.jsonl`` is written when the scenario recorded
    no keystrokes (chat-only scenarios stay clean)."""
    s = Scenario("chat_only").at("2026-05-28 12:00:00")
    s.combat.damage_dealt(10.0)
    s.write(tmp_path / "scenario")

    assert (tmp_path / "scenario" / "chat_replay.log").exists()
    assert not (tmp_path / "scenario" / "keystrokes.jsonl").exists()
