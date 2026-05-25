"""Tests for the chat.log parser: every EventType must have at least one case."""

import pytest

from backend.services.chatlog_parser import EventType, parse_file, parse_line


@pytest.mark.parametrize(
    "line,expected_type,expected_data",
    [
        # ── Player attacks ────────────────────────────────────────────────────────
        (
            "2026-03-24 14:30:15 [System] [] You inflicted 42.3 points of damage",
            EventType.DAMAGE_DEALT,
            {"amount": 42.3},
        ),
        # Critical hit must NOT be parsed as regular damage_dealt
        (
            "2026-03-24 14:30:15 [System] [] Critical hit - Additional damage! You inflicted 84.6 points of damage",
            EventType.CRITICAL_HIT,
            {"amount": 84.6},
        ),
        (
            "2026-03-24 14:30:16 [System] [] The target Jammed your attack",
            EventType.TARGET_JAM,
            {},
        ),
        (
            "2026-03-24 14:30:16 [System] [] The target Dodged your attack",
            EventType.TARGET_DODGE,
            {},
        ),
        (
            "2026-03-24 14:30:16 [System] [] The target Evaded your attack",
            EventType.TARGET_EVADE,
            {},
        ),
        # ── Mob attacks on player ─────────────────────────────────────────────────
        (
            "2026-03-24 14:30:17 [System] [] You took 28.5 points of damage",
            EventType.DAMAGE_RECEIVED,
            {"amount": 28.5},
        ),
        (
            "2026-03-24 14:30:17 [System] [] Damage deflected!",
            EventType.DEFLECT,
            {},
        ),
        (
            "2026-03-24 14:30:18 [System] [] You Evaded the attack",
            EventType.PLAYER_EVADE,
            {},
        ),
        (
            "2026-03-24 14:30:18 [System] [] You Dodged the attack",
            EventType.PLAYER_DODGE,
            {},
        ),
        (
            "2026-03-24 14:30:18 [System] [] You Jammed the attack",
            EventType.PLAYER_JAM,
            {},
        ),
        (
            "2026-03-24 14:30:18 [System] [] The attack missed you",
            EventType.MOB_MISS,
            {},
        ),
        # ── Healing ───────────────────────────────────────────────────────────────
        (
            "2026-03-24 14:30:20 [System] [] You healed yourself 33.5 points",
            EventType.SELF_HEAL,
            {"amount": 33.5},
        ),
        # ── Loot: single item (no quantity) ──────────────────────────────────────
        (
            "2026-03-24 14:31:00 [System] [] You received Animal Muscle Oil Value: 0.12 PED",
            EventType.LOOT,
            {"item_name": "Animal Muscle Oil", "quantity": 1, "value": 0.12},
        ),
        # ── Loot: item with quantity ─────────────────────────────────────────────
        (
            "2026-03-24 14:31:00 [System] [] You received Shrapnel x (342) Value: 3.42 PED",
            EventType.LOOT,
            {"item_name": "Shrapnel", "quantity": 342, "value": 3.42},
        ),
        # ── Skills: direct format (attributes, short) ────────────────────────────
        (
            "2026-03-24 14:31:05 [System] [] You have gained 0.3198 Bravado",
            EventType.SKILL_GAIN,
            {"amount": 0.3198, "skill_name": "Bravado"},
        ),
        # ── Skills: verbose format ("experience in your X skill") ────────────────
        (
            "2026-03-24 14:31:05 [System] [] You have gained 0.1213 experience in your Laser Weaponry Technology skill",
            EventType.SKILL_GAIN,
            {"amount": 0.1213, "skill_name": "Laser Weaponry Technology"},
        ),
        # ── Attribute improvement ("Your X has improved by Y") ─────────────────────
        (
            "2026-03-24 14:31:05 [System] [] Your Agility has improved by 0.6055",
            EventType.SKILL_GAIN,
            {"amount": 0.6055, "skill_name": "Agility"},
        ),
        # ── Enhancer break ────────────────────────────────────────────────────────
        (
            "2026-03-24 14:32:00 [System] [] Your enhancer Weapon Damage Enhancer 2 on your Karma Killer Mk. 3a broke. "
            "You have 7 enhancers remaining on the item. You received 0.50 PED Shrapnel.",
            EventType.ENHANCER_BREAK,
            {
                "enhancer_name": "Weapon Damage Enhancer 2",
                "item_name": "Karma Killer Mk. 3a",
                "remaining": 7,
                "shrapnel_ped": 0.50,
            },
        ),
        # ── Global kill ───────────────────────────────────────────────────────────
        (
            "2026-03-24 15:00:00 [Globals] [] Test Player killed a creature (Atrox Provider) with a value of 51 PED!",
            EventType.GLOBAL_KILL,
            {"player": "Test Player", "creature": "Atrox Provider", "value": 51.0},
        ),
        # ── HoF kill: must NOT be parsed as regular global kill ──────────────────
        (
            "2026-03-24 15:00:00 [Globals] [] Test Player killed a creature (Atrox Alpha) with a value of 150 PED! "
            "A record has been added to the Hall of Fame!",
            EventType.HOF_KILL,
            {"player": "Test Player", "creature": "Atrox Alpha", "value": 150.0},
        ),
        # ── Global item find (PEC value) ──────────────────────────────────────────
        (
            "2026-03-24 15:05:00 [Globals] [] Test Player has found a rare item (Nova Fragment) with a value of 40 PEC!",
            EventType.GLOBAL_ITEM,
            {"player": "Test Player", "item": "Nova Fragment", "value": 40.0},
        ),
        # ── HoF item find: must NOT be parsed as regular global item ─────────────
        (
            "2026-03-24 15:05:00 [Globals] [] Test Player has found a rare item (Adjusted ArMatrix LR-40 (L)) "
            "with a value of 500 PED! A record has been added to the Hall of Fame!",
            EventType.HOF_ITEM,
            {
                "player": "Test Player",
                "item": "Adjusted ArMatrix LR-40 (L)",
                "value": 500.0,
            },
        ),
        # ── Mission completed ───────────────────────────────────────────────────
        (
            "2026-03-24 17:31:58 [System] [] Mission completed (Paneleon Hunter Jameson's Mission (repeatable))",
            EventType.MISSION_COMPLETE,
            {"mission_name": "Paneleon Hunter Jameson's Mission (repeatable)"},
        ),
        # ── New Mission received ────────────────────────────────────────────────
        (
            "2026-03-24 18:38:13 [System] [] New Mission received (ARIS - Daily Hunting 1: Defective Destroyers)",
            EventType.MISSION_RECEIVED,
            {"mission_name": "ARIS - Daily Hunting 1: Defective Destroyers"},
        ),
    ],
)
def test_parse_event(line, expected_type, expected_data):
    event = parse_line(line)
    assert event is not None, f"Failed to parse: {line}"
    assert event.type == expected_type
    for key, value in expected_data.items():
        assert event.data[key] == value, (
            f"data[{key!r}]: got {event.data[key]!r}, expected {value!r}"
        )


