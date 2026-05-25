"""Materialised per-session summaries for Character → Prospect.

Treated as a cache of derived state. Source of truth is the tracking tables
(tracking_sessions, kills, kill_tool_stats, skill_gains). Summaries are
written eagerly when a session ends and lazily rebuilt on read when a row
is missing — e.g. sessions that ended before this feature existed, or
imported historical sessions.

Bump SUMMARY_VERSION when the summary shape or dominance logic changes;
load_prospect_sessions will then transparently recompute stale rows.
"""

from __future__ import annotations

import json
import sqlite3

from backend.services.character_calc import ATTRIBUTE_SKILLS

SUMMARY_VERSION = 1
DOMINANCE_THRESHOLD = 0.6


def compute_session_summary(conn: sqlite3.Connection, session_id: str) -> dict | None:
    """Build the prospect summary for one completed session.

    Returns None when the session is active, has no skill gains, or fails
    the qualifying filters (zero cycled PED, zero duration, no skill gains
    at all). Callers should treat None as "do not include in Prospect".
    """
    sess = conn.execute(
        "SELECT started_at, ended_at, "
        "COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) "
        "FROM tracking_sessions WHERE id = ? AND ended_at IS NOT NULL",
        (session_id,),
    ).fetchone()
    if not sess:
        return None
    started_at, ended_at, armour_cost, heal_cost, dangling_cost = sess

    # Tolerate skill_gains being absent — early bring-up creates tracking
    # tables before app_database has run its schema. Treat absence as
    # "session has no gains to summarise".
    try:
        has_gains = conn.execute(
            "SELECT 1 FROM skill_gains WHERE session_id = ? LIMIT 1",
            (session_id,),
        ).fetchone()
    except sqlite3.OperationalError:
        return None
    if not has_gains:
        return None

    kill_totals = conn.execute(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) "
        "FROM kills WHERE session_id = ?",
        (session_id,),
    ).fetchone()
    kills = int(kill_totals[0] or 0)
    loot_tt = float(kill_totals[1] or 0.0)
    enhancer_cost = float(kill_totals[2] or 0.0)

    weapon_row = conn.execute(
        "SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) "
        "FROM kill_tool_stats ts "
        "JOIN kills k ON k.id = ts.kill_id "
        "WHERE k.session_id = ?",
        (session_id,),
    ).fetchone()
    weapon_cost = float(weapon_row[0] or 0.0)

    mob_rows = conn.execute(
        "SELECT mob_name, COALESCE(mob_species, ''), COALESCE(mob_maturity, ''), COUNT(*) "
        "FROM kills "
        "WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown' "
        "GROUP BY mob_name, mob_species, mob_maturity "
        "ORDER BY COUNT(*) DESC, mob_name ASC",
        (session_id,),
    ).fetchall()
    dominant_mob: str | None = None
    dominant_tag: str | None = None
    if mob_rows:
        total_known = sum(int(r[3] or 0) for r in mob_rows)
        if total_known > 0:
            top_name, top_species, top_maturity, top_count = mob_rows[0]
            if int(top_count or 0) / total_known >= DOMINANCE_THRESHOLD:
                if top_species or top_maturity:
                    dominant_mob = top_name
                else:
                    dominant_tag = top_name

    tool_rows = conn.execute(
        "SELECT ts.tool_name, COALESCE(SUM(ts.shots_fired), 0) "
        "FROM kill_tool_stats ts "
        "JOIN kills k ON k.id = ts.kill_id "
        "WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown' "
        "GROUP BY ts.tool_name "
        "ORDER BY SUM(ts.shots_fired) DESC, ts.tool_name ASC",
        (session_id,),
    ).fetchall()
    dominant_weapon: str | None = None
    if tool_rows:
        total_shots = sum(float(r[1] or 0) for r in tool_rows)
        top_name, top_shots = tool_rows[0]
        if (
            total_shots > 0
            and float(top_shots or 0) / total_shots >= DOMINANCE_THRESHOLD
        ):
            dominant_weapon = top_name

    regular_rows = conn.execute(
        "SELECT skill_name, COALESCE(SUM(ped_value), 0) "
        "FROM skill_gains "
        "WHERE session_id = ? AND ped_value IS NOT NULL "
        "GROUP BY skill_name",
        (session_id,),
    ).fetchall()
    regular_skill_ped = {
        name: float(total or 0.0)
        for name, total in regular_rows
        if float(total or 0.0) > 0
    }

    attr_placeholders = ",".join("?" * len(ATTRIBUTE_SKILLS))
    attr_rows = conn.execute(
        f"SELECT skill_name, COALESCE(SUM(amount), 0) "
        f"FROM skill_gains "
        f"WHERE session_id = ? AND skill_name IN ({attr_placeholders}) "
        f"GROUP BY skill_name",
        (session_id, *ATTRIBUTE_SKILLS),
    ).fetchall()
    attribute_levels = {
        name: float(total or 0.0)
        for name, total in attr_rows
        if float(total or 0.0) > 0
    }

    duration_hours = max((float(ended_at) - float(started_at)) / 3600.0, 0.0)
    armour_cost = float(armour_cost or 0.0)
    heal_cost = float(heal_cost or 0.0)
    dangling_cost = float(dangling_cost or 0.0)
    cycled_ped = weapon_cost + enhancer_cost + armour_cost + heal_cost + dangling_cost
    regular_skill_tt = sum(regular_skill_ped.values())
    attribute_levels_total = sum(attribute_levels.values())

    if cycled_ped <= 0 or duration_hours <= 0:
        return None
    if regular_skill_tt <= 0 and attribute_levels_total <= 0:
        return None

    return {
        "id": session_id,
        "startedAt": float(started_at),
        "endedAt": float(ended_at),
        "durationHours": duration_hours,
        "armourCost": armour_cost,
        "healCost": heal_cost,
        "danglingCost": dangling_cost,
        "weaponCost": weapon_cost,
        "enhancerCost": enhancer_cost,
        "kills": kills,
        "lootTt": loot_tt,
        "regularSkillPed": regular_skill_ped,
        "attributeLevels": attribute_levels,
        "dominantMob": dominant_mob,
        "dominantTag": dominant_tag,
        "dominantWeapon": dominant_weapon,
        "regularSkillTt": round(regular_skill_tt, 4),
        "attributeLevelsTotal": round(attribute_levels_total, 4),
        "cycledPed": round(cycled_ped, 4),
    }


