"""Tracking session management endpoints.

Returns shapes matching the frontend TrackingSession and SessionDetail types.
"""

import logging
import time
from datetime import datetime, timezone

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from backend.dependencies import get_services
from backend.services.character_calc import ATTRIBUTE_SKILLS
from backend.services.config_service import active_trifecta_preset
from backend.services.trifecta_service import validate_trifecta


def _validate_hotbar(config) -> tuple[bool, str | None]:
    """Hotbar-mode attribution is workable as long as at least one slot is bound."""
    if any(library_id is not None for library_id in config.hotbar.values()):
        return True, None
    return False, "Bind at least one hotbar slot in the Equipment page before tracking."


def _validate_attribution(config, conn) -> tuple[bool, str | None]:
    """Gate at session-start: the active attribution mechanism must be usable.

    Hotbar mode → needs at least one bound slot. Trifecta mode → delegates to
    validate_trifecta. Both failure paths point the user at the Equipment page.
    """
    if config.hotbar_hooks_enabled:
        return _validate_hotbar(config)
    ready, message = validate_trifecta(conn, active_trifecta_preset(config))
    if ready:
        return True, None
    return False, message or "Configure the trifecta in the Equipment page before tracking."

log = logging.getLogger(__name__)

router = APIRouter(prefix="/tracking", tags=["tracking"])

class ManualMobLockRequest(BaseModel):
    species: str
    maturity: str = ""

class TagLockRequest(BaseModel):
    tag: str

def _weapon_attribution(config) -> str:
    return "trifecta" if not config.hotbar_hooks_enabled else "hotbar"

def _is_tag_mode(config, mob_tracking_mode: str | None = None) -> bool:
    return (mob_tracking_mode or config.mob_tracking_mode) == "tag"

def _configured_manual_label(config) -> tuple[str | None, str | None]:
    if _is_tag_mode(config):
        tag = config.mob_tracking_tag.strip()
        if tag:
            return tag, "tag"
        return None, None

    species = getattr(config, "manual_mob_species", "").strip()
    maturity = getattr(config, "manual_mob_maturity", "").strip()
    if not species:
        return None, None

    display = f"{maturity} {species}" if maturity else species
    return display, "manual"

def _ts_to_iso(ts: float | None) -> str | None:
    """Convert a Unix timestamp (SQLite REAL) to an ISO 8601 string."""
    if ts is None:
        return None
    return datetime.fromtimestamp(ts, tz=timezone.utc).isoformat()

def _notable_event_category(event_type: str) -> str:
    if event_type.startswith("quest_"):
        return "quest"
    if event_type.startswith("hof_"):
        return "hof"
    return "global"

def _notable_event_label(event_type: str) -> str:
    labels = {
        "global_kill": "Global Kill",
        "global_item": "Global Item",
        "hof_kill": "HoF Kill",
        "hof_item": "HoF Item",
        "quest_started": "Quest Started",
        "quest_completed": "Quest Completed",
    }
    if event_type in labels:
        return labels[event_type]

    category = _notable_event_category(event_type)
    if category == "hof":
        return "HoF"
    return category.capitalize()

def _notable_event_description(event_type: str, mob_or_item: str, value_ped: float) -> str:
    label = _notable_event_label(event_type)
    if event_type.startswith("quest_"):
        return f"{label}: {mob_or_item}"
    return f"{label} — {mob_or_item}: {value_ped:.2f} PED"

def _notable_event_payload(
    event_type: str,
    mob_or_item: str,
    value_ped: float,
    timestamp: float | str | None = None,
) -> dict:
    payload = {
        "type": _notable_event_category(event_type),
        "eventType": event_type,
        "description": _notable_event_description(event_type, mob_or_item, value_ped),
        "value": value_ped,
    }
    if isinstance(timestamp, (int, float)):
        payload["timestamp"] = _ts_to_iso(float(timestamp))
    elif timestamp:
        payload["timestamp"] = timestamp
    return payload

def _trifecta_attribution_summary(svc) -> dict | None:
    config = svc.config_service.get()
    active = active_trifecta_preset(config)
    ids = {
        "smallWeapon": active.small_weapon_id if active else None,
        "bigWeapon": active.big_weapon_id if active else None,
        "healTool": active.heal_id if active else None,
    }
    presets = [{"id": preset.id, "name": preset.name} for preset in config.trifecta_presets]

    if not presets and all(value is None for value in ids.values()):
        return None

    summary = {
        "activePresetId": config.active_trifecta_preset_id,
        "presetName": active.name if active else None,
        "presets": presets,
        "smallWeapon": None,
        "bigWeapon": None,
        "healTool": None,
    }
    for key, item_type in (
        ("smallWeapon", "weapon"),
        ("bigWeapon", "weapon"),
        ("healTool", "healing"),
    ):
        item_id = ids[key]
        if item_id is None:
            continue
        row = svc.app_db.conn.execute(
            "SELECT name FROM equipment_library WHERE id = ? AND item_type = ?",
            (item_id, item_type),
        ).fetchone()
        if row is not None:
            summary[key] = row[0]
    return summary

@router.post("/start")
def start_tracking():
    """Start a new tracking session."""
    svc = get_services()
    if not hasattr(svc, "tracker") or svc.tracker is None:
        raise HTTPException(status_code=503, detail="Tracker not initialised")

    if svc.tracker.is_tracking:
        raise HTTPException(status_code=409, detail="Session already active")

    ready, message = _validate_attribution(svc.config_service.get(), svc.app_db.conn)
    if not ready:
        raise HTTPException(status_code=400, detail=message)

    session = svc.tracker.start_session()
    return {
        "session_id": session.id,
        "started_at": session.start_time.isoformat(),
        "status": "active",
    }

@router.post("/stop")
def stop_tracking():
    """Stop the active tracking session."""
    svc = get_services()
    if not hasattr(svc, "tracker") or svc.tracker is None:
        raise HTTPException(status_code=503, detail="Tracker not initialised")

    if not svc.tracker.is_tracking:
        raise HTTPException(status_code=409, detail="No active session")

    session = svc.tracker.stop_session()
    return {
        "session_id": session.id,
        "started_at": session.start_time.isoformat(),
        "ended_at": session.end_time.isoformat() if session.end_time else None,
        "kill_count": len(session.kills),
    }

@router.get("/status")
def tracking_status():
    """Get current tracking status."""
    return tracking_status_impl(get_services())


