"""Property-based tests for the quest service.

Covers three structural invariants of
``backend.services.quest_service.QuestService`` that hold for every valid
arrangement of quests, playlists, sessions, and completions:

1. ``_find_matching_playlists`` returns exactly the active playlists whose
   immediate set is non-empty, fully covered by the completed set, and whose
   combined scope still contains every completed quest.
2. ``get_session_link_suggestion`` is total: every call yields a defined dict
   drawn from a fixed reason vocabulary, and ``accept_session_link_suggestion``
   raises ``ValueError`` exactly when that classification is not linkable.
3. Both analytics surfaces ignore any session that was never accepted into the
   curated link table (absent entirely, or present only as ``declined``).

The service is a thin layer over SQLite, so each example builds a fresh
on-disk database. Generated inputs are exercised through the public API
(``create_quest`` / ``create_playlist`` / ``accept_session_link_suggestion``)
plus the same direct completion-row inserts the sibling suite uses, so the
properties run against real production query paths.
"""

import contextlib

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from backend.db.app_database import AppDatabase
from backend.services.quest_service import (
    PLAYLIST_GROUP_IMMEDIATE,
    PLAYLIST_GROUP_LONG_HORIZON,
    QuestService,
)

# tracking_sessions / kills / skill_gains are owned by the tracker at runtime;
# the quest tables come from AppDatabase. Mirror the sibling suite and create
# the tracker-side tables the analytics queries read from.
_TRACKER_SCHEMA = """
    CREATE TABLE IF NOT EXISTS tracking_sessions (
        id TEXT PRIMARY KEY, started_at REAL, ended_at REAL,
        is_active INTEGER DEFAULT 1, notes TEXT, quest_id INTEGER,
        armour_cost REAL DEFAULT 0, heal_cost REAL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS kills (
        id TEXT PRIMARY KEY, session_id TEXT,
        timestamp REAL, mob_name TEXT,
        loot_total_ped REAL DEFAULT 0,
        enhancer_cost REAL DEFAULT 0,
        cost_ped REAL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS kill_tool_stats (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kill_id TEXT, tool_name TEXT,
        cost_per_shot REAL DEFAULT 0, shots_fired INTEGER DEFAULT 0);
    CREATE TABLE IF NOT EXISTS skill_gains (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT, timestamp REAL,
        skill_name TEXT, amount REAL, ped_value REAL);
"""

# The full set of reasons get_session_link_suggestion may emit, partitioned by
# the suggestion_type it pairs with. Anything outside this map is a defect.
_LINKABLE_REASONS = {"single_quest", "exact_playlist"}
_NONE_REASONS = {
    "already_linked",
    "declined",
    "no_completions",
    "ambiguous_playlist",
    "unclean",
}


_tmp_factory: pytest.TempPathFactory


@pytest.fixture(autouse=True)
def _bind_tmp_factory(tmp_path_factory: pytest.TempPathFactory) -> None:
    """Root the module's DB temp dirs under pytest's auto-rotated basetemp.

    ``_make_service`` is a plain helper called per generated example from the
    property test bodies, not through the fixture protocol, so it cannot
    request ``tmp_path_factory`` itself. Binding it here keeps every
    helper-created dir under the tree pytest prunes, instead of the OS temp
    directory an interrupted run never cleans.
    """
    global _tmp_factory
    _tmp_factory = tmp_path_factory


def _make_service() -> QuestService:
    """A QuestService over a fresh on-disk DB with the tracker tables present."""
    tmp = _tmp_factory.mktemp("quests") / "quests.db"
    db = AppDatabase(tmp)
    db.conn.executescript(_TRACKER_SCHEMA)
    db.conn.commit()
    return QuestService(db)


def _record_completion(svc: QuestService, session_id: str, quest_id: int) -> None:
    svc._conn.execute(
        "INSERT OR IGNORE INTO session_quest_completions "
        "(session_id, quest_id) VALUES (?, ?)",
        (session_id, quest_id),
    )
    svc._conn.commit()


def _create_finished_session(svc: QuestService, session_id: str) -> None:
    svc._conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) "
        "VALUES (?, ?, ?, 0)",
        (session_id, 1000.0, 2000.0),
    )
    svc._conn.commit()


# A playlist plan: a partition of quest indices into immediate and long-horizon
# groups. Indices reference quests created up front; overlap between the two
# groups is allowed (the production code does not dedupe), which the subset
# checks must tolerate.
def _playlist_plan(n_quests):
    idx = st.integers(min_value=0, max_value=n_quests - 1)
    return st.fixed_dictionaries(
        {
            "immediate": st.lists(idx, min_size=0, max_size=n_quests, unique=True),
            "long_horizon": st.lists(idx, min_size=0, max_size=n_quests, unique=True),
        }
    )


@st.composite
def _world(draw):
    """A small world: a fixed pool of quests plus a handful of playlists."""
    n_quests = draw(st.integers(min_value=1, max_value=5))
    plans = draw(st.lists(_playlist_plan(n_quests), min_size=0, max_size=4))
    return n_quests, plans


# --- _find_matching_playlists ---


