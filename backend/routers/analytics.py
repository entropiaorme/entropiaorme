"""Analytics endpoints — overview, activity comparisons, and ledger CRUD.

Returns shapes matching the frontend analytics types.
"""

import time
import uuid
from collections import defaultdict
from datetime import UTC, datetime
from typing import Any

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from backend.dependencies import get_services
from backend.routers.response_models import AnalyticsOverview

router = APIRouter(prefix="/analytics", tags=["analytics"])
ACTIVITY_DOMINANCE_THRESHOLD = 0.6

INVENTORY_SALE_TAG = "inventory_sale"


# ------------------------------------------------------------------
# Overview
# ------------------------------------------------------------------


def _period_epoch(period: str | None) -> float | None:
    """Return epoch start for a named period, or None for all-time."""
    if not period or period == "all":
        return None
    days = {"30d": 30, "90d": 90, "1y": 365}.get(period)
    return (time.time() - days * 86400) if days else None


def _epoch_to_iso(epoch: float) -> str:
    return datetime.fromtimestamp(epoch, tz=UTC).strftime("%Y-%m-%d")


def _where(col: str, epoch_start: float | None, epoch_end: float | None):
    """Build a WHERE clause + params for epoch-based columns."""
    parts, params = [], []
    if epoch_start is not None:
        parts.append(f"{col} >= ?")
        params.append(epoch_start)
    if epoch_end is not None:
        parts.append(f"{col} < ?")
        params.append(epoch_end)
    return (" AND ".join(parts) if parts else "1=1"), params


def _where_iso(col: str, epoch_start: float | None, epoch_end: float | None):
    """Build a WHERE clause + params for ISO-date TEXT columns."""
    parts, params = [], []
    if epoch_start is not None:
        parts.append(f"{col} >= ?")
        params.append(_epoch_to_iso(epoch_start))
    if epoch_end is not None:
        parts.append(f"{col} < ?")
        params.append(_epoch_to_iso(epoch_end))
    return (" AND ".join(parts) if parts else "1=1"), params


def _compute_metrics(conn, epoch_start: float | None, epoch_end: float | None) -> dict:
    """Compute gains/losses breakdown for a time window.

    Returns dict with loot_tt, skill_tt, codex_pes, quest_pes, tracking_cost, ledger_gains, ledger_losses.
    Codex PES is read from codex_claims, quest PES from quest_claims (both canonical
    PES sources rather than ledger_entries).
    """
    enc_w, enc_p = _where("k.timestamp", epoch_start, epoch_end)
    sg_w, sg_p = _where("sg.timestamp", epoch_start, epoch_end)
    led_w, led_p = _where_iso("le.date", epoch_start, epoch_end)
    cc_w, cc_p = _where("cc.claimed_at", epoch_start, epoch_end)
    qc_w, qc_p = _where("qc.claimed_at", epoch_start, epoch_end)

    loot_tt = conn.execute(
        f"SELECT COALESCE(SUM(k.loot_total_ped), 0) FROM kills k WHERE {enc_w}",
        enc_p,
    ).fetchone()[0]

    weapon_cost = conn.execute(
        f"""SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0)
            FROM kill_tool_stats ts
            JOIN kills k ON k.id = ts.kill_id
            WHERE {enc_w}""",
        enc_p,
    ).fetchone()[0]

    enhancer_cost = conn.execute(
        f"SELECT COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE {enc_w}",
        enc_p,
    ).fetchone()[0]

    # Heal cost + armour cost live on tracking_sessions (session-level)
    sess_w, sess_p = _where("s.started_at", epoch_start, epoch_end)
    sess_costs = conn.execute(
        f"""SELECT COALESCE(SUM(s.armour_cost), 0),
                   COALESCE(SUM(s.heal_cost), 0),
                   COALESCE(SUM(s.dangling_cost), 0)
            FROM tracking_sessions s WHERE {sess_w}""",
        sess_p,
    ).fetchone()
    armour_cost = sess_costs[0]
    heal_cost = sess_costs[1]
    dangling_cost = sess_costs[2]

    tracking_cost = (
        weapon_cost + heal_cost + enhancer_cost + armour_cost + dangling_cost
    )

    skill_tt = conn.execute(
        f"SELECT COALESCE(SUM(sg.ped_value), 0) FROM skill_gains sg WHERE {sg_w}",
        sg_p,
    ).fetchone()[0]

    codex_pes = conn.execute(
        f"SELECT COALESCE(SUM(cc.ped_value), 0) FROM codex_claims cc WHERE {cc_w}",
        cc_p,
    ).fetchone()[0]

    quest_pes = conn.execute(
        f"SELECT COALESCE(SUM(qc.ped_value), 0) FROM quest_claims qc WHERE {qc_w}",
        qc_p,
    ).fetchone()[0]

    markup_rows = conn.execute(
        f"""SELECT le.tag, COALESCE(SUM(le.amount), 0)
            FROM ledger_entries le WHERE le.type = 'markup' AND {led_w}
            GROUP BY le.tag""",
        led_p,
    ).fetchall()
    ledger_gains = {r[0]: round(r[1], 2) for r in markup_rows}

    expense_rows = conn.execute(
        f"""SELECT le.tag, COALESCE(SUM(le.amount), 0)
            FROM ledger_entries le WHERE le.type = 'expense' AND {led_w}
            GROUP BY le.tag""",
        led_p,
    ).fetchall()
    ledger_losses = {r[0]: round(r[1], 2) for r in expense_rows}

    return {
        "loot_tt": loot_tt,
        "skill_tt": skill_tt,
        "codex_pes": codex_pes,
        "quest_pes": quest_pes,
        "tracking_cost": tracking_cost,
        "cycled_breakdown": {
            "weapon": round(weapon_cost, 2),
            "healing": round(heal_cost, 2),
            "enhancer": round(enhancer_cost, 2),
            "armour": round(armour_cost, 2),
            "dangling": round(dangling_cost, 2),
        },
        "ledger_gains": ledger_gains,
        "ledger_losses": ledger_losses,
    }