def tracking_status_impl(svc):
    if not hasattr(svc, "tracker") or svc.tracker is None:
        return {"status": "unavailable"}

    # True while the hotbar listener is currently active. Gated on (toggle-on AND
    # session-active), so False when idle.
    hotbar_listener_active = svc.hotbar_listener.is_running
    config = svc.config_service.get()
    weapon_attribution = _weapon_attribution(config)

    if not svc.tracker.is_tracking:
        current_mob, mob_source = _configured_manual_label(config)
        return {
            "status": "idle",
            "hotbarListenerActive": hotbar_listener_active,
            "weaponAttribution": weapon_attribution,
            "repairOcrEnabled": config.repair_ocr_enabled,
            "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
            "mobEntryMode": config.mob_tracking_mode,
            "currentMob": current_mob,
            "mobSource": mob_source,
        }

    session = svc.tracker.session

    # Compute live cost/returns from completed kills + in-progress accumulator
    weapon_cost = sum(
        ts.cost_per_shot * ts.shots_fired
        for kill in session.kills
        for ts in kill.tool_stats.values()
    )
    enhancer_cost = sum(k.enhancer_cost for k in session.kills)

    # Add in-progress accumulator (shots not yet resolved to a kill)
    acc = svc.tracker.current_accumulator
    if acc:
        weapon_cost += acc.weapon_cost
        enhancer_cost += acc.enhancer_cost

    heal_cost = svc.tracker._session_heal_cost if hasattr(svc.tracker, '_session_heal_cost') else 0.0
    cost = weapon_cost + heal_cost + enhancer_cost
    returns = sum(k.loot_total_ped for k in session.kills)

    # Per-kill aggregates for the modular dashboard pills.
    kills = session.kills
    damage_total = sum(k.damage_dealt for k in kills)
    shots_total = sum(k.shots_fired for k in kills)
    crits_total = sum(k.critical_hits for k in kills)
    max_damage = max((k.damage_dealt for k in kills), default=0.0)
    live_weapon_damage = damage_total + (acc.damage_dealt if acc else 0.0)
    globals_count = sum(1 for k in kills if k.is_global)
    hofs_count = sum(1 for k in kills if k.is_hof)
    latest_kill_loot = kills[-1].loot_total_ped if kills else None
    # Multipliers use kill.cost_ped (weapon cost only) per EU community convention.
    mult_per_kill = [k.loot_total_ped / k.cost_ped for k in kills if k.cost_ped > 0]
    multiplier_avg = (sum(mult_per_kill) / len(mult_per_kill)) if mult_per_kill else None
    multiplier_max = max(mult_per_kill, default=None) if mult_per_kill else None
    multiplier_last = (
        kills[-1].loot_total_ped / kills[-1].cost_ped
        if kills and kills[-1].cost_ped > 0
        else None
    )
    # Recent per-kill multipliers (chronological, oldest → newest) for the
    # Loot Pulse chart. Cap at 120 — the frontend picks how many to render
    # based on available width, so we just need enough headroom for a wide
    # window. 120 floats is ~1 KB, negligible at this poll cadence.
    multiplier_history = [round(m, 4) for m in mult_per_kill[-120:]]

    # Cumulative-net history (per kill) for the Loot Pulse P&L chart.
    # Heal cost is session-level and not per-kill-attributable, so we
    # distribute it pro-rata across kills by their weapon cost share. This
    # makes the curve's final point reconcile exactly with the displayed
    # Net stat (returns - cost), modulo rounding.
    per_kill_weapon = [
        sum(ts.cost_per_shot * ts.shots_fired for ts in k.tool_stats.values())
        for k in kills
    ]
    total_weapon = sum(per_kill_weapon)
    cumulative_net_history: list[float] = []
    if kills:
        running = 0.0
        for k, w in zip(kills, per_kill_weapon):
            heal_share = (heal_cost * (w / total_weapon)) if total_weapon > 0 else 0.0
            running += k.loot_total_ped - w - k.enhancer_cost - heal_share
            cumulative_net_history.append(round(running, 2))
        cumulative_net_history = cumulative_net_history[-120:]

    skill_tt = svc.app_db.conn.execute(
        "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
        (session.id,),
    ).fetchone()[0]

    return {
        "status": "active",
        "session_id": session.id,
        "started_at": session.start_time.isoformat(),
        "kill_count": len(session.kills),
        "cost": round(cost, 2),
        "returns": round(returns, 2),
        "pes": round(float(skill_tt), 2),
        "returnRate": round(returns / cost, 4) if cost > 0 else 0.0,
        "damageDealtTotal": round(damage_total, 1),
        "weaponDamageDealt": round(live_weapon_damage, 1),
        "weaponCost": round(weapon_cost, 6),
        "shotsFiredTotal": shots_total,
        "criticalHitsTotal": crits_total,
        "maxDamage": round(max_damage, 1),
        "globalsCount": globals_count,
        "hofsCount": hofs_count,
        "latestKillLoot": round(latest_kill_loot, 2) if latest_kill_loot is not None else None,
        "multiplierLast": round(multiplier_last, 4) if multiplier_last is not None else None,
        "multiplierAvg": round(multiplier_avg, 4) if multiplier_avg is not None else None,
        "multiplierMax": round(multiplier_max, 4) if multiplier_max is not None else None,
        "multiplierHistory": multiplier_history,
        "cumulativeNetHistory": cumulative_net_history,
        "hotbarListenerActive": hotbar_listener_active,
        "weaponAttribution": weapon_attribution,
        "repairOcrEnabled": config.repair_ocr_enabled,
        "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
        "mobEntryMode": svc.tracker._session_mob_tracking_mode,
        "currentMob": svc.tracker._confirmed_mob_name or None,
        "mobSource": svc.tracker._mob_source if svc.tracker._confirmed_mob_name else None,
    }

@router.post("/release-mob")
def release_mob():
    """Release the currently locked mob."""
    svc = get_services()
    tracker = getattr(svc, "tracker", None)
    config = svc.config_service.get()

    if tracker is not None and tracker.is_tracking and tracker.is_session_tag_mode():
        released = tracker.release_current_mob()
        svc.config_service.update({"mob_tracking_tag": ""})
        return {"released": released}

    if tracker is None or not tracker.is_tracking:
        if _is_tag_mode(config):
            released = config.mob_tracking_tag.strip() or None
            svc.config_service.update({"mob_tracking_tag": ""})
            return {"released": released}
        species = getattr(config, "manual_mob_species", "").strip()
        maturity = getattr(config, "manual_mob_maturity", "").strip()
        released = None
        if species:
            released = f"{maturity} {species}" if maturity else species
        svc.config_service.update({
            "manual_mob_species": "",
            "manual_mob_maturity": "",
        })
        return {"released": released}

    released = tracker.release_current_mob()
    svc.config_service.update({
        "manual_mob_species": "",
        "manual_mob_maturity": "",
    })
    return {"released": released}

