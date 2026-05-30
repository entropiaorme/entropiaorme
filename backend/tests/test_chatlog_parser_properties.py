"""Property-based tests for the chat.log parser.

Covers ``backend.services.chatlog_parser``: fuzz-safety (never raises, returns
None on non-matches) and per-EventType round-trips through canonical lines,
including the precedence rules between overlapping patterns.
"""

import string
from datetime import datetime

import pytest
from hypothesis import assume, given
from hypothesis import strategies as st

from backend.services.chatlog_parser import EventType, parse_line

_TIMESTAMPS = st.datetimes(
    min_value=datetime(2001, 1, 1), max_value=datetime(2099, 12, 31, 23, 59, 59)
)
_AMOUNT = st.floats(
    min_value=0.0, max_value=100000.0, allow_nan=False, allow_infinity=False
)
# Single-word names of letters only: unambiguous against every delimiter the
# parser keys on (no spaces, brackets, or "Value:" / " x (" sequences).
_WORD = st.text(alphabet=string.ascii_letters, min_size=1, max_size=12)


def _ts(dt: datetime) -> str:
    return dt.strftime("%Y-%m-%d %H:%M:%S")


def _system_line(dt: datetime, message: str) -> str:
    return f"{_ts(dt)} [System] [] {message}"


def _globals_line(dt: datetime, body: str) -> str:
    return f"{_ts(dt)} [Globals] [] {body}"


# --- fuzz safety ---


@given(st.text())
def test_parse_line_never_raises_on_arbitrary_text(text):
    parse_line(text)  # must not raise


@given(_TIMESTAMPS, st.text(max_size=80))
def test_valid_timestamp_with_unmarked_content_returns_none(dt, garbage):
    assume("[System] []" not in garbage and "[Globals]" not in garbage)
    assert parse_line(f"{_ts(dt)} {garbage}") is None


# --- amount events ---

_AMOUNT_EVENTS = [
    ("You inflicted {a} points of damage", EventType.DAMAGE_DEALT),
    ("You took {a} points of damage", EventType.DAMAGE_RECEIVED),
    ("You healed yourself {a} points", EventType.SELF_HEAL),
]


@pytest.mark.parametrize("template,event_type", _AMOUNT_EVENTS)
@given(dt=_TIMESTAMPS, amount=_AMOUNT)
def test_amount_event_round_trip(template, event_type, dt, amount):
    text = f"{amount:.2f}"
    line = _system_line(dt, template.format(a=text))
    event = parse_line(line)
    assert event is not None
    assert event.type is event_type
    assert event.data["amount"] == pytest.approx(float(text))
    assert event.timestamp == dt.replace(microsecond=0)
    assert event.raw_line == line


def test_critical_hit_takes_precedence_over_damage_dealt():
    line = _system_line(
        datetime(2025, 1, 1, 12, 0, 0),
        "Critical hit - Additional damage! You inflicted 50.00 points of damage",
    )
    event = parse_line(line)
    assert event is not None
    assert event.type is EventType.CRITICAL_HIT
    assert event.data["amount"] == pytest.approx(50.0)


# --- zero-argument events ---

_ZERO_ARG_EVENTS = [
    ("The target Dodged your attack", EventType.TARGET_DODGE),
    ("The target Evaded your attack", EventType.TARGET_EVADE),
    ("The target Jammed your attack", EventType.TARGET_JAM),
    ("You Dodged the attack", EventType.PLAYER_DODGE),
    ("You Evaded the attack", EventType.PLAYER_EVADE),
    ("You Jammed the attack", EventType.PLAYER_JAM),
    ("The attack missed you", EventType.MOB_MISS),
    ("Damage deflected!", EventType.DEFLECT),
]


@pytest.mark.parametrize("message,event_type", _ZERO_ARG_EVENTS)
@given(dt=_TIMESTAMPS)
def test_zero_argument_event_round_trip(message, event_type, dt):
    event = parse_line(_system_line(dt, message))
    assert event is not None
    assert event.type is event_type
    assert event.data == {}


# --- loot ---


@given(_TIMESTAMPS, _WORD, _AMOUNT)
def test_loot_without_quantity_round_trip(dt, name, value):
    text = f"{value:.2f}"
    event = parse_line(_system_line(dt, f"You received {name} Value: {text} PED"))
    assert event is not None
    assert event.type is EventType.LOOT
    assert event.data["item_name"] == name
    assert event.data["quantity"] == 1
    assert event.data["value"] == pytest.approx(float(text))


@given(_TIMESTAMPS, _WORD, st.integers(min_value=1, max_value=99999), _AMOUNT)
def test_loot_with_quantity_round_trip(dt, name, quantity, value):
    text = f"{value:.2f}"
    event = parse_line(
        _system_line(dt, f"You received {name} x ({quantity}) Value: {text} PED")
    )
    assert event is not None
    assert event.type is EventType.LOOT
    assert event.data["item_name"] == name
    assert event.data["quantity"] == quantity
    assert event.data["value"] == pytest.approx(float(text))


# --- skill gain / enhancer break ---