def _rate_from_metrics(m: dict) -> float:
    # Progression denominations (skill_tt, codex_pes) are intentionally excluded —
    # the rate measures liquid economy, not avatar progress.
    total_gains = m["loot_tt"] + sum(m["ledger_gains"].values())
    total_losses = m["tracking_cost"] + sum(m["ledger_losses"].values())
    return total_gains / total_losses if total_losses > 0 else 0.0


@router.get("/overview", response_model=AnalyticsOverview)
def analytics_overview(period: str = "all"):
    """Cross-session sustainability metrics.

    Total Return = (Loot TT + Skill TT + Ledger markup) / (Tracking cost + Ledger expenses).
    ?period= all | 30d | 90d | 1y
    """
    return overview_impl(get_services().app_db.conn, period)


def overview_impl(conn, period: str = "all"):
    now = time.time()

    epoch_start = _period_epoch(period)

    # --- Main metrics for selected period ---
    m = _compute_metrics(conn, epoch_start, None)
    total_ledger_gains = sum(m["ledger_gains"].values())
    total_ledger_losses = sum(m["ledger_losses"].values())
    # Liquid totals only — skill_tt and codex_pes are progression and stay
    # out of P&L by default.
    total_gains = m["loot_tt"] + total_ledger_gains
    total_losses = m["tracking_cost"] + total_ledger_losses
    return_rate = total_gains / total_losses if total_losses > 0 else 0.0

    # --- Trend (always recent-30d vs prior-30d, independent of period) ---
    day_30 = now - 30 * 86400
    day_60 = now - 60 * 86400
    rate_30d = _rate_from_metrics(_compute_metrics(conn, day_30, None))
    rate_prior = _rate_from_metrics(_compute_metrics(conn, day_60, day_30))

    if rate_30d > 0 and rate_prior > 0:
        if rate_30d > rate_prior * 1.02:
            trend = "improving"
        elif rate_30d < rate_prior * 0.98:
            trend = "declining"
        else:
            trend = "stable"
    else:
        trend = "stable"

    # --- Per-source daily/monthly breakdowns (frontend aggregates by mode) ---
    enc_w, enc_p = _where("k.timestamp", epoch_start, None)
    sg_w, sg_p = _where("sg.timestamp", epoch_start, None)
    led_w, led_p = _where_iso("le.date", epoch_start, None)

    loot_by_day = dict(
        conn.execute(
            f"""SELECT date(k.timestamp, 'unixepoch') as day, COALESCE(SUM(k.loot_total_ped), 0)
            FROM kills k WHERE {enc_w} GROUP BY day""",
            enc_p,
        ).fetchall()
    )

    weapon_cost_by_day = dict(
        conn.execute(
            f"""SELECT date(k.timestamp, 'unixepoch') as day, COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0)
            FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id
            WHERE {enc_w} GROUP BY day""",
            enc_p,
        ).fetchall()
    )
    enhancer_cost_by_day = dict(
        conn.execute(
            f"""SELECT date(k.timestamp, 'unixepoch') as day,
                   COALESCE(SUM(k.enhancer_cost), 0)
            FROM kills k WHERE {enc_w} GROUP BY day""",
            enc_p,
        ).fetchall()
    )
    sess_w2, sess_p2 = _where("s.started_at", epoch_start, None)
    sess_cost_by_day = dict(
        conn.execute(
            f"""SELECT date(s.started_at, 'unixepoch') as day,
                   COALESCE(SUM(s.armour_cost), 0) + COALESCE(SUM(s.heal_cost), 0)
                   + COALESCE(SUM(s.dangling_cost), 0)
            FROM tracking_sessions s WHERE {sess_w2} GROUP BY day""",
            sess_p2,
        ).fetchall()
    )
    cost_by_day = {}
    for day in set(
        list(weapon_cost_by_day) + list(enhancer_cost_by_day) + list(sess_cost_by_day)
    ):
        cost_by_day[day] = (
            weapon_cost_by_day.get(day, 0)
            + enhancer_cost_by_day.get(day, 0)
            + sess_cost_by_day.get(day, 0)
        )

    skill_by_day = dict(
        conn.execute(
            f"""SELECT date(sg.timestamp, 'unixepoch') as day, COALESCE(SUM(sg.ped_value), 0)
            FROM skill_gains sg WHERE {sg_w} GROUP BY day""",
            sg_p,
        ).fetchall()
    )

    cc_w_overview, cc_p_overview = _where("cc.claimed_at", epoch_start, None)
    codex_by_day = dict(
        conn.execute(
            f"""SELECT date(cc.claimed_at, 'unixepoch') as day, COALESCE(SUM(cc.ped_value), 0)
            FROM codex_claims cc WHERE {cc_w_overview} GROUP BY day""",
            cc_p_overview,
        ).fetchall()
    )

    qc_w_overview, qc_p_overview = _where("qc.claimed_at", epoch_start, None)
    quest_by_day = dict(
        conn.execute(
            f"""SELECT date(qc.claimed_at, 'unixepoch') as day, COALESCE(SUM(qc.ped_value), 0)
            FROM quest_claims qc WHERE {qc_w_overview} GROUP BY day""",
            qc_p_overview,
        ).fetchall()
    )

    ledger_gain_day_rows = conn.execute(
        f"""SELECT le.date as day, le.tag, COALESCE(SUM(le.amount), 0)
            FROM ledger_entries le WHERE le.type = 'markup' AND {led_w}
            GROUP BY day, le.tag""",
        led_p,
    ).fetchall()
    ledger_gains_by_day: dict[str, dict[str, float]] = {}
    for day, tag, amount in ledger_gain_day_rows:
        ledger_gains_by_day.setdefault(day, {})[tag] = round(amount, 2)

    ledger_loss_day_rows = conn.execute(
        f"""SELECT le.date as day, le.tag, COALESCE(SUM(le.amount), 0)
            FROM ledger_entries le WHERE le.type = 'expense' AND {led_w}
            GROUP BY day, le.tag""",
        led_p,
    ).fetchall()
    ledger_losses_by_day: dict[str, dict[str, float]] = {}
    for day, tag, amount in ledger_loss_day_rows:
        ledger_losses_by_day.setdefault(day, {})[tag] = round(amount, 2)

    all_days = sorted(
        set(
            list(loot_by_day)
            + list(cost_by_day)
            + list(skill_by_day)
            + list(codex_by_day)
            + list(quest_by_day)
            + list(ledger_gains_by_day)
            + list(ledger_losses_by_day)
        )
    )
    timeline = []
    for day in all_days:
        timeline.append(
            {
                "date": day,
                "lootTt": round(loot_by_day.get(day, 0), 4),
                "pes": round(skill_by_day.get(day, 0), 4),
                "codexPes": round(codex_by_day.get(day, 0), 4),
                "questPes": round(quest_by_day.get(day, 0), 4),
                "ledgerGains": ledger_gains_by_day.get(day, {}),
                "trackingCost": round(cost_by_day.get(day, 0), 4),
                "ledgerLosses": ledger_losses_by_day.get(day, {}),
            }
        )

    # --- Monthly breakdown (per-source) ---
    loot_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', k.timestamp, 'unixepoch') as month, COALESCE(SUM(k.loot_total_ped), 0)
            FROM kills k WHERE {enc_w} GROUP BY month""",
            enc_p,
        ).fetchall()
    )

    weapon_cost_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', k.timestamp, 'unixepoch') as month, COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0)
            FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id
            WHERE {enc_w} GROUP BY month""",
            enc_p,
        ).fetchall()
    )
    enhancer_cost_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', k.timestamp, 'unixepoch') as month,
                   COALESCE(SUM(k.enhancer_cost), 0)
            FROM kills k WHERE {enc_w} GROUP BY month""",
            enc_p,
        ).fetchall()
    )
    sess_cost_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', s.started_at, 'unixepoch') as month,
                   COALESCE(SUM(s.armour_cost), 0) + COALESCE(SUM(s.heal_cost), 0)
                   + COALESCE(SUM(s.dangling_cost), 0)
            FROM tracking_sessions s WHERE {sess_w2} GROUP BY month""",
            sess_p2,
        ).fetchall()
    )
    cost_by_month = {}
    for month in set(
        list(weapon_cost_by_month)
        + list(enhancer_cost_by_month)
        + list(sess_cost_by_month)
    ):
        cost_by_month[month] = (
            weapon_cost_by_month.get(month, 0)
            + enhancer_cost_by_month.get(month, 0)
            + sess_cost_by_month.get(month, 0)
        )

    skill_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', sg.timestamp, 'unixepoch') as month, COALESCE(SUM(sg.ped_value), 0)
            FROM skill_gains sg WHERE {sg_w} GROUP BY month""",
            sg_p,
        ).fetchall()
    )

    codex_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', cc.claimed_at, 'unixepoch') as month, COALESCE(SUM(cc.ped_value), 0)
            FROM codex_claims cc WHERE {cc_w_overview} GROUP BY month""",
            cc_p_overview,
        ).fetchall()
    )

    quest_by_month = dict(
        conn.execute(
            f"""SELECT strftime('%Y-%m', qc.claimed_at, 'unixepoch') as month, COALESCE(SUM(qc.ped_value), 0)
            FROM quest_claims qc WHERE {qc_w_overview} GROUP BY month""",
            qc_p_overview,
        ).fetchall()
    )

    ledger_gain_month_rows = conn.execute(
        f"""SELECT strftime('%Y-%m', le.date) as month, le.tag, COALESCE(SUM(le.amount), 0)
            FROM ledger_entries le WHERE le.type = 'markup' AND {led_w}
            GROUP BY month, le.tag""",
        led_p,
    ).fetchall()
    ledger_gains_by_month: dict[str, dict[str, float]] = {}
    for month, tag, amount in ledger_gain_month_rows:
        ledger_gains_by_month.setdefault(month, {})[tag] = round(amount, 2)

    ledger_loss_month_rows = conn.execute(
        f"""SELECT strftime('%Y-%m', le.date) as month, le.tag, COALESCE(SUM(le.amount), 0)
            FROM ledger_entries le WHERE le.type = 'expense' AND {led_w}
            GROUP BY month, le.tag""",
        led_p,
    ).fetchall()
    ledger_losses_by_month: dict[str, dict[str, float]] = {}
    for month, tag, amount in ledger_loss_month_rows:
        ledger_losses_by_month.setdefault(month, {})[tag] = round(amount, 2)

    all_months = sorted(
        set(
            list(loot_by_month)
            + list(cost_by_month)
            + list(skill_by_month)
            + list(codex_by_month)
            + list(quest_by_month)
            + list(ledger_gains_by_month)
            + list(ledger_losses_by_month)
        )
    )
    monthly = []
    for month in all_months:
        monthly.append(
            {
                "month": month,
                "lootTt": round(loot_by_month.get(month, 0), 4),
                "pes": round(skill_by_month.get(month, 0), 4),
                "codexPes": round(codex_by_month.get(month, 0), 4),
                "questPes": round(quest_by_month.get(month, 0), 4),
                "ledgerGains": ledger_gains_by_month.get(month, {}),
                "trackingCost": round(cost_by_month.get(month, 0), 4),
                "ledgerLosses": ledger_losses_by_month.get(month, {}),
            }
        )

    return {
        "totalReturnRate": round(return_rate, 4),
        "trend": trend,
        "returnsBreakdown": {
            "lootTt": round(m["loot_tt"], 2),
            "pes": round(m["skill_tt"], 2),
            "codexPes": round(m["codex_pes"], 2),
            "questPes": round(m["quest_pes"], 2),
            "ledger": m["ledger_gains"],
        },
        "lossesBreakdown": {
            "trackingCost": round(m["tracking_cost"], 2),
            "cycledBreakdown": m["cycled_breakdown"],
            "ledger": m["ledger_losses"],
        },
        "totalGains": round(total_gains, 2),
        "totalLosses": round(total_losses, 2),
        "timeline": timeline,
        "monthlyBreakdown": monthly,
    }