@router.get("/manual-mob-suggestions")
def manual_mob_suggestions(q: str = "", limit: int = 10):
    """Autocomplete suggestions for manual mob lock."""
    svc = get_services()
    config = svc.config_service.get()
    if svc.tracker.is_tracking and svc.tracker.is_session_tag_mode():
        raise HTTPException(status_code=409, detail="Tag mode disables manual mob selection")
    if not svc.tracker.is_tracking and config.mob_tracking_mode == "tag":
        raise HTTPException(status_code=409, detail="Tag mode disables manual mob selection")

    query = q.strip()
    if not query:
        return []

    return svc.mob_lookup.search_mob_names(query, limit=max(1, min(limit, 20)))

@router.post("/manual-mob-lock")
def manual_mob_lock(req: ManualMobLockRequest):
    """Immediately lock the selected catalogue mob for manual kill stamping."""
    svc = get_services()
    config = svc.config_service.get()
    if svc.tracker.is_tracking and svc.tracker.is_session_tag_mode():
        raise HTTPException(status_code=409, detail="Tag mode disables manual mob selection")
    if not svc.tracker.is_tracking and config.mob_tracking_mode == "tag":
        raise HTTPException(status_code=409, detail="Tag mode disables manual mob selection")

    species = req.species.strip()
    maturity = req.maturity.strip()
    if not svc.mob_lookup.has_mob_name(species, maturity):
        raise HTTPException(status_code=400, detail="Mob is not present in the catalogue")

    display = f"{maturity} {species}" if maturity else species
    svc.config_service.update({
        "manual_mob_species": species,
        "manual_mob_maturity": maturity,
    })
    if svc.tracker.is_tracking:
        svc.tracker.set_manual_mob(display, species, maturity)
    return {"mobName": display, "species": species, "maturity": maturity}

@router.post("/tag-lock")
def tag_lock(req: TagLockRequest):
    """Immediately set the active free-text tag for tag-mode kill stamping."""
    svc = get_services()
    config = svc.config_service.get()
    if svc.tracker.is_tracking:
        if not svc.tracker.is_session_tag_mode():
            raise HTTPException(status_code=409, detail="Active session is not in tag mode")
    elif config.mob_tracking_mode != "tag":
        raise HTTPException(status_code=409, detail="Tag mode is not enabled")

    tag = req.tag.strip()
    if not tag:
        raise HTTPException(status_code=400, detail="Tag cannot be empty")

    svc.config_service.update({"mob_tracking_tag": tag})
    if svc.tracker.is_tracking:
        svc.tracker.set_manual_tag(tag)
    return {"tag": tag}

@router.get("/tag-suggestions")
def tag_suggestions(q: str = "", limit: int = 10):
    """Autocomplete suggestions for free-text session mob tags."""
    svc = get_services()
    query = q.strip()
    if not query:
        return []

    rows = svc.app_db.conn.execute(
        """SELECT mob_name, COUNT(*) as uses
           FROM kills
           WHERE mob_name IS NOT NULL
             AND mob_name != 'Unknown'
             AND COALESCE(mob_species, '') = ''
             AND COALESCE(mob_maturity, '') = ''
             AND lower(mob_name) LIKE ?
           GROUP BY mob_name
           ORDER BY uses DESC, mob_name ASC
           LIMIT ?""",
        (f"%{query.lower()}%", max(1, min(limit, 20))),
    ).fetchall()
    return [row[0] for row in rows]

@router.get("/live")
def tracking_live():
    """Live session data for the overlay — compact stats + current mob."""
    return tracking_live_impl(get_services())


def tracking_live_impl(svc):
    started = time.perf_counter()
    if not hasattr(svc, "tracker") or svc.tracker is None:
        return {"status": "unavailable"}

    detected_tool = getattr(svc.tracker, "_active_hotbar_tool_name", None)
    config = svc.config_service.get()
    weapon_attribution = _weapon_attribution(config)
    trifecta_attribution = (
        _trifecta_attribution_summary(svc) if weapon_attribution == "trifecta" else None
    )

    if not svc.tracker.is_tracking:
        current_mob, mob_source = _configured_manual_label(config)
        return {
            "status": "idle",
            "weaponAttribution": weapon_attribution,
            "repairOcrEnabled": config.repair_ocr_enabled,
            "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
            "mobEntryMode": config.mob_tracking_mode,
            "currentMob": current_mob,
            "mobSource": mob_source,
            "currentTool": detected_tool,
            "trifectaAttribution": trifecta_attribution,
        }

    session = svc.tracker.session
    start_ts = session.start_time.timestamp()
    elapsed = int(time.time() - start_ts)

    # Compute live cost/returns from completed kills + in-progress accumulator
    weapon_cost = sum(
        ts.cost_per_shot * ts.shots_fired
        for kill in session.kills
        for ts in kill.tool_stats.values()
    )
    enhancer_cost = sum(k.enhancer_cost for k in session.kills)

    acc = svc.tracker.current_accumulator
    if acc:
        weapon_cost += acc.weapon_cost
        enhancer_cost += acc.enhancer_cost

    heal_cost = svc.tracker._session_heal_cost if hasattr(svc.tracker, '_session_heal_cost') else 0.0
    cost = weapon_cost + heal_cost + enhancer_cost
    returns = sum(k.loot_total_ped for k in session.kills)
    kills = len(session.kills)

    current_mob = svc.tracker._confirmed_mob_name or None

    # Recent notable events from this session
    notable = svc.app_db.conn.execute(
        """SELECT event_type, mob_or_item, value_ped, timestamp
           FROM notable_events WHERE session_id = ?
           ORDER BY timestamp DESC LIMIT 5""",
        (session.id,),
    ).fetchall()

    recent_events_list = []

    # Warnings from the tracker (e.g., heal tool not equipped)
    if hasattr(svc.tracker, '_session_warnings'):
        for msg in svc.tracker._session_warnings:
            recent_events_list.append({
                "type": "warning",
                "description": msg,
                "value": 0,
            })

    for i, r in enumerate(notable):
        event_type, mob_or_item, value_ped, timestamp = r[0], r[1], r[2], r[3]
        recent_events_list.append(_notable_event_payload(event_type, mob_or_item, value_ped, timestamp))

    # Skill TT from this session
    skill_tt = svc.app_db.conn.execute(
        "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
        (session.id,),
    ).fetchone()[0]

    payload = {
        "status": "active",
        "sessionId": session.id,
        "elapsed": elapsed,
        "killCount": len(session.kills),
        "kills": kills,
        "cost": round(cost, 2),
        "returns": round(returns, 2),
        "pes": round(float(skill_tt), 2),
        "net": round(returns - cost, 2),
        "returnRate": round(returns / cost, 4) if cost > 0 else 0.0,
        "weaponAttribution": weapon_attribution,
        "repairOcrEnabled": config.repair_ocr_enabled,
        "endOfSessionArmourReminderEnabled": config.end_of_session_armour_reminder_enabled,
        "mobEntryMode": svc.tracker._session_mob_tracking_mode,
        "currentMob": current_mob,
        "mobSource": svc.tracker._mob_source if current_mob else None,
        "currentTool": detected_tool,
        "trifectaAttribution": trifecta_attribution,
        "recentEvents": recent_events_list,
    }
    duration_ms = (time.perf_counter() - started) * 1000.0
    if log.isEnabledFor(logging.DEBUG) and duration_ms >= 10.0:
        log.debug(
            "/tracking/live slow-ish response: %.2f ms (kills=%d recent_events=%d warnings=%d)",
            duration_ms,
            kills,
            len(notable),
            len(getattr(svc.tracker, "_session_warnings", [])),
        )
    return payload

