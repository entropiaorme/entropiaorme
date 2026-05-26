"""HuntTracker scenario priming used by guide-mode demo playback.

This module ships in both dev and frozen builds. In frozen builds the
``/demo/*`` router imports ``prime_tracker`` here to populate the
in-memory parallel tracker that drives the guide's mid-hunt readout.

The env-var-driven path (``ENTROPIAORME_DEMO_SCENARIO``) that primed the
live tracker in dev is gated off in frozen builds at its call site in
``backend/tracking/tracker.py`` (``maybe_prime_tracker_from_env`` is only
invoked when ``sys.frozen`` is false), so setting the env var on an
installed build is a no-op. Only the deliberate demo-router path reaches
``prime_tracker`` in production, and that path operates on an in-memory
clone of the bundled demo DB.

Activation flow (dev only, via env var):
1. HuntTracker.__init__ calls ``maybe_prime_tracker_from_env``.
2. That call is wrapped in ``if not sys.frozen`` upstream, so it no-ops
   in frozen builds.
3. In dev: the env var is read; if set, dispatches to the matching
   priming handler.
4. If env var unset, scenario unknown, no handler, or import fails, the
   tracker is left alone.

Activation flow (production, via demo router):
1. ``backend/routers/demo.py`` builds a parallel ``HuntTracker`` pointed
   at an in-memory clone of the bundled demo DB.
2. It calls ``prime_tracker(tracker, "mid_hunt")`` directly on the
   parallel tracker.
3. The live tracker is never touched.

The ``mid_hunt`` handler writes a full synthetic active-session substrate
so the dashboard's stat bars, LootPulse widget, LootCompositionWidget,
and recent-events strip all render with believable values out of the
box. Other scenarios (``overlay_menu_open``, ``skill_scan_in_progress``)
live in different services (overlay-menu window state,
skill_scan_manual service); their priming is not in this module.
"""

from __future__ import annotations

import logging
import os
import random
import sqlite3
import time as _time
import uuid
from datetime import datetime
from typing import TYPE_CHECKING

from backend.scripts.demo_seed.canonical import LIVE_SCENARIOS
from backend.tracking.models import Kill, LootItem, ToolStats, TrackingSession

if TYPE_CHECKING:
    from backend.tracking.tracker import HuntTracker

log = logging.getLogger(__name__)

ENV_VAR = "ENTROPIAORME_DEMO_SCENARIO"


def maybe_prime_tracker_from_env(tracker: HuntTracker) -> bool:
    """If the env var is set, prime the tracker. Returns True if attempted."""
    scenario_name = os.environ.get(ENV_VAR, "").strip()
    if not scenario_name:
        return False
    return prime_tracker(tracker, scenario_name)


def prime_tracker(tracker: HuntTracker, scenario_name: str) -> bool:
    """Prime tracker in-memory state for ``scenario_name``."""
    scenario = next((s for s in LIVE_SCENARIOS if s.name == scenario_name), None)
    if scenario is None:
        log.warning(
            "Demo scenario %r not recognised — skipping priming.", scenario_name
        )
        return False

    handler = _SCENARIO_HANDLERS.get(scenario_name)
    if handler is None:
        log.warning(
            "Demo scenario %r has no priming handler (declared but not yet implemented).",
            scenario_name,
        )
        return False

    log.info("Priming HuntTracker for demo scenario %r.", scenario_name)
    try:
        handler(tracker, scenario.payload)
    except Exception:
        log.exception(
            "Demo scenario %r priming raised — leaving tracker as-is.", scenario_name
        )
        return False
    return True


# ─── Scenario handlers ───────────────────────────────────────────────────────

# Reproducible RNG for kill generation. Using a distinctive constant so the
# stream is deterministic across re-runs and obviously-not-real to anyone
# looking at the code.
_MID_HUNT_RNG_SEED = 0xDEC0_0DE5