# ------------------------------------------------------------------
# Activity
# ------------------------------------------------------------------


def _load_activity_sessions(conn) -> list[dict]:
    """Return completed session summaries for activity composition tables."""
    sessions: dict[str, dict] = {}

    session_rows = conn.execute(
        """
        SELECT id,
               started_at,
               ended_at,
               COALESCE(armour_cost, 0),
               COALESCE(heal_cost, 0),
               COALESCE(dangling_cost, 0)
        FROM tracking_sessions
        WHERE ended_at IS NOT NULL
        """,
    ).fetchall()

    for row in session_rows:
        session_id, started_at, ended_at, armour_cost, heal_cost, dangling_cost = row
        duration_seconds = max(float(ended_at or 0) - float(started_at or 0), 0.0)
        sessions[session_id] = {
            "id": session_id,
            "durationHours": duration_seconds / 3600.0,
            "armourCost": float(armour_cost or 0),
            "healCost": float(heal_cost or 0),
            "danglingCost": float(dangling_cost or 0),
            "weaponCost": 0.0,
            "enhancerCost": 0.0,
            "weaponShots": 0.0,
            "kills": 0,
            "lootTt": 0.0,
            "skillTt": 0.0,
            "dominantMob": None,
            "dominantMobKills": 0,
            "dominantTag": None,
            "dominantTagKills": 0,
            "dominantWeapon": None,
        }

    if not sessions:
        return []

    kill_rows = conn.execute(
        """
        SELECT session_id,
               COUNT(*),
               COALESCE(SUM(loot_total_ped), 0),
               COALESCE(SUM(enhancer_cost), 0)
        FROM kills
        GROUP BY session_id
        """,
    ).fetchall()
    for row in kill_rows:
        session = sessions.get(row[0])
        if not session:
            continue
        session["kills"] = int(row[1] or 0)
        session["lootTt"] = float(row[2] or 0)
        session["enhancerCost"] = float(row[3] or 0)

    weapon_rows = conn.execute(
        """
        SELECT k.session_id,
               COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0),
               COALESCE(SUM(ts.shots_fired), 0)
        FROM kill_tool_stats ts
        JOIN kills k ON k.id = ts.kill_id
        GROUP BY k.session_id
        """,
    ).fetchall()
    for row in weapon_rows:
        session = sessions.get(row[0])
        if session:
            session["weaponCost"] = float(row[1] or 0)
            session["weaponShots"] = float(row[2] or 0)

    skill_rows = conn.execute(
        """
        SELECT session_id,
               COALESCE(SUM(ped_value), 0)
        FROM skill_gains
        WHERE ped_value IS NOT NULL
        GROUP BY session_id
        """,
    ).fetchall()
    for row in skill_rows:
        session = sessions.get(row[0])
        if session:
            session["skillTt"] = float(row[1] or 0)

    group_rows = conn.execute(
        """
        SELECT session_id,
               mob_name,
               COALESCE(mob_species, ''),
               COALESCE(mob_maturity, ''),
               COUNT(*)
        FROM kills
        WHERE mob_name IS NOT NULL
          AND mob_name != 'Unknown'
        GROUP BY session_id, mob_name, mob_species, mob_maturity
        ORDER BY session_id, COUNT(*) DESC, mob_name ASC
        """,
    ).fetchall()

    groups_by_session: dict[str, list[dict]] = defaultdict(list)
    for row in group_rows:
        groups_by_session[row[0]].append(
            {
                "name": row[1],
                "species": row[2],
                "maturity": row[3],
                "kills": int(row[4] or 0),
            }
        )

    for session_id, groups in groups_by_session.items():
        session = sessions.get(session_id)
        if not session:
            continue
        total_known_kills = sum(group["kills"] for group in groups)
        if total_known_kills <= 0:
            continue
        top = groups[0]
        if (top["kills"] / total_known_kills) < ACTIVITY_DOMINANCE_THRESHOLD:
            continue
        if top["species"] or top["maturity"]:
            session["dominantMob"] = top["name"]
            session["dominantMobKills"] = top["kills"]
        else:
            session["dominantTag"] = top["name"]
            session["dominantTagKills"] = top["kills"]

    weapon_groups = conn.execute(
        """
        SELECT k.session_id,
               ts.tool_name,
               COALESCE(SUM(ts.shots_fired), 0) as total_shots
        FROM kill_tool_stats ts
        JOIN kills k ON k.id = ts.kill_id
        WHERE ts.tool_name IS NOT NULL
          AND ts.tool_name != 'Unknown'
        GROUP BY k.session_id, ts.tool_name
        ORDER BY k.session_id, total_shots DESC, ts.tool_name ASC
        """,
    ).fetchall()
    weapons_by_session: dict[str, list[dict]] = defaultdict(list)
    for row in weapon_groups:
        weapons_by_session[row[0]].append(
            {
                "name": row[1],
                "shots": float(row[2] or 0),
            }
        )

    for session_id, groups in weapons_by_session.items():
        session = sessions.get(session_id)
        if not session:
            continue
        total_shots = sum(group["shots"] for group in groups)
        if total_shots <= 0:
            continue
        top = groups[0]
        if (top["shots"] / total_shots) >= ACTIVITY_DOMINANCE_THRESHOLD:
            session["dominantWeapon"] = top["name"]

    result = []
    for session in sessions.values():
        session["cycledPed"] = round(
            session["weaponCost"]
            + session["enhancerCost"]
            + session["armourCost"]
            + session["healCost"]
            + session["danglingCost"],
            4,
        )
        if session["durationHours"] <= 0:
            continue
        if session["cycledPed"] <= 0:
            continue
        if session["kills"] <= 0:
            continue
        result.append(session)

    return result