@router.get("/recent-events")
def recent_events():
    """Recent notable events for the latest tracking session — dashboard activity feed.

    Scoped to the most recent session (by started_at) so starting a fresh session
    clears the dashboard feed; events populate again as they occur in-session.
    """
    return recent_events_impl(get_services())


def recent_events_impl(svc):
    latest = svc.app_db.conn.execute(
        "SELECT id FROM tracking_sessions ORDER BY started_at DESC LIMIT 1"
    ).fetchone()
    if not latest:
        return []
    rows = svc.app_db.conn.execute(
        """SELECT ne.event_type, ne.mob_or_item, ne.value_ped, ne.timestamp
           FROM notable_events ne
           WHERE ne.session_id = ?
           ORDER BY ne.timestamp DESC LIMIT 20""",
        (latest[0],),
    ).fetchall()

    events = []
    for i, r in enumerate(rows):
        event_type, mob_or_item, value_ped, ts = r
        payload = _notable_event_payload(event_type, mob_or_item, value_ped)
        events.append({
            "id": f"ne-{i}",
            **payload,
            "timestamp": _ts_to_iso(ts),
        })
    return events

@router.get("/sessions")
def list_sessions():
    """List recent tracking sessions with aggregated stats.

    Returns shapes matching the frontend TrackingSession type.
    """
    return list_sessions_impl(get_services().app_db.conn)


def list_sessions_impl(conn):
    rows = conn.execute(
        """SELECT id, started_at, ended_at, is_active
           FROM tracking_sessions ORDER BY started_at DESC LIMIT 20""",
    ).fetchall()

    sessions = []
    for row in rows:
        sid, started_at, ended_at, is_active = row[0], row[1], row[2], bool(row[3])

        # Duration
        if ended_at and started_at:
            duration = int(ended_at - started_at)
        elif is_active and started_at:
            duration = int(time.time() - started_at)
        else:
            duration = 0

        # Cost: weapon cycling + heal + enhancer + armour + dangling
        weapon_cost = conn.execute(
            """SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0)
               FROM kill_tool_stats ts
               JOIN kills k ON k.id = ts.kill_id
               WHERE k.session_id = ?""",
            (sid,),
        ).fetchone()[0]

        enhancer_cost_val = conn.execute(
            "SELECT COALESCE(SUM(k.enhancer_cost), 0) FROM kills k WHERE k.session_id = ?",
            (sid,),
        ).fetchone()[0]

        sess_costs = conn.execute(
            "SELECT COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) FROM tracking_sessions WHERE id = ?",
            (sid,),
        ).fetchone()
        armour_cost = sess_costs[0]
        heal_cost_val = sess_costs[1]
        dangling_cost = sess_costs[2]

        cost = weapon_cost + heal_cost_val + enhancer_cost_val + armour_cost + dangling_cost

        # Returns: sum of loot
        returns = conn.execute(
            "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
            (sid,),
        ).fetchone()[0]

        # Primary mobs (top 3 by kill count)
        primary_mobs = [
            r[0]
            for r in conn.execute(
                """SELECT mob_name FROM kills
                   WHERE session_id = ? AND mob_name IS NOT NULL AND mob_name != 'Unknown'
                   GROUP BY mob_name ORDER BY COUNT(*) DESC LIMIT 3""",
                (sid,),
            ).fetchall()
        ]

        # Primary weapons (top 3 by shots fired)
        primary_weapons = [
            r[0]
            for r in conn.execute(
                """SELECT ts.tool_name FROM kill_tool_stats ts
                   JOIN kills k ON k.id = ts.kill_id
                   WHERE k.session_id = ? AND ts.tool_name IS NOT NULL AND ts.tool_name != 'Unknown'
                   GROUP BY ts.tool_name ORDER BY SUM(ts.shots_fired) DESC LIMIT 3""",
                (sid,),
            ).fetchall()
        ]

        # Net is liquid-only — skill_tt is progression (PES) and stays out
        # of the session-list net.
        net = returns - cost
        return_rate = returns / cost if cost > 0 else 0.0

        # Notable event counts
        notable_counts = conn.execute(
            """SELECT
                 COALESCE(SUM(CASE WHEN event_type LIKE 'global_%' THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN event_type LIKE 'hof_%' THEN 1 ELSE 0 END), 0)
               FROM notable_events WHERE session_id = ?""",
            (sid,),
        ).fetchone()

        sessions.append({
            "id": sid,
            "startTime": _ts_to_iso(started_at),
            "endTime": _ts_to_iso(ended_at),
            "duration": duration,
            "primaryMobs": primary_mobs,
            "primaryWeapons": primary_weapons,
            "cost": round(cost, 2),
            "returns": round(returns, 2),
            "net": round(net, 2),
            "returnRate": round(return_rate, 4),
            "globals": notable_counts[0],
            "hofs": notable_counts[1],
        })
    return sessions

@router.delete("/session/{session_id}")
def delete_session(session_id: str):
    """Delete a tracking session and all associated data."""
    svc = get_services()
    conn = svc.app_db.conn

    # Verify session exists
    row = conn.execute(
        "SELECT id, is_active FROM tracking_sessions WHERE id = ?", (session_id,),
    ).fetchone()
    if not row:
        raise HTTPException(status_code=404, detail="Session not found")
    if row[1]:
        raise HTTPException(status_code=409, detail="Cannot delete an active session")

    # Get kill IDs for child table cleanup
    kill_ids = [r[0] for r in conn.execute(
        "SELECT id FROM kills WHERE session_id = ?", (session_id,),
    ).fetchall()]

    if kill_ids:
        ph = ",".join("?" * len(kill_ids))
        conn.execute(f"DELETE FROM kill_tool_stats WHERE kill_id IN ({ph})", kill_ids)
        conn.execute(f"DELETE FROM kill_loot_items WHERE kill_id IN ({ph})", kill_ids)

    conn.execute("DELETE FROM kills WHERE session_id = ?", (session_id,))
    conn.execute("DELETE FROM skill_gains WHERE session_id = ?", (session_id,))
    conn.execute("DELETE FROM notable_events WHERE session_id = ?", (session_id,))
    conn.execute("DELETE FROM session_summaries WHERE session_id = ?", (session_id,))
    conn.execute("DELETE FROM tracking_sessions WHERE id = ?", (session_id,))
    conn.commit()

    return {"status": "deleted", "sessionId": session_id}