# Engineered headline targets: the dashboard's stat bars, computed from the
# kill stream, land on these. Values were picked to be internally consistent:
#   100 kills x 1.52 PED/kill = 152 PED cycled
#   152 PED x 105.2% = 159.90 PED loot (rate)
#   PES 5.02 / 152 PED x 100 = 3.30 PES/100 (close to the 3.26 target)
#   46_360 weapon damage / 15_200 PEC = 3.05 DPP
_MID_HUNT_KILL_COUNT = 100
_MID_HUNT_AVG_COST_PED = 1.52
_MID_HUNT_TOTAL_COST_PED = _MID_HUNT_KILL_COUNT * _MID_HUNT_AVG_COST_PED  # 152.00
_MID_HUNT_TARGET_RATE = 1.052
_MID_HUNT_TOTAL_LOOT_PED = round(
    _MID_HUNT_TOTAL_COST_PED * _MID_HUNT_TARGET_RATE, 2
)  # 159.90
_MID_HUNT_TARGET_PES_PER_100 = (
    3.26  # → PES = realised_cost × 3.26 / 100, ~5.19 at cost 159.20
)
_MID_HUNT_TARGET_DPP = 3.05  # → total damage = realised_cost × 100 × DPP
_MID_HUNT_LAST_KILL_LOOT = 0.80
_MID_HUNT_HIGH_MULT_IDX = (
    47  # mid-session "global" kill carries the literal loot composition
)

# The Korss H400 demo weapon, used as the sole tool for this synthetic
# session. cost_per_shot in PED matches the canonical equipment library
# enrichment.
_KORSS_CPS_PED = 0.398

# Loot composition on the high-mult kill: the literal 8-item drop scripted
# for the global. The remaining 99 kills carry only Shrapnel matching their
# per-kill loot_total. Total composition value here is 8.90 PED, which is
# intentionally less than the session's full 159.90 PED loot; most of the
# loot on other kills is Shrapnel filler. The widget aggregates all
# kill_loot_items for the session, so the visible composition will be:
# (Shrapnel from 99 normal kills) + (these 8 items from the global kill).
_MID_HUNT_COMPOSITION: tuple[tuple[str, int, float], ...] = (
    # (item_name, quantity, value_ped)
    ("Shrapnel", 51_878, 5.18),
    ("Robot Filter", 77, 2.77),
    ("Animal Oil Residue", 48, 0.48),
    ("Animal Muscle Oil", 13, 0.39),
    ("Bone", 1, 0.03),
    ("Jagged Tooth", 1, 0.02),
    ("Lesser Claw", 1, 0.02),
    ("Diluted Sweat", 1, 0.01),
)
_MID_HUNT_COMPOSITION_TOTAL = round(
    sum(v for _, _, v in _MID_HUNT_COMPOSITION), 2
)  # 8.90 PED

# A small pool of skill names to spread the PES gains across. Picked from
# canonical.SKILL_NAMES (Ranged + general combat + survival) so the
# Character > Skills tab shows believable per-skill activity if anyone
# queries skill_gains by name.
_MID_HUNT_SKILL_POOL: tuple[str, ...] = (
    "Hit Ability",
    "Damage Ability",
    "Combat Reflexes",
    "Combat Sense",
    "Ranged Laser (Hit)",
    "Ranged Laser (Dmg)",
    "Aim",
    "Anatomy",
    "Inflict Ranged Damage",
    "Wounded",
    "Evade",
    "Athletics",
)

# Recent-events synthetic global. The displayed value (60.5 PED) is
# intentionally decoupled from the kill's actual loot and the session
# totals so the recent-events strip shows a chunky global irrespective
# of the session math.
_MID_HUNT_GLOBAL_VALUE_PED = 60.5