def _build_activity_slice_rows(
    sessions: list[dict],
    *,
    key: str,
    kills_key: str,
    name_field: str,
) -> list[dict]:
    grouped: dict[str, list[dict]] = defaultdict(list)
    for session in sessions:
        value = session.get(key)
        if value:
            grouped[value].append(session)

    rows: list[dict[str, Any]] = []
    for value, matched_sessions in grouped.items():
        sessions_count = len(matched_sessions)
        kills = sum(int(session.get(kills_key) or 0) for session in matched_sessions)
        hours = sum(float(session["durationHours"]) for session in matched_sessions)
        cycled = sum(float(session["cycledPed"]) for session in matched_sessions)
        loot_tt = sum(float(session["lootTt"]) for session in matched_sessions)
        skill_tt = sum(float(session["skillTt"]) for session in matched_sessions)
        rows.append(
            {
                name_field: value,
                "sessions": sessions_count,
                "kills": kills,
                "hours": round(hours, 2),
                "cycled": round(cycled, 2),
                "pesPer100Ped": round((skill_tt / cycled) * 100, 2)
                if cycled > 0
                else 0.0,
                "lootRate": round(loot_tt / cycled, 4) if cycled > 0 else 0.0,
            }
        )

    rows.sort(key=lambda row: (-row["kills"], -row["cycled"], row[name_field]))
    return rows