@router.get("/session/{session_id}")
def get_session(session_id: str):
    """Get full session detail with aggregated summary.

    Returns shape matching the frontend SessionDetail type.
    """
    return get_session_impl(get_services().app_db.conn, session_id)


# ── Loot-entry deactivate / activate (post-hoc sessions-tab editing) ──
#
# Hidden-flag overlay: a row's deactivated_at flips between NULL (active)
# and unixepoch('now') (deactivated). The denormalised kills.loot_total_ped
# is mutated in the same transaction so the analytics surface that reads
# SUM(kills.loot_total_ped) stays untouched by this affordance. The
# session_summaries cache row is invalidated so the next session-list read
# recomputes.

def _lookup_loot_for_session(conn, session_id: str, loot_item_id: int):
    """Resolve a loot row scoped to a session.

    Returns (kill_id, value_ped, deactivated_at). Raises 404 if the
    session does not exist, or if the loot row does not exist within it
    (cross-session ids cannot reach into another session).
    """
    session_row = conn.execute(
        "SELECT id FROM tracking_sessions WHERE id = ?", (session_id,),
    ).fetchone()
    if not session_row:
        raise HTTPException(status_code=404, detail="Session not found")

    loot_row = conn.execute(
        "SELECT l.kill_id, l.value_ped, l.deactivated_at "
        "FROM kill_loot_items l "
        "JOIN kills k ON k.id = l.kill_id "
        "WHERE l.id = ? AND k.session_id = ?",
        (loot_item_id, session_id),
    ).fetchone()
    if not loot_row:
        raise HTTPException(status_code=404, detail="Loot item not found in this session")
    return loot_row[0], float(loot_row[1] or 0.0), loot_row[2]


def _build_loot_edit_response(conn, session_id: str, kill_id: str, loot_item_id: int):
    """Build the response payload returned by deactivate / activate.

    Includes the post-mutation flag plus the two totals the frontend
    re-renders against (per-kill denormalised total + per-session
    SUM aggregate) so the client can update without a full session
    refetch.
    """
    flag_row = conn.execute(
        "SELECT deactivated_at FROM kill_loot_items WHERE id = ?", (loot_item_id,),
    ).fetchone()
    kill_total = conn.execute(
        "SELECT loot_total_ped FROM kills WHERE id = ?", (kill_id,),
    ).fetchone()[0]
    session_returns = conn.execute(
        "SELECT COALESCE(SUM(loot_total_ped), 0) FROM kills WHERE session_id = ?",
        (session_id,),
    ).fetchone()[0]
    return {
        "sessionId": session_id,
        "killId": kill_id,
        "lootItemId": loot_item_id,
        "deactivatedAt": _ts_to_iso(flag_row[0]),
        "killLootTotalPed": round(float(kill_total or 0.0), 4),
        "sessionTotalReturns": round(float(session_returns or 0.0), 2),
    }


@router.post("/session/{session_id}/loot/{loot_item_id}/deactivate")
def deactivate_loot_item(session_id: str, loot_item_id: int):
    """Soft-deactivate a loot row; reversible via the activate endpoint.

    Atomically: stamps `deactivated_at`, subtracts the row's `value_ped`
    from the parent `kills.loot_total_ped`, invalidates the
    `session_summaries` cache for the session. 409 if the row is already
    deactivated (idempotency would silently no-op the kill-total update
    on a stale client retry, which is the wrong default).
    """
    conn = get_services().app_db.conn
    return _deactivate_loot_item_impl(conn, session_id, loot_item_id)


def _deactivate_loot_item_impl(conn, session_id: str, loot_item_id: int):
    """Backend-side deactivate operation; the connection-injectable form
    of `deactivate_loot_item` for direct testing against an arbitrary
    SQLite connection without spinning up the full services container."""
    kill_id, value_ped, deactivated_at = _lookup_loot_for_session(
        conn, session_id, loot_item_id,
    )
    if deactivated_at is not None:
        raise HTTPException(status_code=409, detail="Loot item is already deactivated")

    try:
        conn.execute(
            "UPDATE kill_loot_items SET deactivated_at = unixepoch('now') WHERE id = ?",
            (loot_item_id,),
        )
        conn.execute(
            "UPDATE kills SET loot_total_ped = loot_total_ped - ? WHERE id = ?",
            (value_ped, kill_id),
        )
        conn.execute(
            "DELETE FROM session_summaries WHERE session_id = ?", (session_id,),
        )
        conn.commit()
    except Exception:
        conn.rollback()
        raise

    return _build_loot_edit_response(conn, session_id, kill_id, loot_item_id)


@router.post("/session/{session_id}/loot/{loot_item_id}/activate")
def activate_loot_item(session_id: str, loot_item_id: int):
    """Reactivate a previously deactivated loot row.

    Inverse of deactivate: clears `deactivated_at`, adds the row's
    `value_ped` back to the parent `kills.loot_total_ped`, invalidates
    the `session_summaries` cache. 409 if the row is already active.
    """
    conn = get_services().app_db.conn
    return _activate_loot_item_impl(conn, session_id, loot_item_id)


class RenameMobRequest(BaseModel):
    fromMobName: str
    toMobName: str


class RestoreMobRequest(BaseModel):
    currentMobName: str


# ── Session metadata edit: rename mob / restore mob ──────────────────
#
# Mass-rename overlay: editing a session's attributed mob name rewrites
# `kills.mob_name` for all kills in that session whose current mob_name
# matches the `from` value. The pre-edit value is preserved into
# `kills.original_mob_name` via COALESCE on the first rename, so the
# inverse restore endpoint can revert even after multiple consecutive
# renames (COALESCE keeps the *first* original, so undo lands at the
# genuinely-original capture). Tag-mode sessions persist the tag into
# `kills.mob_name` at write time, so the same endpoint covers tag edits
# transparently (frontend labels the affordance based on session mode).

def _validate_session_exists(conn, session_id: str) -> None:
    """Validate that the session exists and is not still active.

    Raises 404 if missing. Raises 409 if the session is still active:
    rename and restore are post-hoc operations, and editing kills.mob_name
    on a live session creates drift between SQLite and the tracker's
    in-memory state (the tracker continues writing further kills under
    the pre-edit name + can stamp the session's stop-flow with stale
    aggregates). The frontend should only expose the edit affordances
    after the session has ended.
    """
    row = conn.execute(
        "SELECT id, is_active FROM tracking_sessions WHERE id = ?", (session_id,),
    ).fetchone()
    if not row:
        raise HTTPException(status_code=404, detail="Session not found")
    if bool(row[1]):
        raise HTTPException(
            status_code=409,
            detail="Session mob edits are only available after the session has ended",
        )