def write_session_summary(conn: sqlite3.Connection, session_id: str) -> None:
    """Compute and upsert the summary row for one session.

    No-op (and clears any stale row) when the session doesn't qualify.
    Caller owns the surrounding transaction / commit.
    """
    summary = compute_session_summary(conn, session_id)
    if summary is None:
        conn.execute(
            "DELETE FROM session_summaries WHERE session_id = ?",
            (session_id,),
        )
        return
    conn.execute(
        "INSERT OR REPLACE INTO session_summaries ("
        "session_id, summary_version, started_at, ended_at, duration_hours, "
        "kills, loot_tt, weapon_cost, enhancer_cost, armour_cost, heal_cost, "
        "dangling_cost, cycled_ped, regular_skill_ped_json, attribute_levels_json, "
        "regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, "
        "dominant_weapon, computed_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch('now'))",
        (
            summary["id"],
            SUMMARY_VERSION,
            summary["startedAt"],
            summary["endedAt"],
            summary["durationHours"],
            summary["kills"],
            summary["lootTt"],
            summary["weaponCost"],
            summary["enhancerCost"],
            summary["armourCost"],
            summary["healCost"],
            summary["danglingCost"],
            summary["cycledPed"],
            json.dumps(summary["regularSkillPed"]),
            json.dumps(summary["attributeLevels"]),
            summary["regularSkillTt"],
            summary["attributeLevelsTotal"],
            summary["dominantMob"],
            summary["dominantTag"],
            summary["dominantWeapon"],
        ),
    )


def delete_session_summary(conn: sqlite3.Connection, session_id: str) -> None:
    """Remove a session's summary row. Idempotent. Caller owns commit."""
    conn.execute(
        "DELETE FROM session_summaries WHERE session_id = ?",
        (session_id,),
    )


def _row_to_prospect_dict(row: sqlite3.Row | tuple) -> dict:
    # Column order matches the SELECT in load_prospect_sessions.
    (
        session_id,
        started_at,
        ended_at,
        duration_hours,
        kills,
        loot_tt,
        weapon_cost,
        enhancer_cost,
        armour_cost,
        heal_cost,
        dangling_cost,
        cycled_ped,
        regular_json,
        attr_json,
        regular_skill_tt,
        attribute_levels_total,
        dominant_mob,
        dominant_tag,
        dominant_weapon,
    ) = row
    return {
        "id": session_id,
        "startedAt": float(started_at or 0.0),
        "endedAt": float(ended_at or 0.0),
        "durationHours": float(duration_hours or 0.0),
        "kills": int(kills or 0),
        "lootTt": float(loot_tt or 0.0),
        "weaponCost": float(weapon_cost or 0.0),
        "enhancerCost": float(enhancer_cost or 0.0),
        "armourCost": float(armour_cost or 0.0),
        "healCost": float(heal_cost or 0.0),
        "danglingCost": float(dangling_cost or 0.0),
        "cycledPed": float(cycled_ped or 0.0),
        "regularSkillPed": json.loads(regular_json) if regular_json else {},
        "attributeLevels": json.loads(attr_json) if attr_json else {},
        "regularSkillTt": float(regular_skill_tt or 0.0),
        "attributeLevelsTotal": float(attribute_levels_total or 0.0),
        "dominantMob": dominant_mob,
        "dominantTag": dominant_tag,
        "dominantWeapon": dominant_weapon,
    }


def load_prospect_sessions(conn: sqlite3.Connection) -> list[dict]:
    """Return all qualifying completed-session summaries.

    Lazily rebuilds any missing or stale-version rows, so new installs
    converge on first read without needing a migration.
    """
    missing = conn.execute(
        "SELECT s.id FROM tracking_sessions s "
        "LEFT JOIN session_summaries ss ON ss.session_id = s.id "
        "WHERE s.ended_at IS NOT NULL "
        "AND EXISTS (SELECT 1 FROM skill_gains sg WHERE sg.session_id = s.id) "
        "AND (ss.session_id IS NULL OR ss.summary_version < ?)",
        (SUMMARY_VERSION,),
    ).fetchall()
    if missing:
        for (sid,) in missing:
            write_session_summary(conn, sid)
        conn.commit()

    rows = conn.execute(
        "SELECT session_id, started_at, ended_at, duration_hours, kills, loot_tt, "
        "weapon_cost, enhancer_cost, armour_cost, heal_cost, dangling_cost, "
        "cycled_ped, regular_skill_ped_json, attribute_levels_json, "
        "regular_skill_tt, attribute_levels_total, dominant_mob, dominant_tag, "
        "dominant_weapon "
        "FROM session_summaries"
    ).fetchall()
    return [_row_to_prospect_dict(r) for r in rows]