@router.get("/activity")
def analytics_activity():
    """Per-mob, per-tag, and per-weapon activity comparisons."""
    return activity_impl(get_services().app_db.conn)


def activity_impl(conn):
    activity_sessions = _load_activity_sessions(conn)

    mob_comparisons = _build_activity_slice_rows(
        activity_sessions,
        key="dominantMob",
        kills_key="dominantMobKills",
        name_field="mobName",
    )

    tag_comparisons = _build_activity_slice_rows(
        activity_sessions,
        key="dominantTag",
        kills_key="dominantTagKills",
        name_field="tagName",
    )

    weapon_groups: dict[str, list[dict]] = defaultdict(list)
    for session in activity_sessions:
        dominant_weapon = session.get("dominantWeapon")
        if dominant_weapon:
            weapon_groups[dominant_weapon].append(session)

    weapon_comparisons: list[dict[str, Any]] = []
    for weapon_name, matched_sessions in weapon_groups.items():
        sessions_count = len(matched_sessions)
        kills = sum(int(session["kills"]) for session in matched_sessions)
        hours = sum(float(session["durationHours"]) for session in matched_sessions)
        cycled = sum(float(session["cycledPed"]) for session in matched_sessions)
        loot_tt = sum(float(session["lootTt"]) for session in matched_sessions)
        skill_tt = sum(float(session["skillTt"]) for session in matched_sessions)

        weapon_comparisons.append(
            {
                "weaponName": weapon_name,
                "sessions": sessions_count,
                "kills": kills,
                "hours": round(hours, 2),
                "cycled": round(cycled, 2),
                "pesPer100Ped": round((skill_tt / cycled) * 100, 2)
                if cycled > 0
                else 0.0,
                "lootRate": round(loot_tt / cycled, 4) if cycled > 0 else 0.0,
            }
        )

    weapon_comparisons.sort(
        key=lambda row: (-row["kills"], -row["cycled"], row["weaponName"])
    )

    return {
        "mobComparisons": mob_comparisons,
        "tagComparisons": tag_comparisons,
        "weaponComparisons": weapon_comparisons,
    }