def _build_mob_edit_response(conn, session_id: str, mob_name: str):
    """Build the response payload for rename-mob / restore-mob.

    Queries the post-mutation per-mob kill count for the resulting
    `mob_name` so the frontend can re-render the session row + the
    per-mob breakdown without a full session refetch. Queries (rather
    than reusing the affected-rows count) because the destination
    `mob_name` may have had pre-existing kills in the session: a
    rename A->C when C already has kills lands the destination at
    `affected + pre-existing`, not just `affected`.
    """
    row = conn.execute(
        "SELECT COUNT(*) FROM kills WHERE session_id = ? AND mob_name = ?",
        (session_id, mob_name),
    ).fetchone()
    return {
        "sessionId": session_id,
        "mobName": mob_name,
        "killCount": int(row[0] or 0),
    }


@router.post("/session/{session_id}/rename-mob")
def rename_session_mob(session_id: str, body: RenameMobRequest):
    """Rewrite kills.mob_name for matching kills in this session.

    Atomically: preserves the pre-edit `mob_name` into
    `original_mob_name` via COALESCE (first-original semantics), rewrites
    `mob_name` for every matching kill in the session, invalidates the
    `session_summaries` cache row. 404 on missing session. 409 if the
    `fromMobName` value has no matching kills in the session, or if
    `fromMobName == toMobName` (the no-op rename would silently succeed
    otherwise).
    """
    conn = get_services().app_db.conn
    return _rename_session_mob_impl(conn, session_id, body.fromMobName, body.toMobName)


def _rename_session_mob_impl(conn, session_id: str, from_mob: str, to_mob: str):
    """Backend-side rename operation; the connection-injectable form of
    `rename_session_mob` for direct testing against an arbitrary SQLite
    connection without spinning up the full services container."""
    _validate_session_exists(conn, session_id)
    from_mob = from_mob.strip()
    to_mob = to_mob.strip()
    if not from_mob or not to_mob:
        raise HTTPException(
            status_code=400,
            detail="Mob names cannot be blank",
        )
    if from_mob == to_mob:
        raise HTTPException(
            status_code=409,
            detail="rename target matches the current value (no-op)",
        )

    matching = conn.execute(
        "SELECT COUNT(*) FROM kills WHERE session_id = ? AND mob_name = ?",
        (session_id, from_mob),
    ).fetchone()[0]
    if matching == 0:
        raise HTTPException(
            status_code=409,
            detail=f"No kills in this session match mob_name='{from_mob}'",
        )

    try:
        conn.execute(
            "UPDATE kills "
            "SET original_mob_name = COALESCE(original_mob_name, mob_name) "
            "WHERE session_id = ? AND mob_name = ?",
            (session_id, from_mob),
        )
        conn.execute(
            "UPDATE kills SET mob_name = ? WHERE session_id = ? AND mob_name = ?",
            (to_mob, session_id, from_mob),
        )
        conn.execute(
            "DELETE FROM session_summaries WHERE session_id = ?", (session_id,),
        )
        conn.commit()
    except Exception:
        conn.rollback()
        raise

    return _build_mob_edit_response(conn, session_id, to_mob)


@router.post("/session/{session_id}/restore-mob")
def restore_session_mob(session_id: str, body: RestoreMobRequest):
    """Revert kills in this session whose current mob_name matches the
    request and carry a preserved `original_mob_name`.

    Inverse of rename-mob. Atomically: rewrites `mob_name` back to
    `original_mob_name`, clears `original_mob_name`, invalidates the
    `session_summaries` cache. 404 on missing session. 409 if no kills
    in the session match the request (either nothing has been renamed
    to that current name, or no preserved original exists to restore).
    """
    conn = get_services().app_db.conn
    return _restore_session_mob_impl(conn, session_id, body.currentMobName)


def _restore_session_mob_impl(conn, session_id: str, current_mob: str):
    """Backend-side restore operation; the connection-injectable form of
    `restore_session_mob` for direct testing against an arbitrary SQLite
    connection without spinning up the full services container."""
    _validate_session_exists(conn, session_id)
    current_mob = current_mob.strip()
    if not current_mob:
        raise HTTPException(
            status_code=400,
            detail="Mob name cannot be blank",
        )

    eligible = conn.execute(
        "SELECT COUNT(*), COUNT(DISTINCT original_mob_name), MIN(original_mob_name) "
        "FROM kills "
        "WHERE session_id = ? AND mob_name = ? AND original_mob_name IS NOT NULL",
        (session_id, current_mob),
    ).fetchone()
    matching = int(eligible[0] or 0)
    distinct_originals = int(eligible[1] or 0)
    restored_to = eligible[2]
    if matching == 0:
        raise HTTPException(
            status_code=409,
            detail=(
                f"No restorable kills in this session for mob_name='{current_mob}' "
                "(either no rename has happened or the preservation column is empty)"
            ),
        )
    if distinct_originals > 1:
        # Two or more distinct prior names merged into the same current
        # mob_name (e.g. rename A->C, then rename B->C). Restoring would
        # need to split the cohort back into multiple destinations,
        # which the single-result response shape cannot express
        # unambiguously. Refuse with an informative 409 so the caller
        # knows the situation; the API does not offer a target-by-
        # original-name endpoint, so this case is for the frontend to
        # surface to the user.
        raise HTTPException(
            status_code=409,
            detail=(
                f"Ambiguous restore for mob_name='{current_mob}': "
                f"{distinct_originals} distinct prior names merged into it."
            ),
        )

    try:
        conn.execute(
            "UPDATE kills "
            "SET mob_name = original_mob_name, original_mob_name = NULL "
            "WHERE session_id = ? AND mob_name = ? AND original_mob_name IS NOT NULL",
            (session_id, current_mob),
        )
        conn.execute(
            "DELETE FROM session_summaries WHERE session_id = ?", (session_id,),
        )
        conn.commit()
    except Exception:
        conn.rollback()
        raise

    return _build_mob_edit_response(conn, session_id, restored_to)