def test_unrecognised_channel_returns_none():
    assert parse_line("2026-03-24 14:30:15 [Local] [Player] Hello everyone") is None


def test_malformed_line_returns_none():
    assert parse_line("not a valid chat log line") is None


def test_empty_line_returns_none():
    assert parse_line("") is None
    assert parse_line("   ") is None


def test_all_event_types_covered():
    """Every EventType must appear in the parametrize cases above."""
    covered = {
        EventType.DAMAGE_DEALT,
        EventType.CRITICAL_HIT,
        EventType.TARGET_JAM,
        EventType.TARGET_DODGE,
        EventType.TARGET_EVADE,
        EventType.DAMAGE_RECEIVED,
        EventType.DEFLECT,
        EventType.PLAYER_EVADE,
        EventType.PLAYER_DODGE,
        EventType.PLAYER_JAM,
        EventType.MOB_MISS,
        EventType.SELF_HEAL,
        EventType.LOOT,
        EventType.SKILL_GAIN,
        EventType.ENHANCER_BREAK,
        EventType.GLOBAL_KILL,
        EventType.HOF_KILL,
        EventType.GLOBAL_ITEM,
        EventType.HOF_ITEM,
        EventType.MISSION_COMPLETE,
        EventType.MISSION_RECEIVED,
    }
    for event_type in EventType:
        assert event_type in covered, f"EventType.{event_type.name} has no test case"