# ------------------------------------------------------------------
# Ledger
# ------------------------------------------------------------------


class LedgerEntryCreate(BaseModel):
    date: str
    type: str  # 'expense' | 'markup'
    description: str
    amount: float
    tag: str


@router.get("/ledger")
def list_ledger():
    """List all ledger entries.

    The activity ledger is liquid economy only. Codex rewards live in
    `codex_claims` and skill-typed quest rewards live in `quest_claims`.
    New writes never produce PES-tagged ledger rows, so no read-side
    filter is needed.
    """
    return list_ledger_impl(get_services().app_db.conn)


def list_ledger_impl(conn):
    rows = conn.execute(
        """SELECT id, date, type, description, amount, tag
           FROM ledger_entries
           ORDER BY date DESC, id DESC""",
    ).fetchall()
    return [
        {
            "id": r[0],
            "date": r[1],
            "type": r[2],
            "description": r[3],
            "amount": r[4],
            "tag": r[5],
        }
        for r in rows
    ]


@router.post("/ledger")
def create_ledger_entry(entry: LedgerEntryCreate):
    """Create a new ledger entry."""
    svc = get_services()
    entry_id = str(uuid.uuid4())
    svc.app_db.conn.execute(
        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) VALUES (?, ?, ?, ?, ?, ?)",
        (entry_id, entry.date, entry.type, entry.description, entry.amount, entry.tag),
    )
    svc.app_db.conn.commit()
    return {
        "id": entry_id,
        "date": entry.date,
        "type": entry.type,
        "description": entry.description,
        "amount": entry.amount,
        "tag": entry.tag,
    }