def _activate_loot_item_impl(conn, session_id: str, loot_item_id: int):
    """Backend-side activate operation; the connection-injectable form
    of `activate_loot_item` for direct testing against an arbitrary
    SQLite connection without spinning up the full services container."""
    kill_id, value_ped, deactivated_at = _lookup_loot_for_session(
        conn, session_id, loot_item_id,
    )
    if deactivated_at is None:
        raise HTTPException(status_code=409, detail="Loot item is already active")

    try:
        conn.execute(
            "UPDATE kill_loot_items SET deactivated_at = NULL WHERE id = ?",
            (loot_item_id,),
        )
        conn.execute(
            "UPDATE kills SET loot_total_ped = loot_total_ped + ? WHERE id = ?",
            (value_ped, kill_id),
        )
        conn.execute(
            "DELETE FROM session_summaries WHERE session_id = ?", (session_id,),
        )
        conn.commit()
    except Exception:
        conn.rollback()
        raise

    return _build_loot_edit_response(conn, session_id, kill_id, loot_item_id)


def get_session_impl(conn, session_id: str):
    session_row = conn.execute(
        "SELECT id, started_at, ended_at, is_active FROM tracking_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    if not session_row:
        raise HTTPException(status_code=404, detail="Session not found")

    started_at, ended_at, is_active = session_row[1], session_row[2], bool(session_row[3])

    # Duration
    if ended_at and started_at:
        duration = int(ended_at - started_at)
    elif is_active and started_at:
        duration = int(time.time() - started_at)
    else:
        duration = 0

    # Session-level costs
    sess_costs = conn.execute(
        "SELECT COALESCE(armour_cost, 0), COALESCE(heal_cost, 0), COALESCE(dangling_cost, 0) FROM tracking_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    armour_cost = sess_costs[0]
    session_heal_cost = sess_costs[1]
    dangling_cost = sess_costs[2]

    # Session totals straight from kills — one query, no Python loop.
    kill_totals = conn.execute(
        "SELECT COUNT(*), COALESCE(SUM(loot_total_ped), 0), COALESCE(SUM(enhancer_cost), 0) "
        "FROM kills WHERE session_id = ?",
        (session_id,),
    ).fetchone()
    kills = int(kill_totals[0] or 0)
    total_returns = float(kill_totals[1] or 0.0)
    total_enhancer_cost = float(kill_totals[2] or 0.0)

    # Tool stats aggregated across the whole session in a single query.
    tool_rows = conn.execute(
        "SELECT t.tool_name, "
        "       SUM(t.shots_fired), "
        "       SUM(t.damage_dealt), "
        "       SUM(t.critical_hits), "
        "       SUM(COALESCE(t.cost_per_shot, 0) * COALESCE(t.shots_fired, 0)) "
        "FROM kill_tool_stats t "
        "JOIN kills k ON k.id = t.kill_id "
        "WHERE k.session_id = ? "
        "GROUP BY t.tool_name",
        (session_id,),
    ).fetchall()
    weapon_cost = 0.0
    merged_tools: dict[str, dict] = {}
    for name, shots, dmg, crits, cost_attr in tool_rows:
        cost_attr = float(cost_attr or 0.0)
        weapon_cost += cost_attr
        merged_tools[name] = {
            "shotsFired": int(shots or 0),
            "damageDealt": float(dmg or 0.0),
            "crits": int(crits or 0),
            "costAttributed": cost_attr,
        }

    # Loot breakdown aggregated in a single query. Deactivated rows are
    # filtered out of the aggregate so the existing UI surface (item-name
    # rollup) reflects the user's post-hoc edits; the individual rows
    # remain accessible via `lootEntries` below for the editing affordance.
    loot_agg_rows = conn.execute(
        "SELECT l.item_name, SUM(l.quantity), SUM(l.value_ped) "
        "FROM kill_loot_items l "
        "JOIN kills k ON k.id = l.kill_id "
        "WHERE k.session_id = ? "
        "AND COALESCE(l.is_enhancer_shrapnel, 0) = 0 "
        "AND l.deactivated_at IS NULL "
        "GROUP BY l.item_name",
        (session_id,),
    ).fetchall()
    merged_loot: dict[str, dict] = {
        name: {"quantity": int(qty or 0), "ttValue": float(val or 0.0)}
        for name, qty, val in loot_agg_rows
    }

    # Per-mob breakdown for the sessions-tab metadata-edit affordance.
    # Surfaces both the current attributed `mob_name` and any preserved
    # `original_mob_name` so the frontend can render an "originally X"
    # indicator on renamed mobs and offer a restore action. Sorted by
    # kill count descending so the most-hunted mob appears first.
    mob_breakdown_rows = conn.execute(
        "SELECT mob_name, original_mob_name, COUNT(*) "
        "FROM kills "
        "WHERE session_id = ? AND mob_name IS NOT NULL "
        "GROUP BY mob_name, original_mob_name "
        "ORDER BY COUNT(*) DESC",
        (session_id,),
    ).fetchall()
    mob_breakdown = [
        {
            "currentName": row[0],
            "originalName": row[1],
            "killCount": int(row[2] or 0),
        }
        for row in mob_breakdown_rows
    ]

    # Per-row loot detail for the sessions-tab editing affordance. Returned
    # unfiltered so the frontend can render both active and deactivated
    # rows (the latter greyed out, with an Activate action). Sorted by id
    # to keep insertion order stable across reads.
    loot_entry_rows = conn.execute(
        "SELECT l.id, l.kill_id, l.item_name, l.quantity, l.value_ped, "
        "       l.is_enhancer_shrapnel, l.deactivated_at "
        "FROM kill_loot_items l "
        "JOIN kills k ON k.id = l.kill_id "
        "WHERE k.session_id = ? "
        "ORDER BY l.id",
        (session_id,),
    ).fetchall()
    loot_entries = [
        {
            "id": row[0],
            "killId": row[1],
            "itemName": row[2],
            "quantity": int(row[3] or 0),
            "valuePed": float(row[4] or 0.0),
            "isEnhancerShrapnel": bool(row[5]),
            "deactivatedAt": _ts_to_iso(row[6]),
        }
        for row in loot_entry_rows
    ]

    total_cost = weapon_cost + session_heal_cost + total_enhancer_cost + armour_cost + dangling_cost

    detail_skill_tt = conn.execute(
        "SELECT COALESCE(SUM(ped_value), 0) FROM skill_gains WHERE session_id = ?",
        (session_id,),
    ).fetchone()[0]

    # Net is liquid-only — skill_tt is progression (PES) and stays out of P&L.
    # The PES value is still surfaced as
    # `summary.pes` for the SessionDetail card to display alongside.
    net = total_returns - total_cost
    return_rate = total_returns / total_cost if total_cost > 0 else 0.0

    # Build loot breakdown sorted by TT value descending
    loot_breakdown = sorted(
        [{"name": k, "quantity": v["quantity"], "ttValue": round(v["ttValue"], 2)} for k, v in merged_loot.items()],
        key=lambda x: x["ttValue"],
        reverse=True,
    )

    # Build tool stats sorted by shots fired descending
    tool_stats = sorted(
        [
            {
                "weaponName": k,
                "shotsFired": v["shotsFired"],
                "damageDealt": v["damageDealt"],
                "crits": v["crits"],
                "costAttributed": round(v["costAttributed"], 2),
            }
            for k, v in merged_tools.items()
        ],
        key=lambda x: x["shotsFired"],
        reverse=True,
    )

    # Notable events
    notable_rows = conn.execute(
        "SELECT event_type, mob_or_item, value_ped FROM notable_events WHERE session_id = ? ORDER BY timestamp",
        (session_id,),
    ).fetchall()
    notable_events = []
    for nr in notable_rows:
        evt_type = nr[0]
        payload = _notable_event_payload(evt_type, nr[1], nr[2])
        notable_events.append({
            "type": payload["type"],
            "eventType": payload["eventType"],
            "target": nr[1],
            "item": nr[1],
            "value": nr[2],
        })

    return {
        "sessionId": session_id,
        "summary": {
            "cost": round(total_cost, 2),
            "returns": round(total_returns, 2),
            "pes": round(float(detail_skill_tt), 2),
            "net": round(net, 2),
            "returnRate": round(return_rate, 4),
            "kills": kills,
            "duration": duration,
            "costBreakdown": {
                "weaponCost": round(weapon_cost, 2),
                "healCost": round(session_heal_cost, 2),
                "enhancerCost": round(total_enhancer_cost, 2),
                "armourCost": round(armour_cost, 2),
            },
        },
        "notableEvents": notable_events,
        "lootBreakdown": loot_breakdown,
        "lootEntries": loot_entries,
        "mobBreakdown": mob_breakdown,
        "effectiveLoot": round(total_returns, 2),
        "toolStats": tool_stats,
        "skillGains": _session_skill_gains(conn, session_id),
    }

def _session_skill_gains(conn, session_id: str) -> list[dict]:
    """Aggregate skill gains for a session, with current calibrated level."""
    attr_placeholders = ",".join("?" * len(ATTRIBUTE_SKILLS))
    rows = conn.execute(
        f"""SELECT sg.skill_name,
                  SUM(sg.amount) as total_amount,
                  COALESCE(SUM(sg.ped_value), 0) as total_ped
           FROM skill_gains sg
           WHERE sg.session_id = ?
             AND sg.skill_name NOT IN ({attr_placeholders})
           GROUP BY sg.skill_name
           ORDER BY total_ped DESC""",
        (session_id, *ATTRIBUTE_SKILLS),
    ).fetchall()

    if not rows:
        return []

    # Batch-fetch latest calibrated levels for all skills in one query
    skill_names = [r[0] for r in rows]
    placeholders = ",".join("?" * len(skill_names))
    cal_rows = conn.execute(
        f"""SELECT skill_name, level FROM skill_calibrations
            WHERE id IN (
                SELECT MAX(id) FROM skill_calibrations
                WHERE skill_name IN ({placeholders})
                GROUP BY skill_name
            )""",
        skill_names,
    ).fetchall()
    levels = {r[0]: r[1] for r in cal_rows}

    return [
        {
            "skillName": r[0],
            "level": round(levels.get(r[0], 0), 1),
            "ttValueGained": round(r[2], 4),
        }
        for r in rows
    ]

# ------------------------------------------------------------------
# Repair OCR — post-session armour cost capture
# ------------------------------------------------------------------

@router.post("/session/{session_id}/repair-scan")
def repair_scan(session_id: str):
    """Run OCR on the bundled-anchor repair region. Returns result without saving."""
    svc = get_services()
    cfg = svc.config_service.get()
    if not cfg.repair_ocr_enabled:
        raise HTTPException(status_code=400, detail="Repair OCR is disabled")
    return svc.repair_ocr.scan_repair_cost()

class ArmourCostBody(BaseModel):
    cost: float

@router.post("/session/{session_id}/armour-cost")
def set_armour_cost(session_id: str, body: ArmourCostBody):
    """Save armour repair cost to a session."""
    svc = get_services()
    row = svc.app_db.conn.execute(
        "SELECT id FROM tracking_sessions WHERE id = ?", (session_id,),
    ).fetchone()
    if not row:
        raise HTTPException(status_code=404, detail="Session not found")

    svc.app_db.conn.execute(
        "UPDATE tracking_sessions SET armour_cost = COALESCE(armour_cost, 0) + ? WHERE id = ?",
        (body.cost, session_id),
    )
    svc.app_db.conn.commit()
    return {"sessionId": session_id, "armourCost": round(body.cost, 2)}

class SessionQuestLinkDecisionBody(BaseModel):
    action: str

@router.get("/session/{session_id}/quest-link-suggestion")
def get_session_quest_link_suggestion(session_id: str):
    """Get the curated post-session quest analytics linkage suggestion."""
    svc = get_services()
    row = svc.app_db.conn.execute(
        "SELECT id FROM tracking_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    if not row:
        raise HTTPException(status_code=404, detail="Session not found")

    suggestion = svc.quest_service.get_session_link_suggestion(session_id)
    return {
        "sessionId": session_id,
        "suggestionType": suggestion["suggestion_type"],
        "reason": suggestion["reason"],
        "questId": str(suggestion["quest_id"]) if suggestion["quest_id"] is not None else None,
        "questName": suggestion["quest_name"],
        "playlistId": str(suggestion["playlist_id"]) if suggestion["playlist_id"] is not None else None,
        "playlistName": suggestion["playlist_name"],
    }

@router.post("/session/{session_id}/quest-link")
def decide_session_quest_link(session_id: str, body: SessionQuestLinkDecisionBody):
    """Persist the curated quest analytics linkage decision for a session."""
    svc = get_services()
    row = svc.app_db.conn.execute(
        "SELECT id FROM tracking_sessions WHERE id = ?",
        (session_id,),
    ).fetchone()
    if not row:
        raise HTTPException(status_code=404, detail="Session not found")

    action = body.action.strip().lower()
    if action == "accept":
        try:
            suggestion = svc.quest_service.accept_session_link_suggestion(session_id)
        except ValueError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc
        return {
            "sessionId": session_id,
            "status": "linked",
            "linkType": suggestion["suggestion_type"],
            "questId": str(suggestion["quest_id"]) if suggestion["quest_id"] is not None else None,
            "questName": suggestion["quest_name"],
            "playlistId": str(suggestion["playlist_id"]) if suggestion["playlist_id"] is not None else None,
            "playlistName": suggestion["playlist_name"],
        }

    if action == "decline":
        svc.quest_service.decline_session_link(session_id)
        return {"sessionId": session_id, "status": "declined"}

    raise HTTPException(status_code=400, detail="Action must be 'accept' or 'decline'")