def _build_kill_stream(
    elapsed_seconds: float,
    started_at_epoch: float,
    rng: random.Random,
) -> list[Kill]:
    """Generate 100 deterministic kills hitting the headline targets."""
    n = _MID_HUNT_KILL_COUNT

    # Per-kill shots: target the headline avg cost / Korss CPS, then
    # quantise to integer (kills schema requires integer shots). Realised
    # cost becomes shots x cps and drifts slightly from the planned 1.52 x
    # n_kills (~4-5% high because of rounding up to 4 shots when 3.82 is
    # the unconstrained value). Loot, damage, and PES then derive from the
    # *realised* cost so the headline-derived stats (rate, DPP, PES/100)
    # land exactly on the headline targets, at the cost of avg cost/kill
    # displaying as ~1.59 instead of exactly 1.52.
    raw_shot_targets = [
        _MID_HUNT_AVG_COST_PED * rng.uniform(0.92, 1.08) / _KORSS_CPS_PED
        for _ in range(n)
    ]
    per_kill_shots = [max(1, round(s)) for s in raw_shot_targets]
    per_kill_cost = [round(s * _KORSS_CPS_PED, 4) for s in per_kill_shots]
    realised_total_cost = sum(per_kill_cost)

    # Loot total scales to keep rate at exactly _MID_HUNT_TARGET_RATE
    # against the realised cost. Last kill = 0.80, high-mult kill = the
    # composition total (8.90); the remaining 98 kills absorb the rest.
    realised_total_loot = round(realised_total_cost * _MID_HUNT_TARGET_RATE, 2)
    realised_total_damage = round(realised_total_cost * 100 * _MID_HUNT_TARGET_DPP, 1)

    # Per-kill damage: distribute proportionally to cost.
    per_kill_damage = [
        round(realised_total_damage * (c / realised_total_cost), 1)
        for c in per_kill_cost
    ]

    other_loot_total = (
        realised_total_loot - _MID_HUNT_COMPOSITION_TOTAL - _MID_HUNT_LAST_KILL_LOOT
    )
    other_indices = [i for i in range(n) if i != _MID_HUNT_HIGH_MULT_IDX and i != n - 1]
    raw_loot = []
    for _ in other_indices:
        # Mix of loss kills (~55%) and win kills (~45%) for visual chop.
        if rng.random() < 0.55:
            raw_loot.append(rng.uniform(0.40, 1.30))
        else:
            raw_loot.append(rng.uniform(1.40, 3.00))
    loot_scale = other_loot_total / sum(raw_loot)
    other_loot_scaled = [round(v * loot_scale, 4) for v in raw_loot]

    per_kill_loot = [0.0] * n
    for idx, value in zip(other_indices, other_loot_scaled, strict=True):
        per_kill_loot[idx] = value
    per_kill_loot[_MID_HUNT_HIGH_MULT_IDX] = _MID_HUNT_COMPOSITION_TOTAL
    per_kill_loot[n - 1] = _MID_HUNT_LAST_KILL_LOOT

    # Per-kill mob: Caboria Old dominates (~70%) so dominant_mob/dominant_weapon
    # in any aggregation lands cleanly. Sprinkle Mature/Young for variety.
    def _sample_mob() -> tuple[str, str, str]:
        roll = rng.random()
        if roll < 0.70:
            return ("Caboria Old", "Caboria", "Old")
        if roll < 0.90:
            return ("Caboria Mature", "Caboria", "Mature")
        return ("Caboria Young", "Caboria", "Young")

    # The high-mult kill sticks to "Caboria Old" so the recent-events global
    # has a stable mob name.
    mobs = [_sample_mob() for _ in range(n)]
    mobs[_MID_HUNT_HIGH_MULT_IDX] = ("Caboria Old", "Caboria", "Old")

    # Build kills with timestamps spread evenly across the elapsed window.
    kills: list[Kill] = []
    slot_seconds = elapsed_seconds / n
    for i in range(n):
        kill_id = str(uuid.uuid4())
        timestamp = started_at_epoch + (i + 0.5) * slot_seconds
        cost_ped = per_kill_cost[i]
        loot_total = per_kill_loot[i]
        damage = per_kill_damage[i]
        shots = per_kill_shots[i]
        crits = max(0, round(shots * 0.05))
        damage_taken = round(damage * 0.06, 2)
        mob_name, species, maturity = mobs[i]
        is_global = i == _MID_HUNT_HIGH_MULT_IDX

        tool_stats = {
            "Korss H400": ToolStats(
                tool_name="Korss H400",
                shots_fired=shots,
                damage_dealt=damage,
                critical_hits=crits,
                cost_per_shot=_KORSS_CPS_PED,
            )
        }

        kills.append(
            Kill(
                id=kill_id,
                session_id="",  # caller fills in
                mob_name=mob_name,
                mob_species=species,
                mob_maturity=maturity,
                timestamp=timestamp,
                shots_fired=shots,
                damage_dealt=damage,
                damage_taken=damage_taken,
                critical_hits=crits,
                cost_ped=cost_ped,
                enhancer_cost=0.0,
                loot_total_ped=loot_total,
                is_global=is_global,
                is_hof=False,
                tool_stats=tool_stats,
            )
        )

    return kills