@router.delete("/ledger/{entry_id}")
def delete_ledger_entry(entry_id: str):
    """Delete a ledger entry."""
    svc = get_services()
    result = svc.app_db.conn.execute(
        "DELETE FROM ledger_entries WHERE id = ?",
        (entry_id,),
    )
    svc.app_db.conn.commit()
    if result.rowcount == 0:
        raise HTTPException(status_code=404, detail="Entry not found")
    return {"status": "deleted"}


# ------------------------------------------------------------------
# Ledger Quick-Entry Presets
# ------------------------------------------------------------------


class LedgerPresetCreate(BaseModel):
    name: str
    type: str  # 'expense' | 'markup'
    description: str
    amount: float
    tag: str


def _preset_row_to_dict(row) -> dict:
    return {
        "id": row[0],
        "name": row[1],
        "type": row[2],
        "description": row[3],
        "amount": row[4],
        "tag": row[5],
    }


@router.get("/ledger/presets")
def list_ledger_presets():
    """List all ledger quick-entry presets."""
    return list_ledger_presets_impl(get_services().app_db.conn)


def list_ledger_presets_impl(conn):
    rows = conn.execute(
        "SELECT id, name, type, description, amount, tag FROM ledger_presets "
        "ORDER BY created_at ASC, id ASC"
    ).fetchall()
    return [_preset_row_to_dict(r) for r in rows]


@router.post("/ledger/presets")
def create_ledger_preset(preset: LedgerPresetCreate):
    """Create a new ledger quick-entry preset."""
    if preset.type not in ("expense", "markup"):
        raise HTTPException(
            status_code=400, detail="type must be 'expense' or 'markup'"
        )
    svc = get_services()
    preset_id = str(uuid.uuid4())
    svc.app_db.conn.execute(
        "INSERT INTO ledger_presets (id, name, type, description, amount, tag) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (
            preset_id,
            preset.name,
            preset.type,
            preset.description,
            preset.amount,
            preset.tag,
        ),
    )
    svc.app_db.conn.commit()
    return {
        "id": preset_id,
        "name": preset.name,
        "type": preset.type,
        "description": preset.description,
        "amount": preset.amount,
        "tag": preset.tag,
    }


@router.delete("/ledger/presets/{preset_id}")
def delete_ledger_preset(preset_id: str):
    """Delete a ledger quick-entry preset."""
    svc = get_services()
    result = svc.app_db.conn.execute(
        "DELETE FROM ledger_presets WHERE id = ?",
        (preset_id,),
    )
    svc.app_db.conn.commit()
    if result.rowcount == 0:
        raise HTTPException(status_code=404, detail="Preset not found")
    return {"status": "deleted"}


# ------------------------------------------------------------------
# Inventory Ledger
# ------------------------------------------------------------------
# UL/persistent items (weapons, estates, deeds, UL blueprints, speculative
# loot buys) whose value falls outside current cost-per-shot / loot tracking.
# Purchases and sales do NOT touch ledger_entries directly; only the realised
# gain/loss delta on sale is emitted with tag=inventory_sale.


class InventoryItemCreate(BaseModel):
    name: str
    tt_value: float
    markup_paid: float
    notes: str | None = None
    acquired_at: str | None = None


class InventoryItemPatch(BaseModel):
    name: str | None = None
    tt_value: float | None = None
    markup_paid: float | None = None
    notes: str | None = None


class InventoryItemSell(BaseModel):
    sale_price: float
    description: str | None = None
    sold_at: str | None = None


def _inventory_row_to_dict(row) -> dict:
    return {
        "id": row["id"],
        "name": row["name"],
        "ttValue": row["tt_value"],
        "markupPaid": row["markup_paid"],
        "notes": row["notes"],
        "acquiredAt": row["acquired_at"],
    }


@router.get("/inventory")
def list_inventory_items():
    """List all inventory items, newest first."""
    return list_inventory_items_impl(get_services().app_db.conn)


