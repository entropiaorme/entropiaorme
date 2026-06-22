"""Analytics overview aggregation tests.

Regression cover for the timeline / monthly tracking-cost series: every
session-level cost component that feeds the headline ``trackingCost`` must
also feed the per-bucket series, so the bars reconcile with the total.
"""

from datetime import UTC, datetime
from pathlib import Path

from backend.db.app_database import AppDatabase
from backend.routers.analytics import overview_impl
from backend.testing.clock import MockClock
from backend.tracking.schema import init_tracking_tables

# A fixed instant so the overview trend block's recent-30d/prior-30d windows
# are deterministic across UTC dates (the real clock made the measured trend
# branches drift run to run). The seeded data sits in the past relative to it.
_FIXED_CLOCK = MockClock(start=datetime(2026, 7, 1, 0, 0, 0, tzinfo=UTC))


def _epoch(year: int, month: int, day: int) -> float:
    return datetime(year, month, day, 12, 0, 0, tzinfo=UTC).timestamp()


def _seed_conn(tmp_path: Path):
    """Build a conn carrying both the app and tracking schemas, seeded with
    two sessions in distinct months, each with a non-zero dangling cost."""
    db = AppDatabase(tmp_path / "analytics.db")
    init_tracking_tables(db.conn)
    conn = db.conn

    # Session A (March): armour + heal + dangling, plus one kill carrying
    # weapon cost (via a tool stat), enhancer cost, and loot.
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, ?, 0, ?, ?, ?)",
        ("sess-a", _epoch(2026, 3, 15), _epoch(2026, 3, 15) + 3600, 1.0, 2.0, 3.0),
    )
    conn.execute(
        "INSERT INTO kills (id, session_id, timestamp, enhancer_cost, loot_total_ped) "
        "VALUES (?, ?, ?, ?, ?)",
        ("kill-a", "sess-a", _epoch(2026, 3, 15), 0.5, 10.0),
    )
    conn.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, cost_per_shot) "
        "VALUES (?, ?, ?, ?)",
        ("kill-a", "Weapon", 2, 0.5),  # weapon cost = 2 * 0.5 = 1.0
    )

    # Session B (April): armour + dangling only, no kills.
    conn.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, ?, 0, ?, ?, ?)",
        ("sess-b", _epoch(2026, 4, 15), _epoch(2026, 4, 15) + 3600, 0.5, 0.0, 1.5),
    )
    conn.commit()
    return conn


# weapon 1.0 + heal 2.0 + enhancer 0.5 + armour 1.5 + dangling 4.5
_EXPECTED_TRACKING_COST = 9.5


def test_headline_tracking_cost_includes_all_components(tmp_path):
    conn = _seed_conn(tmp_path)
    result = overview_impl(conn, "all", clock=_FIXED_CLOCK)
    assert result["lossesBreakdown"]["trackingCost"] == _EXPECTED_TRACKING_COST


def test_timeline_tracking_cost_reconciles_with_headline(tmp_path):
    """Daily trackingCost bars must sum to the headline (regression: the
    daily series previously omitted dangling cost)."""
    conn = _seed_conn(tmp_path)
    result = overview_impl(conn, "all", clock=_FIXED_CLOCK)
    headline = result["lossesBreakdown"]["trackingCost"]
    timeline_total = sum(day["trackingCost"] for day in result["timeline"])
    assert round(timeline_total, 4) == round(headline, 4)


def test_monthly_tracking_cost_reconciles_with_headline(tmp_path):
    """Monthly trackingCost bars must sum to the headline (regression: the
    monthly series previously omitted dangling cost)."""
    conn = _seed_conn(tmp_path)
    result = overview_impl(conn, "all", clock=_FIXED_CLOCK)
    headline = result["lossesBreakdown"]["trackingCost"]
    monthly_total = sum(m["trackingCost"] for m in result["monthlyBreakdown"])
    assert round(monthly_total, 4) == round(headline, 4)