def _write_demo_session_to_db(
    db: sqlite3.Connection,
    session_id: str,
    started_at_epoch: float,
    elapsed_seconds: float,
    kills: list[Kill],
    rng: random.Random,
) -> None:
    """Mirror the in-memory session into DB rows.

    Required by the LootCompositionWidget (queries `/tracking/session/<id>`),
    the PES stat (queries `skill_gains` by session_id), and the
    recent-events strip (queries `notable_events` by session_id).
    """
    db.execute(
        "INSERT INTO tracking_sessions "
        "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
        "VALUES (?, ?, NULL, 0, 0.0, 0.0, 0.0)",
        (session_id, started_at_epoch),
    )

    for kill in kills:
        db.execute(
            "INSERT INTO kills "
            "(id, session_id, mob_name, mob_species, mob_maturity, timestamp, "
            "shots_fired, damage_dealt, damage_taken, critical_hits, "
            "cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                kill.id,
                session_id,
                kill.mob_name,
                kill.mob_species,
                kill.mob_maturity,
                kill.timestamp,
                kill.shots_fired,
                round(kill.damage_dealt, 2),
                kill.damage_taken,
                kill.critical_hits,
                round(kill.cost_ped, 4),
                kill.enhancer_cost,
                round(kill.loot_total_ped, 4),
                1 if kill.is_global else 0,
                1 if kill.is_hof else 0,
            ),
        )
        for tool_name, ts in kill.tool_stats.items():
            db.execute(
                "INSERT INTO kill_tool_stats "
                "(kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (
                    kill.id,
                    tool_name,
                    ts.shots_fired,
                    round(ts.damage_dealt, 2),
                    ts.critical_hits,
                    ts.cost_per_shot,
                ),
            )

    # Loot composition: only the literal 8 items on the high-mult kill, no
    # per-kill Shrapnel filler. The other 99 kills carry no kill_loot_items
    # rows; their loot_total_ped lives only on the kills row, which drives
    # the headline Loot TT. The composition widget reads kill_loot_items
    # by session_id and shows per-segment shares with no aggregate-total
    # footer, so dropping the filler lets the scripted 58.2/31.1/...
    # percentages render exactly while the headline Loot TT still reflects
    # the full per-kill loot stream. The implicit consequence is that the
    # composition widget's segment values sum to 8.90 PED while the
    # headline shows ~167 PED; there's no surface that exposes both totals
    # in a comparable position, so the visible inconsistency is silent.
    high_mult_kill = kills[_MID_HUNT_HIGH_MULT_IDX]
    for item_name, qty, value in _MID_HUNT_COMPOSITION:
        db.execute(
            "INSERT INTO kill_loot_items "
            "(kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
            "VALUES (?, ?, ?, ?, 0)",
            (high_mult_kill.id, item_name, qty, value),
        )
        high_mult_kill.loot_items.append(
            LootItem(item_name=item_name, quantity=qty, value_ped=value)
        )

    # PES skill gains spread across the session. Total ped_value scales
    # with realised cost so the PES/100 derived stat lands at exactly
    # _MID_HUNT_TARGET_PES_PER_100 (3.26). amount/ped_value ratio mirrors
    # the ~0.45 from the canonical sessions seeder so the per-skill
    # records look familiar to the rest of the demo.
    realised_total_cost = sum(k.cost_ped for k in kills)
    target_pes_total = round(
        realised_total_cost * _MID_HUNT_TARGET_PES_PER_100 / 100, 4
    )
    n_gains = 12
    raw_pes = [rng.uniform(0.20, 0.70) for _ in range(n_gains)]
    pes_scale = target_pes_total / sum(raw_pes)
    pes_values = [round(v * pes_scale, 4) for v in raw_pes]
    skill_window = _MID_HUNT_SKILL_POOL[:n_gains]
    for i, (skill_name, ped_value) in enumerate(
        zip(skill_window, pes_values, strict=True)
    ):
        gain_ts = started_at_epoch + (i + 0.5) * (elapsed_seconds / n_gains)
        amount = round(ped_value * rng.uniform(2.0, 2.4), 5)
        db.execute(
            "INSERT INTO skill_gains "
            "(session_id, timestamp, skill_name, amount, ped_value) "
            "VALUES (?, ?, ?, ?, ?)",
            (session_id, gain_ts, skill_name, amount, ped_value),
        )

    # Recent-events synthetic global. Value (60.5 PED) is deliberately
    # decoupled from the kill's actual loot so the events strip shows a
    # chunky global irrespective of the session math.
    event_ts = started_at_epoch + (elapsed_seconds * 0.55)
    db.execute(
        "INSERT INTO notable_events "
        "(session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (
            session_id,
            high_mult_kill.id,
            "global_kill",
            high_mult_kill.mob_name,
            _MID_HUNT_GLOBAL_VALUE_PED,
            event_ts,
        ),
    )

    db.commit()