def list_inventory_items_impl(conn):
    rows = conn.execute(
        "SELECT id, name, tt_value, markup_paid, notes, acquired_at "
        "FROM inventory_items ORDER BY acquired_at DESC, id DESC"
    ).fetchall()
    return [_inventory_row_to_dict(r) for r in rows]


@router.post("/inventory")
def create_inventory_item(item: InventoryItemCreate):
    """Create a new inventory item."""
    svc = get_services()
    item_id = str(uuid.uuid4())
    acquired_at = item.acquired_at or datetime.now(UTC).strftime("%Y-%m-%d")
    svc.app_db.conn.execute(
        "INSERT INTO inventory_items (id, name, tt_value, markup_paid, notes, acquired_at) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (item_id, item.name, item.tt_value, item.markup_paid, item.notes, acquired_at),
    )
    svc.app_db.conn.commit()
    row = svc.app_db.conn.execute(
        "SELECT id, name, tt_value, markup_paid, notes, acquired_at "
        "FROM inventory_items WHERE id = ?",
        (item_id,),
    ).fetchone()
    return _inventory_row_to_dict(row)


@router.patch("/inventory/{item_id}")
def update_inventory_item(item_id: str, patch: InventoryItemPatch):
    """Edit fields on an inventory item. Bumps updated_at."""
    svc = get_services()
    existing = svc.app_db.conn.execute(
        "SELECT id FROM inventory_items WHERE id = ?",
        (item_id,),
    ).fetchone()
    if not existing:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    fields: list[str] = []
    params: list = []
    if patch.name is not None:
        fields.append("name = ?")
        params.append(patch.name)
    if patch.tt_value is not None:
        fields.append("tt_value = ?")
        params.append(patch.tt_value)
    if patch.markup_paid is not None:
        fields.append("markup_paid = ?")
        params.append(patch.markup_paid)
    if patch.notes is not None:
        fields.append("notes = ?")
        params.append(patch.notes)

    if fields:
        fields.append("updated_at = unixepoch('now')")
        params.append(item_id)
        svc.app_db.conn.execute(
            f"UPDATE inventory_items SET {', '.join(fields)} WHERE id = ?",
            tuple(params),
        )
        svc.app_db.conn.commit()

    row = svc.app_db.conn.execute(
        "SELECT id, name, tt_value, markup_paid, notes, acquired_at "
        "FROM inventory_items WHERE id = ?",
        (item_id,),
    ).fetchone()
    return _inventory_row_to_dict(row)


@router.delete("/inventory/{item_id}")
def delete_inventory_item(item_id: str):
    """Hard delete an inventory item (correction path, no ledger entry emitted)."""
    svc = get_services()
    result = svc.app_db.conn.execute(
        "DELETE FROM inventory_items WHERE id = ?",
        (item_id,),
    )
    svc.app_db.conn.commit()
    if result.rowcount == 0:
        raise HTTPException(status_code=404, detail="Inventory item not found")
    return {"status": "deleted"}


@router.post("/inventory/{item_id}/sell")
def sell_inventory_item(item_id: str, payload: InventoryItemSell):
    """Sell an inventory item: emit realised delta to ledger + remove row, atomically.

    Zero-delta sales skip ledger emission (no noise row); the item is still
    removed from the inventory ledger and ledgerEntry is returned as null.
    """
    svc = get_services()
    conn = svc.app_db.conn
    row = conn.execute(
        "SELECT id, name, tt_value, markup_paid, notes, acquired_at "
        "FROM inventory_items WHERE id = ?",
        (item_id,),
    ).fetchone()
    if not row:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    cost_basis = row["tt_value"] + row["markup_paid"]
    delta = payload.sale_price - cost_basis
    sold_at = payload.sold_at or datetime.now(UTC).strftime("%Y-%m-%d")
    sold_item = _inventory_row_to_dict(row)

    ledger_entry: dict | None = None
    with conn:
        if delta != 0:
            entry_id = str(uuid.uuid4())
            entry_type = "markup" if delta > 0 else "expense"
            amount = abs(delta)
            description = payload.description or f"Inventory Sale: {row['name']}"
            conn.execute(
                "INSERT INTO ledger_entries (id, date, type, description, amount, tag) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (
                    entry_id,
                    sold_at,
                    entry_type,
                    description,
                    amount,
                    INVENTORY_SALE_TAG,
                ),
            )
            ledger_entry = {
                "id": entry_id,
                "date": sold_at,
                "type": entry_type,
                "description": description,
                "amount": amount,
                "tag": INVENTORY_SALE_TAG,
            }
        conn.execute("DELETE FROM inventory_items WHERE id = ?", (item_id,))

    return {"ledgerEntry": ledger_entry, "soldItem": sold_item}
