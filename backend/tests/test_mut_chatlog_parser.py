"""Mutation-hardening tests for backend.services.chatlog_parser.

Each test targets a specific surviving mutant cluster. The assertions pin the
exact observable behaviour (event type, data values, timestamp, raw_line) that
the corresponding mutation would break.
"""

from __future__ import annotations

import re
from datetime import datetime

import pytest

from backend.services.chatlog_parser import (
    EventType,
    _amount,
    parse_file,
    parse_line,
)


# ─────────────────────────────────────────────────────────────────────────────
# _amount() exercised DIRECTLY at runtime. The production module builds
# SYSTEM_RULES (which captures _amount()'s returned lambda) at import time, so a
# test that only calls parse_line never re-invokes _amount during its own body;
# mutmut would then never associate this test with the x__amount function. By
# calling _amount() inside the test body we (a) make mutmut map this test to
# x__amount and (b) pin every observable property of the extractor.
#
# Kills x__amount__mutmut_1 (default group 1 -> 2: returns the wrong group),
#       x__amount__mutmut_2 (returns None instead of a dict),
#       x__amount__mutmut_3 (key "amount" -> "XXamountXX"),
#       x__amount__mutmut_4 (key "amount" -> "AMOUNT"),
#       x__amount__mutmut_5 (float(None) -> TypeError),
#       x__amount__mutmut_6 (match.group(None) -> IndexError).
# ─────────────────────────────────────────────────────────────────────────────
def test_amount_extractor_default_group_is_one():
    extractor = _amount()
    # Two numeric groups: the default must read group 1 (10.0), NOT group 2.
    match = re.search(r"(\d+(?:\.\d+)?) .* (\d+(?:\.\d+)?)", "10.0 and 20.0")
    assert match is not None
    assert extractor(match) == {"amount": 10.0}


def test_amount_extractor_explicit_group():
    extractor = _amount(2)
    match = re.search(r"(\d+(?:\.\d+)?) .* (\d+(?:\.\d+)?)", "10.0 and 20.0")
    assert match is not None
    # Explicit group 2 -> 20.0; pins the float key and value.
    result = extractor(match)
    assert result == {"amount": 20.0}
    assert isinstance(result["amount"], float)


# ─────────────────────────────────────────────────────────────────────────────
# _amount(): the float-amount extractor shared by all damage / heal rules.
# Kills x__amount__mutmut_1 (group=2 -> IndexError),
#       x__amount__mutmut_2 (return None instead of dict),
#       x__amount__mutmut_3/4 (key renamed to "XXamountXX"/"AMOUNT"),
#       x__amount__mutmut_5 (float(None) -> TypeError),
#       x__amount__mutmut_6 (match.group(None) -> IndexError).
# ─────────────────────────────────────────────────────────────────────────────
@pytest.mark.parametrize(
    "line,expected_type,expected_amount",
    [
        (
            "2026-03-24 14:30:15 [System] [] You inflicted 42.3 points of damage",
            EventType.DAMAGE_DEALT,
            42.3,
        ),
        (
            "2026-03-24 14:30:15 [System] [] Critical hit - Additional damage! "
            "You inflicted 84.6 points of damage",
            EventType.CRITICAL_HIT,
            84.6,
        ),
        (
            "2026-03-24 14:30:17 [System] [] You took 28.5 points of damage",
            EventType.DAMAGE_RECEIVED,
            28.5,
        ),
        (
            "2026-03-24 14:30:20 [System] [] You healed yourself 33.5 points",
            EventType.SELF_HEAL,
            33.5,
        ),
    ],
)
def test_amount_extractor_uses_group_one_under_amount_key(
    line, expected_type, expected_amount
):
    event = parse_line(line)
    assert event is not None
    assert event.type == expected_type
    # The data dict must literally contain the key "amount" (not "AMOUNT" /
    # "XXamountXX") and must not be None; the float must come from group 1.
    assert event.data == {"amount": expected_amount}
    assert isinstance(event.data["amount"], float)