def _prime_mid_hunt(tracker: HuntTracker, payload: dict) -> None:
    """Mid-hunt scenario: synthesise a fully populated active session.

    Builds an in-memory ``TrackingSession`` with 100 kills hitting the
    scripted headline targets (rate 105.2%, last loot 0.80 PED, avg cost
    1.52 PED, PES 5.02, DPP 3.05, max mult ~5.85x on the global kill),
    mirrors the session into DB rows so the LootCompositionWidget,
    PES query, and recent-events strip render coherently, and finally
    points the tracker at the new session.
    """
    if tracker.is_tracking:
        log.warning("Tracker already in session; refusing to prime mid_hunt over it.")
        return

    rng = random.Random(_MID_HUNT_RNG_SEED)
    elapsed_seconds = float(payload.get("elapsed_seconds", 754) or 754)
    started_at_epoch = _time.time() - elapsed_seconds

    session_id = str(uuid.uuid4())
    kills = _build_kill_stream(elapsed_seconds, started_at_epoch, rng)

    # Stamp the session_id into each kill before any DB write.
    for kill in kills:
        kill.session_id = session_id

    _write_demo_session_to_db(
        tracker._db,
        session_id,
        started_at_epoch,
        elapsed_seconds,
        kills,
        rng,
    )

    # Build the in-memory session and assign to tracker. Setting the
    # private fields directly bypasses HuntTracker's session-start
    # lifecycle, which is what we want for demo mode — the listener
    # subsystems aren't running, but every read-only consumer of the
    # tracker's state surface (status endpoint, dashboard widgets,
    # overlay, recent-events strip) sees a coherent active session.
    session = TrackingSession(
        id=session_id,
        start_time=datetime.fromtimestamp(started_at_epoch),
        kills=kills,
    )
    tracker._session = session
    tracker._session_heal_cost = 0.0
    tracker._session_warnings = []
    tracker._confirmed_mob_name = "Caboria Old"
    tracker._confirmed_mob_species = "Caboria"
    tracker._confirmed_mob_maturity = "Old"
    tracker._mob_source = "manual"
    tracker._session_mob_tracking_mode = "mob"
    tracker._session_mob_tracking_tag = ""
    tracker._accumulator = None
    tracker._last_kill = kills[-1]

    realised_cost = sum(k.cost_ped for k in kills)
    realised_loot = sum(k.loot_total_ped for k in kills)
    realised_damage = sum(
        ts.damage_dealt for k in kills for ts in k.tool_stats.values()
    )
    log.info(
        "Demo mid_hunt primed: %d kills, total cost %.2f PED, total loot %.2f PED, "
        "rate %.1f%%, last loot %.2f PED, max mult %.2fx, target PES/100 %.2f, DPP %.2f.",
        len(kills),
        realised_cost,
        realised_loot,
        100.0 * realised_loot / max(realised_cost, 1e-9),
        kills[-1].loot_total_ped,
        max((k.loot_total_ped / k.cost_ped) for k in kills if k.cost_ped > 0),
        _MID_HUNT_TARGET_PES_PER_100,
        realised_damage / max(realised_cost * 100, 1e-9),
    )


_SCENARIO_HANDLERS = {
    "mid_hunt": _prime_mid_hunt,
    # `overlay_menu_open` lives in the overlay-menu Tauri window state,
    # not HuntTracker — its priming is frontend-side and doesn't go
    # through this module. Declared in canonical.LIVE_SCENARIOS for
    # completeness; no handler here.
    #
    # `skill_scan_in_progress` lives in skill_scan_manual service state.
    # Same situation.
}
