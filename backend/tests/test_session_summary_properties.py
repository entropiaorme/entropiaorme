"""Property-based tests for the per-session prospect summary service.

Covers ``backend.services.session_summary``: ``compute_session_summary``
(the eager derive-from-tracking-tables surface) and the
``load_prospect_sessions`` read path that lazily rebuilds materialised rows.

The summary is a cache of derived state whose source of truth is the tracking
tables, so each test seeds an in-memory database with a qualifying session
(at least one positive skill gain, positive cycled PED, positive duration)
and asserts a structural invariant over the returned dict. Inputs are
generated directly into the tables rather than driven through the live
tracker: the invariants under test are properties of the aggregation, so
spanning the table state space exercises them more thoroughly than replaying
a fixed event script would.
"""

from __future__ import annotations

import sqlite3
import uuid

import pytest
from hypothesis import given
from hypothesis import strategies as st

from backend.services.character_calc import ATTRIBUTE_SKILLS
from backend.services.session_summary import (
    compute_session_summary,
    load_prospect_sessions,
    write_session_summary,
)
from backend.tracking.schema import init_tracking_tables

_REGULAR_SKILLS = ["Laser Weaponry Technology", "Anatomy", "Dexterity", "Wounding"]
_ATTRIBUTES = sorted(ATTRIBUTE_SKILLS)
_MOB_NAMES = ["Atrox", "Daikiba", "Snablesnot", "Combibo"]
_TOOL_NAMES = ["Sollomate Opalo", "CB5 Regular", "Breer P1a"]

_POSITIVE = st.floats(
    min_value=0.0001, max_value=10000.0, allow_nan=False, allow_infinity=False
)
_NONNEG = st.floats(
    min_value=0.0, max_value=10000.0, allow_nan=False, allow_infinity=False
)
_COUNT = st.integers(min_value=1, max_value=20)