# ─────────────────────────────────────────────────────────────────────────────
# parse_line(): HTML entity unescaping branch.
# content = unescape(g2) if "&" in g2 else g2
# Kills x_parse_line__mutmut_18 ("&" -> "XX&XX": condition never true, never unescapes),
#       x_parse_line__mutmut_19 ("&" in -> "&" not in: inverted; entity left raw).
# ─────────────────────────────────────────────────────────────────────────────
def test_html_entities_in_global_player_name_are_unescaped():
    # A global-kill line whose player name carries an HTML entity. The parser
    # must unescape "&amp;" -> "&" before extracting the player group.
    line = (
        "2026-03-24 15:00:00 [Globals] [] Smith &amp; Wesson killed a creature "
        "(Atrox Provider) with a value of 51 PED!"
    )
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.GLOBAL_KILL
    # Original unescapes -> "Smith & Wesson". Mutant 18 (never unescape) and
    # mutant 19 (unescape only when no "&") both leave the raw "&amp;".
    assert event.data["player"] == "Smith & Wesson"


def test_no_entity_line_still_parses_when_unescape_branch_inverted():
    # A line without "&". Original takes the else-branch (no unescape, no-op).
    # Mutant 19 ("&" not in) would call unescape() here; harmless for plain
    # text, so this case alone does not separate it -- the entity test above
    # does. Kept to pin that ordinary lines are unaffected.
    line = "2026-03-24 14:30:15 [System] [] You inflicted 42.3 points of damage"
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.DAMAGE_DEALT


# ─────────────────────────────────────────────────────────────────────────────
# parse_line(): the [Globals] dispatch passes (timestamp, content, raw_line)
# to _parse_global.
# Kills x_parse_line__mutmut_32 (_parse_global(None, ...) -> timestamp None),
#       x_parse_line__mutmut_34 (_parse_global(timestamp, content, None) -> raw_line None).
# (Also reinforced by the _parse_global tests below.)
# ─────────────────────────────────────────────────────────────────────────────
def test_global_event_carries_timestamp_and_raw_line():
    raw = (
        "2026-03-24 15:00:00 [Globals] [] Test Player killed a creature "
        "(Atrox Provider) with a value of 51 PED!"
    )
    event = parse_line(raw)
    assert event is not None
    assert event.type == EventType.GLOBAL_KILL
    assert event.timestamp == datetime(2026, 3, 24, 15, 0, 0)
    assert event.raw_line == raw


# ─────────────────────────────────────────────────────────────────────────────
# _parse_global(): ChatEvent(event_type, timestamp, data, raw_line).
# Kills x__parse_global__mutmut_6 (timestamp -> None),
#       x__parse_global__mutmut_8 (raw_line -> None).
# Use a HoF-item line to also drive a distinct global rule.
# ─────────────────────────────────────────────────────────────────────────────
def test_parse_global_hof_item_timestamp_and_raw_line_preserved():
    raw = (
        "2026-03-24 15:05:00 [Globals] [] Test Player has found a rare item "
        "(Rare Sword) with a value of 1000 PED! "
        "A record has been added to the Hall of Fame!"
    )
    event = parse_line(raw)
    assert event is not None
    assert event.type == EventType.HOF_ITEM
    assert event.timestamp == datetime(2026, 3, 24, 15, 5, 0)
    assert event.raw_line == raw
    assert event.data["item"] == "Rare Sword"


# ─────────────────────────────────────────────────────────────────────────────
# _parse_system(): ChatEvent for the LOOT branch.
# Kills x__parse_system__mutmut_7 (timestamp -> None on loot),
#       x__parse_system__mutmut_9 (raw_line -> None on loot).
# ─────────────────────────────────────────────────────────────────────────────
def test_parse_system_loot_timestamp_and_raw_line_preserved():
    raw = (
        "2026-03-24 14:31:00 [System] [] You received Animal Muscle Oil Value: 0.12 PED"
    )
    event = parse_line(raw)
    assert event is not None
    assert event.type == EventType.LOOT
    assert event.timestamp == datetime(2026, 3, 24, 14, 31, 0)
    assert event.raw_line == raw
    assert event.data["item_name"] == "Animal Muscle Oil"