@given(_TIMESTAMPS, _WORD, _AMOUNT)
def test_skill_gain_verbose_round_trip(dt, skill, amount):
    text = f"{amount:.2f}"
    event = parse_line(
        _system_line(dt, f"You have gained {text} experience in your {skill} skill")
    )
    assert event is not None
    assert event.type is EventType.SKILL_GAIN
    assert event.data["amount"] == pytest.approx(float(text))
    assert event.data["skill_name"] == skill


@given(_TIMESTAMPS, _WORD, _WORD, st.integers(min_value=0, max_value=99), _AMOUNT)
def test_enhancer_break_round_trip(dt, enhancer, item, remaining, shrapnel):
    text = f"{shrapnel:.2f}"
    message = (
        f"Your enhancer {enhancer} on your {item} broke. "
        f"You have {remaining} enhancers remaining on the item. "
        f"You received {text} PED Shrapnel."
    )
    event = parse_line(_system_line(dt, message))
    assert event is not None
    assert event.type is EventType.ENHANCER_BREAK
    assert event.data["enhancer_name"] == enhancer
    assert event.data["item_name"] == item
    assert event.data["remaining"] == remaining
    assert event.data["shrapnel_ped"] == pytest.approx(float(text))


# --- globals: kill / item precedence ---


@given(_TIMESTAMPS, _WORD, _WORD, _AMOUNT)
def test_global_vs_hof_kill_precedence(dt, player, creature, value):
    text = f"{value:.2f}"
    base = f"{player} killed a creature ({creature}) with a value of {text} PED!"
    plain = parse_line(_globals_line(dt, base))
    hof = parse_line(
        _globals_line(dt, base + " A record has been added to the Hall of Fame!")
    )
    assert plain is not None and plain.type is EventType.GLOBAL_KILL
    assert hof is not None and hof.type is EventType.HOF_KILL
    for event in (plain, hof):
        assert event.data["player"] == player
        assert event.data["creature"] == creature
        assert event.data["value"] == pytest.approx(float(text))


@given(_TIMESTAMPS, _WORD, _WORD, _AMOUNT, st.sampled_from(["PED", "PEC"]))
def test_global_vs_hof_item_precedence(dt, player, item, value, denom):
    text = f"{value:.2f}"
    base = f"{player} has found a rare item ({item}) with a value of {text} {denom}!"
    plain = parse_line(_globals_line(dt, base))
    hof = parse_line(
        _globals_line(dt, base + " A record has been added to the Hall of Fame!")
    )
    assert plain is not None and plain.type is EventType.GLOBAL_ITEM
    assert hof is not None and hof.type is EventType.HOF_ITEM
    for event in (plain, hof):
        assert event.data["player"] == player
        assert event.data["item"] == item
        assert event.data["value"] == pytest.approx(float(text))


_HOF_SUFFIX = " A record has been added to the Hall of Fame!"

# A free-text decoy that the leading (.+?) capture may have to absorb before
# locking onto the genuine trailing global clause. It is deliberately allowed to
# embed value-like tokens, but must not itself smuggle in a complete second
# global clause (the parser is only specified over real single-clause lines) or
# the HoF suffix. The trailing space joins it cleanly to the player name.
_DECOY = st.text(
    alphabet=string.ascii_letters + string.digits + " .,!?:;",
    min_size=0,
    max_size=40,
).map(lambda s: f"{s} " if s else "")


def _decoy_ok(decoy: str) -> bool:
    return (
        "killed a creature" not in decoy
        and "has found a rare item" not in decoy
        and "Hall of Fame" not in decoy
    )


@given(_TIMESTAMPS, _DECOY, _WORD, _WORD, _AMOUNT)
def test_hof_kill_wins_over_plain_global_under_decoy_prefix(
    dt, decoy, player, creature, value
):
    # The surviving invariant: any valid global KILL line that ends with the
    # HoF suffix classifies as HOF_KILL, never the plain GLOBAL_KILL, even when
    # a decoy prefix forces the leading (.+?) capture to backtrack. The value
    # field carries no thousands separator (real EU globals never do), so the
    # line is always a recognised global rather than a parse miss.
    assume(_decoy_ok(decoy))
    text = f"{value:.2f}"
    body = (
        f"{decoy}{player} killed a creature ({creature}) "
        f"with a value of {text} PED!{_HOF_SUFFIX}"
    )
    event = parse_line(_globals_line(dt, body))
    assert event is not None
    assert event.type is EventType.HOF_KILL


@given(_TIMESTAMPS, _DECOY, _WORD, _WORD, _AMOUNT, st.sampled_from(["PED", "PEC"]))
def test_hof_item_wins_over_plain_global_under_decoy_prefix(
    dt, decoy, player, item, value, denom
):
    # As above for the rare-item global: the HoF rule precedes the plain rule in
    # GLOBAL_RULES order, so any HoF-suffixed item line resolves to HOF_ITEM.
    assume(_decoy_ok(decoy))
    text = f"{value:.2f}"
    body = (
        f"{decoy}{player} has found a rare item ({item}) "
        f"with a value of {text} {denom}!{_HOF_SUFFIX}"
    )
    event = parse_line(_globals_line(dt, body))
    assert event is not None
    assert event.type is EventType.HOF_ITEM