def _fresh_db() -> sqlite3.Connection:
    """In-memory DB with the tracking schema plus the skill_gains table.

    ``skill_gains`` lives in the app database in production; the summary
    service only reads it, so the minimal column set defined here is
    sufficient to drive the aggregation.
    """
    conn = sqlite3.connect(":memory:")
    init_tracking_tables(conn)
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS skill_gains (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id    TEXT NOT NULL,
            timestamp     REAL NOT NULL,
            skill_name    TEXT NOT NULL,
            amount        REAL NOT NULL,
            ped_value     REAL
        );
        """
    )
    conn.commit()
    return conn


def _insert_session(
    conn: sqlite3.Connection,
    session_id: str,
    started_at: float,
    ended_at: float,
    *,
    armour_cost: float = 0.0,
    heal_cost: float = 0.0,
    dangling_cost: float = 0.0,
) -> None:
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, ?, 0, ?, ?, ?)",
        (session_id, started_at, ended_at, armour_cost, heal_cost, dangling_cost),
    )


def _insert_kill(
    conn: sqlite3.Connection,
    kill_id: str,
    session_id: str,
    *,
    mob_name: str,
    species: str = "",
    maturity: str = "",
    timestamp: float = 0.0,
    enhancer_cost: float = 0.0,
    loot_total_ped: float = 0.0,
) -> None:
    conn.execute(
        "INSERT INTO kills "
        "(id, session_id, mob_name, mob_species, mob_maturity, timestamp, "
        "enhancer_cost, loot_total_ped) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (
            kill_id,
            session_id,
            mob_name,
            species,
            maturity,
            timestamp,
            enhancer_cost,
            loot_total_ped,
        ),
    )


def _insert_tool_stat(
    conn: sqlite3.Connection,
    kill_id: str,
    tool_name: str,
    *,
    shots_fired: int,
    cost_per_shot: float,
) -> None:
    conn.execute(
        "INSERT INTO kill_tool_stats "
        "(kill_id, tool_name, shots_fired, cost_per_shot) VALUES (?, ?, ?, ?)",
        (kill_id, tool_name, shots_fired, cost_per_shot),
    )


def _insert_skill_gain(
    conn: sqlite3.Connection,
    session_id: str,
    skill_name: str,
    amount: float,
    ped_value: float | None,
) -> None:
    conn.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) "
        "VALUES (?, 0.0, ?, ?, ?)",
        (session_id, skill_name, amount, ped_value),
    )


# Strategies that build a whole qualifying session as DB rows. Each returns the
# connection and the session id so the property can compute and assert.

# Regular-skill gains: name -> positive ped_value. At least one entry, which
# (together with one positive cycled-PED component) guarantees the session
# qualifies and a dict is returned.
_regular_gains = st.dictionaries(
    keys=st.sampled_from(_REGULAR_SKILLS),
    values=_POSITIVE,
    min_size=1,
    max_size=len(_REGULAR_SKILLS),
)

# Attribute gains: subset of ATTRIBUTE_SKILLS -> positive amount.
_attribute_gains = st.dictionaries(
    keys=st.sampled_from(_ATTRIBUTES),
    values=_POSITIVE,
    min_size=1,
    max_size=len(_ATTRIBUTES),
)


def _seed_qualifying(
    conn: sqlite3.Connection,
    session_id: str,
    *,
    regular: dict[str, float],
    attributes: dict[str, float] | None = None,
    weapon_cost: float = 1.0,
) -> None:
    """Seed one session guaranteed to pass compute's qualifying filters.

    A single kill carries the weapon cost via one tool-stat row (one shot at
    ``weapon_cost``), so cycled PED is strictly positive. Duration is one hour.
    """
    _insert_session(conn, session_id, started_at=0.0, ended_at=3600.0)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, session_id, mob_name=_MOB_NAMES[0])
    _insert_tool_stat(
        conn, kill_id, _TOOL_NAMES[0], shots_fired=1, cost_per_shot=weapon_cost
    )
    for name, ped in regular.items():
        # ped_value carries the TT value; amount is incidental to these gains.
        _insert_skill_gain(conn, session_id, name, amount=0.01, ped_value=ped)
    if attributes:
        for name, amount in attributes.items():
            # Attributes carry NULL ped_value in production; the breakdown is
            # built from SUM(amount), so seed amount and leave ped_value NULL.
            _insert_skill_gain(conn, session_id, name, amount=amount, ped_value=None)
    conn.commit()


# --- regularSkillTt mirrors its breakdown (holds) ---


@given(_regular_gains)
def test_regular_skill_tt_equals_rounded_sum_of_breakdown(regular):
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid, regular=regular)
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    breakdown = summary["regularSkillPed"]
    assert summary["regularSkillTt"] == pytest.approx(round(sum(breakdown.values()), 4))
    # Every surviving per-skill value is strictly positive (the > 0 filter).
    for value in breakdown.values():
        assert value > 0.0


# --- attributeLevelsTotal mirrors its breakdown (holds) ---


@given(_regular_gains, _attribute_gains)
def test_attribute_levels_total_equals_rounded_sum_of_breakdown(regular, attributes):
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid, regular=regular, attributes=attributes)
    summary = compute_session_summary(conn, sid)
    assert summary is not None
    breakdown = summary["attributeLevels"]
    assert summary["attributeLevelsTotal"] == pytest.approx(
        round(sum(breakdown.values()), 4)
    )
    # Keys are a strict subset of the attribute set, all values positive.
    assert set(breakdown).issubset(ATTRIBUTE_SKILLS)
    for value in breakdown.values():
        assert value > 0.0


# --- dominantMob and dominantTag are mutually exclusive (holds unconditionally) ---
#
# The needs-qualification caveat (a name-dominant mob can be reported as
# neither) only arises when a post-hoc rename splits one logical mob across
# differing species/maturity rows; the seed below never constructs that split,
# so only the unconditional headline (never both non-null) is asserted.


@st.composite
def _mixed_kill_session(draw):
    """A session whose kills mix species-tagged and tag-only mobs.

    Each kill independently either carries a species (drives the dominant_mob
    branch) or is blank (drives the dominant_tag branch), so the top group can
    land in either branch across examples without ever splitting one logical
    mob name across both forms.
    """
    regular = draw(_regular_gains)
    n_kills = draw(_COUNT)
    # Each kill: (mob_name, has_species). Keep one logical name per (name,form)
    # pairing by deriving species/maturity deterministically from the flag.
    kills = draw(
        st.lists(
            st.tuples(st.sampled_from(_MOB_NAMES), st.booleans()),
            min_size=n_kills,
            max_size=n_kills,
        )
    )
    return regular, kills


@given(_mixed_kill_session())
def test_dominant_mob_and_tag_are_never_both_set(payload):
    regular, kills = payload
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
    for idx, (mob_name, has_species) in enumerate(kills):
        kill_id = uuid.uuid4().hex
        species = mob_name if has_species else ""
        maturity = "Young" if has_species else ""
        cost = 1.0 if idx == 0 else 0.0
        _insert_kill(
            conn,
            kill_id,
            sid,
            mob_name=mob_name,
            species=species,
            maturity=maturity,
            timestamp=float(idx),
        )
        _insert_tool_stat(
            conn, kill_id, _TOOL_NAMES[0], shots_fired=1, cost_per_shot=cost
        )
    for name, ped in regular.items():
        _insert_skill_gain(conn, sid, name, amount=0.01, ped_value=ped)
    conn.commit()

    summary = compute_session_summary(conn, sid)
    assert summary is not None
    assert not (
        summary["dominantMob"] is not None and summary["dominantTag"] is not None
    )


# --- durationHours is never negative (holds) ---
#
# compute clamps the (ended_at - started_at) delta to >= 0 and rejects the
# clamped-to-zero case, so a returned summary always has a positive duration
# even when the timestamps are out of order.


@given(_regular_gains, _NONNEG, _NONNEG)
def test_duration_hours_is_non_negative(regular, started_at, ended_at):
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _insert_session(conn, sid, started_at=started_at, ended_at=ended_at)
    kill_id = uuid.uuid4().hex
    _insert_kill(conn, kill_id, sid, mob_name=_MOB_NAMES[0])
    _insert_tool_stat(conn, kill_id, _TOOL_NAMES[0], shots_fired=1, cost_per_shot=1.0)
    for name, ped in regular.items():
        _insert_skill_gain(conn, sid, name, amount=0.01, ped_value=ped)
    conn.commit()

    summary = compute_session_summary(conn, sid)
    if summary is None:
        # Non-positive duration (e.g. ended_at <= started_at) is filtered out
        # entirely; there is nothing to violate the invariant.
        return
    assert summary["durationHours"] >= 0.0


# --- materialised kills/loot mirror the kills table (holds, existence-scoped) ---
#
# Scoped to rows actually present after load_prospect_sessions, per the
# needs-qualification: non-qualifying sessions produce no summary row, so the
# property is over materialised rows only.


@given(
    st.lists(
        st.tuples(st.sampled_from(_MOB_NAMES), _NONNEG, _COUNT),
        min_size=1,
        max_size=3,
    ),
    _regular_gains,
)
def test_materialised_rows_mirror_kills_and_loot(sessions, regular):
    conn = _fresh_db()
    session_ids: list[str] = []
    for mob_name, loot_each, n_kills in sessions:
        sid = uuid.uuid4().hex
        session_ids.append(sid)
        _insert_session(conn, sid, started_at=0.0, ended_at=3600.0)
        for k in range(n_kills):
            kill_id = uuid.uuid4().hex
            cost = 1.0 if k == 0 else 0.0
            _insert_kill(
                conn,
                kill_id,
                sid,
                mob_name=mob_name,
                timestamp=float(k),
                loot_total_ped=loot_each,
            )
            _insert_tool_stat(
                conn, kill_id, _TOOL_NAMES[0], shots_fired=1, cost_per_shot=cost
            )
        for name, ped in regular.items():
            _insert_skill_gain(conn, sid, name, amount=0.01, ped_value=ped)
    conn.commit()

    rows = load_prospect_sessions(conn)
    by_id = {row["id"]: row for row in rows}
    assert by_id, "at least one qualifying session should materialise"
    for sid in by_id:
        live = conn.execute(
            "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0) "
            "FROM kills WHERE session_id = ?",
            (sid,),
        ).fetchone()
        assert by_id[sid]["kills"] == int(live[0])
        assert by_id[sid]["lootTt"] == pytest.approx(float(live[1]))


def test_write_then_load_is_idempotent_on_materialised_count():
    """A degenerate single-session sanity check around the write/load pair.

    Guards that an explicit write followed by a load surfaces exactly the
    rows the kills table backs, with no duplicate materialisation.
    """
    conn = _fresh_db()
    sid = uuid.uuid4().hex
    _seed_qualifying(conn, sid, regular={_REGULAR_SKILLS[0]: 5.0})
    write_session_summary(conn, sid)
    conn.commit()
    rows = load_prospect_sessions(conn)
    assert len(rows) == 1
    assert rows[0]["id"] == sid