# ─────────────────────────────────────────────────────────────────────────────
# _message(): parts = content.split("] ", 2); return parts[-1] if len==3 else content
# Kills x__message__mutmut_2 (split(None, 2): whitespace split collapses the
#         leading space the original keeps),
#       x__message__mutmut_5 (split("] ") unlimited: a "] " inside the message
#         yields >3 parts -> returns full content),
#       x__message__mutmut_6 (rsplit: takes the rightmost "] " separators),
#       x__message__mutmut_8 (split("] ", 3): a 3rd "] " yields 4 parts).
# ─────────────────────────────────────────────────────────────────────────────
def test_message_splits_on_bracket_separator_not_whitespace():
    # Two spaces after the "[System] []" marker. The original splits on "] "
    # and KEEPS the leading space, so the message is " You inflicted ..." which
    # does NOT start with the "You inflicted" prefix -> no rule matches -> None.
    # Mutant 2 splits on whitespace, collapsing the space to "You inflicted ..."
    # which DOES match -> returns a DAMAGE_DEALT event. So the original yields
    # None here.
    line = "2026-03-24 14:30:15 [System] []  You inflicted 42.3 points of damage"
    assert parse_line(line) is None


def test_message_preserves_internal_bracket_separator_in_mission_name():
    # The mission name contains "[Repeatable] " -> an embedded "] " separator.
    # Original: split("] ", 2) stops after 2 splits, so the message is the full
    # "Mission completed (Paneleon [Repeatable] Hunt)" and parses correctly.
    # Mutant 5 (unlimited split) and mutant 8 (maxsplit 3) over-split -> len!=3
    # -> return full content (with the "[System] []" prefix) -> prefix check
    # fails -> None. Mutant 6 (rsplit) keeps only the tail "Hunt)" -> None.
    line = (
        "2026-03-24 17:31:58 [System] [] Mission completed (Paneleon [Repeatable] Hunt)"
    )
    event = parse_line(line)
    assert event is not None
    assert event.type == EventType.MISSION_COMPLETE
    assert event.data["mission_name"] == "Paneleon [Repeatable] Hunt"


# ─────────────────────────────────────────────────────────────────────────────
# parse_file(): open(path, "r", encoding="utf-8", errors="replace").
# Kills x_parse_file__mutmut_4 (errors=None -> strict -> UnicodeDecodeError),
#       x_parse_file__mutmut_7 (errors arg dropped -> strict -> UnicodeDecodeError),
#       x_parse_file__mutmut_13 (errors="XXreplaceXX" -> LookupError),
#       x_parse_file__mutmut_14 (errors="REPLACE" -> LookupError).
# The file holds an invalid UTF-8 byte inside an otherwise-valid loot line;
# only errors="replace" decodes it without raising.
# ─────────────────────────────────────────────────────────────────────────────
def test_parse_file_replaces_invalid_utf8_bytes(tmp_path):
    log_file = tmp_path / "chat.log"
    # 0xff is not valid UTF-8. With errors="replace" the line still decodes
    # (the byte becomes U+FFFD) and the loot line is recognised. With strict
    # decoding or an unknown error handler, .read()/iteration raises.
    log_file.write_bytes(
        b"2026-03-24 14:30:15 [System] [] You inflicted 42.3 points of damage\n"
        b"2026-03-24 14:31:00 [System] [] You received Animal\xff Muscle Oil "
        b"Value: 0.12 PED\n"
    )

    events = parse_file(str(log_file))

    # Must not raise, and must recover the clean damage line at minimum.
    assert len(events) >= 1
    assert events[0].type == EventType.DAMAGE_DEALT
    assert events[0].data["amount"] == 42.3