def test_critical_hit_distinguished_from_damage_dealt():
    """A critical hit line must parse as CRITICAL_HIT, never DAMAGE_DEALT."""
    line = "2026-03-24 14:30:15 [System] [] Critical hit - Additional damage! You inflicted 84.6 points of damage"
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.CRITICAL_HIT
    assert event.data["amount"] == 84.6


def test_hof_kill_distinguished_from_global_kill():
    """A HoF kill line must parse as HOF_KILL, never GLOBAL_KILL."""
    line = (
        "2026-03-24 15:00:00 [Globals] [] Test Player killed a creature (Atrox Alpha) "
        "with a value of 150 PED! A record has been added to the Hall of Fame!"
    )
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.HOF_KILL


def test_hof_item_distinguished_from_global_item():
    """A HoF item line must parse as HOF_ITEM, never GLOBAL_ITEM."""
    line = (
        "2026-03-24 15:05:00 [Globals] [] Test Player has found a rare item (Rare Sword) "
        "with a value of 1000 PED! A record has been added to the Hall of Fame!"
    )
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.HOF_ITEM


def test_loot_quantity_extraction():
    """Loot with 'x (N)' quantity must extract item name and count correctly."""
    line = (
        "2026-03-24 14:31:00 [System] [] You received Shrapnel x (342) Value: 3.42 PED"
    )
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.LOOT
    assert event.data["item_name"] == "Shrapnel"
    assert event.data["quantity"] == 342
    assert event.data["value"] == 3.42


def test_loot_no_quantity():
    """Loot without quantity marker must default to quantity=1."""
    line = (
        "2026-03-24 14:31:00 [System] [] You received Animal Muscle Oil Value: 0.12 PED"
    )
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.LOOT
    assert event.data["quantity"] == 1


def test_skill_verbose_before_direct():
    """Verbose skill format must parse correctly and not be caught by direct pattern."""
    line = "2026-03-24 14:31:05 [System] [] You have gained 0.1213 experience in your Laser Weaponry Technology skill"
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.SKILL_GAIN
    assert event.data["skill_name"] == "Laser Weaponry Technology"
    assert event.data["amount"] == 0.1213


def test_parse_file(tmp_path):
    """parse_file returns events for recognised lines and skips unrecognised ones."""
    log_content = "\n".join(
        [
            "2026-03-24 14:30:15 [System] [] You inflicted 42.3 points of damage",
            "2026-03-24 14:30:16 [Local] [SomePlayer] hey anyone hunting atrox?",
            "2026-03-24 14:30:17 [System] [] You took 28.5 points of damage",
            "2026-03-24 14:31:00 [System] [] You received Animal Muscle Oil Value: 0.12 PED",
            "2026-03-24 14:31:05 [System] [] You have gained 0.3198 Bravado",
            "2026-03-24 15:00:00 [Globals] [] Test Player killed a creature (Atrox Provider) with a value of 51 PED!",
            "not a valid line",
        ]
    )
    log_file = tmp_path / "chat.log"
    log_file.write_text(log_content, encoding="utf-8")

    events = parse_file(str(log_file))

    assert len(events) == 5
    assert events[0].type == EventType.DAMAGE_DEALT
    assert events[1].type == EventType.DAMAGE_RECEIVED
    assert events[2].type == EventType.LOOT
    assert events[3].type == EventType.SKILL_GAIN
    assert events[4].type == EventType.GLOBAL_KILL


def test_parse_file_raw_line_preserved(tmp_path):
    """ChatEvent.raw_line must contain the original line text."""
    raw = "2026-03-24 14:30:15 [System] [] You inflicted 42.3 points of damage\n"
    log_file = tmp_path / "chat.log"
    log_file.write_text(raw, encoding="utf-8")

    events = parse_file(str(log_file))
    assert len(events) == 1
    assert events[0].raw_line == raw
