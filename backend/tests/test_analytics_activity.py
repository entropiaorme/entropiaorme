"""Unit cover for the analytics activity-comparison endpoint.

Exercises the per-group ranking helper and the ``activity_impl`` assembler so
the activity surface has direct coverage, independent of the broader analytics
overview regression tests.
"""

from pathlib import Path

from backend.db.app_database import AppDatabase
from backend.routers.analytics import _build_activity_slice_rows, activity_impl
from backend.tracking.schema import init_tracking_tables


def _session(mob, kills, hours, cycled, loot_tt, skill_tt):
    return {
        "dominantMob": mob,
        "dominantMobKills": kills,
        "durationHours": hours,
        "cycledPed": cycled,
        "lootTt": loot_tt,
        "skillTt": skill_tt,
    }


def test_build_activity_slice_rows_groups_ranks_and_rates():
    sessions = [
        _session("Atrox", 10, 2.0, 100.0, 90.0, 5.0),
        _session("Atrox", 5, 1.0, 50.0, 40.0, 2.0),
        _session("Daikiba", 3, 0.5, 20.0, 15.0, 1.0),
        _session(None, 99, 9.0, 9.0, 9.0, 9.0),  # falsy group key -> dropped
    ]

    rows = _build_activity_slice_rows(
        sessions,
        key="dominantMob",
        kills_key="dominantMobKills",
        name_field="mobName",
    )

    # The None-keyed session is dropped; two mob groups remain, ranked by kills.
    assert [row["mobName"] for row in rows] == ["Atrox", "Daikiba"]

    atrox = rows[0]
    assert atrox["sessions"] == 2
    assert atrox["kills"] == 15
    assert atrox["cycled"] == 150.0
    # pesPer100Ped == (sum skill_tt / sum cycled) * 100 == (7 / 150) * 100
    assert atrox["pesPer100Ped"] == round((7.0 / 150.0) * 100, 2)
    # lootRate == sum loot_tt / sum cycled == 130 / 150
    assert atrox["lootRate"] == round(130.0 / 150.0, 4)


def test_activity_impl_on_empty_db_returns_empty_comparisons(tmp_path: Path):
    db = AppDatabase(tmp_path / "activity.db")
    init_tracking_tables(db.conn)

    result = activity_impl(db.conn)

    assert result == {
        "mobComparisons": [],
        "tagComparisons": [],
        "weaponComparisons": [],
    }