@settings(max_examples=60)
@given(world=_world(), completed=st.lists(st.integers(min_value=0, max_value=4)))
def test_playlist_match_is_immediate_subset_within_scope(world, completed):
    n_quests, plans = world
    svc = _make_service()
    quest_ids = [svc.create_quest({"name": f"Q{i}"})["id"] for i in range(n_quests)]

    playlist_groups: dict[object, tuple[set, set]] = {}
    for plan in plans:
        items = [
            {"quest_id": quest_ids[i], "group_type": PLAYLIST_GROUP_IMMEDIATE}
            for i in plan["immediate"]
        ] + [
            {"quest_id": quest_ids[i], "group_type": PLAYLIST_GROUP_LONG_HORIZON}
            for i in plan["long_horizon"]
        ]
        pl = svc.create_playlist({"name": f"PL{len(playlist_groups)}", "items": items})
        playlist_groups[pl["id"]] = (
            {quest_ids[i] for i in plan["immediate"]},
            {quest_ids[i] for i in plan["long_horizon"]},
        )

    # Map generated quest indices (0..4) onto the real ids, dropping any that
    # exceed the pool so the completed list is always a valid id set.
    completed_ids = [quest_ids[i] for i in completed if i < n_quests]
    completed_set = set(completed_ids)

    matched = set(svc._find_matching_playlists(completed_ids))

    for playlist_id, (immediate_set, long_horizon_set) in playlist_groups.items():
        scope = immediate_set | long_horizon_set
        expected = (
            bool(immediate_set)
            and immediate_set.issubset(completed_set)
            and completed_set.issubset(scope)
        )
        assert (playlist_id in matched) is expected


# --- get_session_link_suggestion / accept_session_link_suggestion ---


@given(
    n_quests=st.integers(min_value=0, max_value=4),
    completed_idx=st.lists(st.integers(min_value=0, max_value=3), max_size=6),
    pre_action=st.sampled_from(["none", "accept", "decline"]),
)
def test_suggestion_classification_is_total(n_quests, completed_idx, pre_action):
    svc = _make_service()
    quest_ids = [svc.create_quest({"name": f"Q{i}"})["id"] for i in range(n_quests)]
    # A playlist over the first two quests, when present, lets the >1 branch
    # reach both the exact_playlist and ambiguous/unclean outcomes.
    if n_quests >= 2:
        svc.create_playlist({"name": "PL", "quest_ids": quest_ids[:2]})

    session_id = "sess-1"
    _create_finished_session(svc, session_id)
    for i in completed_idx:
        if i < n_quests:
            _record_completion(svc, session_id, quest_ids[i])

    # Optionally drive the session through accept/decline first so the
    # already-linked and declined branches are exercised too. accept may be
    # impossible (no linkable suggestion); tolerate that and fall back.
    if pre_action == "decline":
        svc.decline_session_link(session_id)
    elif pre_action == "accept":
        with contextlib.suppress(ValueError):
            svc.accept_session_link_suggestion(session_id)

    suggestion = svc.get_session_link_suggestion(session_id)

    # Totality: a defined dict with a recognised (type, reason) pairing.
    assert set(suggestion).issuperset(
        {"suggestion_type", "reason", "quest_id", "playlist_id"}
    )
    stype = suggestion["suggestion_type"]
    reason = suggestion["reason"]
    if stype == "quest":
        assert reason == "single_quest"
    elif stype == "playlist":
        assert reason == "exact_playlist"
    else:
        assert stype == "none"
        assert reason in _NONE_REASONS

    # accept raises ValueError iff (and only iff) the type is not linkable.
    if stype in ("quest", "playlist"):
        assert reason in _LINKABLE_REASONS
        accepted = svc.accept_session_link_suggestion(session_id)
        assert accepted["suggestion_type"] == stype
    else:
        try:
            svc.accept_session_link_suggestion(session_id)
            raised = False
        except ValueError:
            raised = True
        assert raised


# --- analytics gate ---


@given(
    n_quests=st.integers(min_value=1, max_value=3),
    completed_idx=st.lists(
        st.integers(min_value=0, max_value=2), min_size=1, max_size=4
    ),
    decline=st.booleans(),
)
def test_analytics_ignore_never_accepted_sessions(n_quests, completed_idx, decline):
    svc = _make_service()
    quest_ids = [
        svc.create_quest({"name": f"Q{i}", "reward_ped": float(i + 1)})["id"]
        for i in range(n_quests)
    ]
    pl = svc.create_playlist({"name": "PL", "quest_ids": quest_ids})

    session_id = "sess-1"
    _create_finished_session(svc, session_id)
    for i in completed_idx:
        if i < n_quests:
            _record_completion(svc, session_id, quest_ids[i])

    # The session has raw completions but is never accepted (optionally
    # explicitly declined). Neither state may surface in analytics.
    if decline:
        svc.decline_session_link(session_id)

    assert svc.get_quest_analytics() == []

    playlist_stats = svc.get_playlist_analytics(pl["id"])
    assert playlist_stats is not None
    assert playlist_stats["matched_sessions"] == 0
    assert playlist_stats["total_reward_ped"] == 0
    assert playlist_stats["loot_tt"] == 0
    assert playlist_stats["total_duration"] == 0
