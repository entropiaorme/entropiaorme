"""Sessions-domain seeder — full 90-day session corpus.

Writes tracking_sessions + kills + kill_loot_items + kill_tool_stats +
skill_gains + notable_events + materialised session_summaries against the
canonical mobs / items / skill_names from CoreSeeder. Drives Dashboard,
Analytics (Overview / Activity / Sessions list), and Character > Prospect.
"""

from __future__ import annotations

import logging
import random
import sqlite3
import uuid
from pathlib import Path

from backend.scripts.demo_seed.contract import CanonicalRefs
from backend.services.session_summary import write_session_summary

log = logging.getLogger(__name__)

# Distinctive seed so the sessions stream stays deterministic across re-runs
# and doesn't correlate with any sibling per-domain seeder's RNG. The chosen
# value (0x5E55_1044) produces a balanced 4/12/5/2/2 weapon distribution
# across the 25-session corpus, covering all five canonical weapons.
_RNG_SEED = 0x5E55_1044
_DAY = 86400.0
_HOUR = 3600.0


# ─── Tunables ────────────────────────────────────────────────────────────────

# Plausible cost-per-shot baseline by canonical weapon name. Used both for
# kills.cost_ped (= cps × shots) and kill_tool_stats.cost_per_shot.
_COST_PER_SHOT: dict[str, float] = {
    "Emik Enigma L1 (L)": 0.052,
    "Korss H400": 0.398,
    "Herman CAP-7 Jungle (L)": 0.151,
    "Jester D-1": 0.043,
    "Castorian Pioneer EnBlade-2 (L)": 0.198,
}

# Approx shots-to-kill ranges for the canonical mob species + maturity combos
# the corpus uses, scaled to the dominant weapon for the session profile.
_SHOTS_TO_KILL: dict[tuple[str, str], tuple[int, int]] = {
    ("Caboria", "Young"): (10, 18),
    ("Caboria", "Mature"): (14, 22),
    ("Caboria", "Old"): (20, 32),
    ("Caboria", "Provider"): (28, 42),
    ("Atrox", "Young"): (16, 24),
    ("Atrox", "Mature"): (22, 32),
    ("Atrox", "Old"): (24, 38),
    ("Atrox", "Stalker"): (32, 48),
    ("Argonaut", "Young"): (12, 20),
    ("Argonaut", "Mature"): (18, 28),
    ("Argonaut", "Old"): (22, 34),
    ("Argonaut", "Provider"): (28, 42),
    ("Combibo", "Young"): (8, 14),
    ("Combibo", "Mature"): (10, 16),
    ("Combibo", "Old"): (14, 20),
    ("Daikiba", "Young"): (10, 18),
    ("Daikiba", "Mature"): (14, 22),
    ("Daikiba", "Old"): (18, 28),
    ("Daikiba", "Provider"): (24, 36),
    ("Snablesnot Male", "Young"): (10, 16),
    ("Snablesnot Male", "Old"): (16, 24),
    ("Snablesnot Male", "Provider"): (22, 32),
}

# Combat skill pools (subsets of canonical SKILL_NAMES).
_RANGED_SKILLS = (
    "Hit Ability",
    "Damage Ability",
    "Combat Reflexes",
    "Combat Sense",
    "Ranged Laser (Hit)",
    "Ranged Laser (Dmg)",
    "Ranged Blp (Hit)",
    "Ranged Blp (Dmg)",
    "Aim",
    "Anatomy",
    "Inflict Ranged Damage",
)
_MELEE_SKILLS = (
    "Hit Ability",
    "Damage Ability",
    "Combat Reflexes",
    "Melee Combat (Hit)",
    "Melee Combat (Dmg)",
    "Power Catch",
    "Lightweight Melee Weapons",
    "Inflict Melee Damage",
)
_SUPPORT_SKILLS = ("Evade", "Dodge", "Athletics", "Wounded", "Serendipity")

# Attribute names that double as skill_gains rows (so session_summary's
# attribute_levels_json comes out non-empty). These belong to refs.attribute_names
# rather than refs.skill_names, but the skill_gains table is unconstrained.
_ATTRIBUTES_FAVOURED = ("Stamina", "Strength", "Agility")

# Plausible loot item table (real EU lore items, public game data). Ordering
# weighted to match the real-EU drop frequency profile: animal residues / oils
# / hides dominate; creature parts (bones, claws, teeth, fangs) and stackable
# stones / nexus components fill the long tail.
_NAMED_LOOT_ITEMS: tuple[str, ...] = (
    # bulk creature-residue drops (top of any real animal-loot histogram)
    "Animal Oil Residue",
    "Animal Oil Residue",
    "Animal Oil Residue",
    "Animal Muscle Oil",
    "Animal Muscle Oil",
    "Animal Hide",
    "Animal Hide",
    # creature-part drops (bones / claws / teeth — Argonaut/Caboria/Daikiba style)
    "Bone",
    "Soft Hide",
    "Fine Hide",
    "Jagged Tooth",
    "Lesser Claw",
    "Argonaut Bone",
    "Argonaut Claw Small",
    # less-common oils + stackables
    "Animal Eye Oil",
    "Animal Liver Oil",
    "Animal Thyroid Oil",
    "Animal Adrenal Oil",
    "Wool",
    "Lysterium Stone",
    "Belkar Stone",
    "Diluted Sweat",
    "Paint Can (Olive)",
    "Socket 1 Component",
)
_RARE_LOOT_ITEMS: tuple[str, ...] = (
    "Dunkel Element",
    "Robust Oil",
    "Ares Component",
    "Nexus",
    "Kerm Stone",
    "Force Nexus",
    "Easter Strongbox",
    "Mayhem Token",
)

# Session profiles: (weapon, dominant_species, dominant_maturity, mode).
# Mixed mob choice within a session weights toward this dominant pair so the
# session_summary dominance check (>= 60%) reliably populates dominant_mob
# and dominant_weapon.
_PROFILES: tuple[tuple[str, str, str, str], ...] = (
    ("Emik Enigma L1 (L)", "Caboria", "Young", "ranged"),
    ("Emik Enigma L1 (L)", "Caboria", "Mature", "ranged"),
    ("Emik Enigma L1 (L)", "Combibo", "Mature", "ranged"),
    ("Korss H400", "Caboria", "Old", "ranged"),
    ("Korss H400", "Caboria", "Provider", "ranged"),
    ("Korss H400", "Argonaut", "Mature", "ranged"),
    ("Korss H400", "Argonaut", "Old", "ranged"),
    # Big-mob ranged work runs on Korss H400 (Hedoc Mayhem is a heal tool here).
    ("Korss H400", "Atrox", "Old", "ranged"),
    ("Korss H400", "Atrox", "Stalker", "ranged"),
    ("Korss H400", "Argonaut", "Provider", "ranged"),
    ("Herman CAP-7 Jungle (L)", "Daikiba", "Old", "ranged"),
    ("Herman CAP-7 Jungle (L)", "Daikiba", "Provider", "ranged"),
    ("Herman CAP-7 Jungle (L)", "Snablesnot Male", "Old", "ranged"),
    ("Jester D-1", "Combibo", "Mature", "ranged"),
    ("Jester D-1", "Combibo", "Old", "ranged"),
    ("Castorian Pioneer EnBlade-2 (L)", "Snablesnot Male", "Old", "melee"),
    ("Castorian Pioneer EnBlade-2 (L)", "Combibo", "Old", "melee"),
)

# 25 sessions, distributed:
#   first 30d: 6 (career establishment)
#   middle 30d: 9 (peak engagement)
#   last 30d: 10 (current activity)
_SESSION_COUNTS_BY_PERIOD: tuple[int, ...] = (6, 9, 10)


class SessionsSeeder:
    name: str = "sessions"
    depends_on: tuple[str, ...] = ("core",)

    def seed(self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path) -> None:
        rng = random.Random(_RNG_SEED)

        # Build the full session corpus in-memory first so cross-row coherence
        # (notable-event placement, total counts) is easy to reason about.
        sessions = self._build_corpus(refs, rng)

        # Pre-pick which sessions carry notable events. 1 HoF + 2 globals + 1
        # multiplier = 4 sessions. Spread them across the timeline.
        n = len(sessions)
        notable_pool = sorted(rng.sample(range(n), 4))
        notable_assignments: dict[int, tuple[str, float]] = {
            notable_pool[0]: ("global_kill", rng.uniform(55.0, 90.0)),
            notable_pool[1]: ("hof_kill", rng.uniform(380.0, 720.0)),
            notable_pool[2]: ("global_item", rng.uniform(60.0, 95.0)),
            notable_pool[3]: ("multiplier", rng.uniform(22.0, 55.0)),
        }

        kill_total = 0
        loot_row_total = 0
        skill_gain_total = 0
        notable_total = 0

        for idx, sess in enumerate(sessions):
            kills = self._generate_kills(sess, rng, refs.mobs)
            if not kills:
                # Should never happen but guard so we don't write empty sessions.
                continue

            # Per-session loot return distribution. Long-run avg loot/cost in
            # EU sits below 1.0 (the platform's house edge), so the typical
            # band centres on ~0.95 (slight chip-down) rather than ~1.0.
            # Loss / break-even / gain trichotomy drives chart variance;
            # HoF/global sessions bump separately on the bearer kill below.
            roll = rng.random()
            if roll < 0.65:
                session_return = rng.uniform(
                    0.78, 1.10
                )  # typical (chip-down + small wins)
            elif roll < 0.85:
                session_return = rng.uniform(0.55, 0.82)  # loss session
            else:
                session_return = rng.uniform(1.10, 1.35)  # gain session

            notable = notable_assignments.get(idx)
            notable_kill_idx = rng.randrange(len(kills)) if notable else None
            notable_value = notable[1] if notable else 0.0

            # Distribute loot per-kill: each kill gets cost × per-kill multiplier
            # sampled around session_return; the notable kill bears its full event
            # value on top.
            total_cost = sum(k["cost_ped"] for k in kills)
            for k_i, k in enumerate(kills):
                noise = rng.uniform(0.55, 1.55)
                loot_val = k["cost_ped"] * session_return * noise
                if k_i == notable_kill_idx:
                    loot_val += notable_value
                    if notable[0] == "hof_kill":
                        k["is_hof"] = True
                        k["is_global"] = True
                    elif notable[0] in ("global_kill", "global_item"):
                        k["is_global"] = True
                # Per-kill enhancer cost (small fraction of weapon cost).
                k["enhancer_cost"] = round(k["cost_ped"] * rng.uniform(0.0, 0.06), 4)
                k["loot_total_ped"] = round(max(0.0, loot_val), 4)

            # Per-session aux costs. Real-DB shape: armour/heal cost as a share
            # of weapon cost is generally low for typical-mob hunting; spikes
            # up on tank-heavy sessions. Polished bands sit lower than the
            # initial pass and add a 1-in-6 spike chance for high-armour mobs.
            spike = rng.random() < 0.18
            armour_lo, armour_hi = (0.06, 0.14) if spike else (0.015, 0.07)
            heal_lo, heal_hi = (0.04, 0.10) if spike else (0.012, 0.05)
            armour_cost = round(total_cost * rng.uniform(armour_lo, armour_hi), 4)
            heal_cost = round(total_cost * rng.uniform(heal_lo, heal_hi), 4)
            dangling_cost = round(rng.uniform(0.0, 1.2), 4)

            # Insert tracking_sessions row (is_active=0 to avoid HuntTracker
            # orphan-recovery muting these on backend startup).
            db.execute(
                "INSERT INTO tracking_sessions "
                "(id, started_at, ended_at, is_active, armour_cost, heal_cost, dangling_cost) "
                "VALUES (?, ?, ?, 0, ?, ?, ?)",
                (
                    sess["id"],
                    sess["started_at"],
                    sess["ended_at"],
                    armour_cost,
                    heal_cost,
                    dangling_cost,
                ),
            )

            # Insert kills + tool_stats + loot rows.
            for k in kills:
                db.execute(
                    "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, "
                    "timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, "
                    "cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        k["id"],
                        k["session_id"],
                        k["mob_name"],
                        k["mob_species"],
                        k["mob_maturity"],
                        k["timestamp"],
                        k["shots_fired"],
                        round(k["damage_dealt"], 2),
                        round(k["damage_taken"], 2),
                        k["critical_hits"],
                        k["cost_ped"],
                        k["enhancer_cost"],
                        k["loot_total_ped"],
                        1 if k.get("is_global") else 0,
                        1 if k.get("is_hof") else 0,
                    ),
                )
                db.execute(
                    "INSERT INTO kill_tool_stats "
                    "(kill_id, tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot) "
                    "VALUES (?, ?, ?, ?, ?, ?)",
                    (
                        k["id"],
                        k["weapon"],
                        k["shots_fired"],
                        round(k["damage_dealt"], 2),
                        k["critical_hits"],
                        k["cps"],
                    ),
                )
                # Loot rows: a dominant Shrapnel filler + 0-3 named items.
                self._write_loot_rows(db, k, rng)
                loot_row_total += k["_loot_row_count"]
                kill_total += 1

            # Skill gains: 8-14 rows per session, mixed combat + occasional
            # support + 1-3 attribute rows so attribute_levels_json populates.
            sg_count = self._write_skill_gains(db, sess, kills, rng)
            skill_gain_total += sg_count

            # Notable event row (if any).
            if notable:
                evt_type, evt_val = notable
                target_kill = kills[notable_kill_idx]
                mob_or_item = target_kill["mob_name"]
                if evt_type == "global_item":
                    mob_or_item = rng.choice(_RARE_LOOT_ITEMS)
                db.execute(
                    "INSERT INTO notable_events "
                    "(session_id, kill_id, event_type, mob_or_item, value_ped, timestamp) "
                    "VALUES (?, ?, ?, ?, ?, ?)",
                    (
                        sess["id"],
                        target_kill["id"],
                        evt_type,
                        mob_or_item,
                        round(evt_val, 2),
                        target_kill["timestamp"],
                    ),
                )
                notable_total += 1

            # Materialise the per-session summary row using the canonical helper.
            write_session_summary(db, sess["id"])

        log.info(
            "%s seeder: %d sessions, %d kills, %d loot rows, %d skill_gains, %d notable_events",
            self.name,
            len(sessions),
            kill_total,
            loot_row_total,
            skill_gain_total,
            notable_total,
        )

    # ─── helpers ────────────────────────────────────────────────────────────

    def _build_corpus(self, refs: CanonicalRefs, rng: random.Random) -> list[dict]:
        """Build the in-memory session list spread across the 90-day window."""
        now = refs.timeline.demo_now
        window_start = now - 90 * _DAY
        sessions: list[dict] = []
        for period_idx, count in enumerate(_SESSION_COUNTS_BY_PERIOD):
            period_start = window_start + period_idx * 30 * _DAY
            for _ in range(count):
                # Pick a daytime offset within the period; clamp so the longest
                # session still ends inside the period bounds.
                start_offset = rng.uniform(0, 30 * _DAY - 4.5 * _HOUR)
                started_at = period_start + start_offset
                # Duration distribution: 15% short, 70% typical, 15% long.
                roll = rng.random()
                if roll < 0.15:
                    duration_h = rng.uniform(0.5, 1.0)
                elif roll < 0.85:
                    duration_h = rng.uniform(1.0, 2.5)
                else:
                    duration_h = rng.uniform(2.6, 4.0)
                ended_at = started_at + duration_h * _HOUR
                profile = rng.choice(_PROFILES)
                sessions.append(
                    {
                        "id": str(uuid.uuid4()),
                        "started_at": started_at,
                        "ended_at": ended_at,
                        "duration_hours": duration_h,
                        "profile": profile,
                    }
                )
        sessions.sort(key=lambda s: s["started_at"])
        return sessions

    def _generate_kills(
        self,
        sess: dict,
        rng: random.Random,
        mobs: tuple,
    ) -> list[dict]:
        """Generate the per-kill list for one session.

        Kill count is driven directly (not budget-scaled) so the total stays
        in the 200-300 target band regardless of weapon tier mix. 85% of kills
        hit the dominant species + maturity so summary dominance survives.
        """
        weapon, dom_species, dom_maturity, _mode = sess["profile"]
        cps = _COST_PER_SHOT[weapon]
        duration_h = sess["duration_hours"]
        # Target ~5 kills/hour baseline + small random jitter; clamp to 4..18
        # so even long sessions don't blow the global budget. 25 sessions ×
        # ~10 mean kills lands ~250 total kills (squarely in the 200-300 band).
        target = int(round(duration_h * rng.uniform(4.0, 7.0)))
        target = max(4, min(target, 18))

        species_by_name = {m.species: m for m in mobs}
        dom_obj = species_by_name.get(dom_species)
        other_species_pool = [m for m in mobs if m.species != dom_species]

        kills: list[dict] = []
        # Even time spacing so the in-session histogram looks plausible.
        slot = 0
        while len(kills) < target:
            if rng.random() < 0.85:
                species, maturity = dom_species, dom_maturity
            else:
                if rng.random() < 0.65 and dom_obj:
                    other_mats = [m for m in dom_obj.maturities if m != dom_maturity]
                    if other_mats:
                        species = dom_species
                        maturity = rng.choice(other_mats)
                    else:
                        species = dom_species
                        maturity = dom_maturity
                else:
                    other = (
                        rng.choice(other_species_pool)
                        if other_species_pool
                        else dom_obj
                    )
                    species = other.species
                    maturity = rng.choice(other.maturities)

            s2k = _SHOTS_TO_KILL.get(
                (species, maturity),
                (15, 25),
            )
            shots = rng.randint(*s2k)
            crits = rng.randint(0, max(1, shots // 10))
            damage = shots * rng.uniform(8.0, 22.0)
            damage_taken = max(0.0, rng.gauss(damage * 0.08, damage * 0.04))
            cost = round(shots * cps, 4)

            # Even-ish time placement across the session window.
            slot += 1
            kills.append(
                {
                    "id": str(uuid.uuid4()),
                    "session_id": sess["id"],
                    "mob_name": f"{species} {maturity}",
                    "mob_species": species,
                    "mob_maturity": maturity,
                    "timestamp": 0.0,  # filled below once total count known
                    "shots_fired": shots,
                    "damage_dealt": damage,
                    "damage_taken": damage_taken,
                    "critical_hits": crits,
                    "cost_ped": cost,
                    "enhancer_cost": 0.0,  # filled in seed() once loot is rolled
                    "loot_total_ped": 0.0,  # filled in seed()
                    "is_global": False,
                    "is_hof": False,
                    "weapon": weapon,
                    "cps": cps,
                }
            )

        # Stamp timestamps evenly across the session window (small jitter).
        if kills:
            window = sess["ended_at"] - sess["started_at"]
            step = window / (len(kills) + 1)
            for i, k in enumerate(kills):
                jitter = rng.uniform(-step * 0.3, step * 0.3)
                k["timestamp"] = sess["started_at"] + step * (i + 1) + jitter
        return kills

    def _write_loot_rows(
        self,
        db: sqlite3.Connection,
        kill: dict,
        rng: random.Random,
    ) -> None:
        """Write 1-4 kill_loot_items rows for one kill, splitting loot_total_ped."""
        loot_total = kill["loot_total_ped"]
        rows: list[tuple[str, int, float, int]] = []
        if loot_total <= 0:
            # Empty-loot kill: still write a 0-PED Shrapnel row so loot_items
            # joins downstream don't see an entirely empty kill.
            rows.append(("Shrapnel", 1, 0.0, 1))
        else:
            # Shrapnel takes the bulk; named items get a small slice each.
            # Shrapnel quantity is value × 10000 (1 piece = 0.0001 PED in EU);
            # the initial pass used × 1000, off by a factor of 10.
            shrap_share = rng.uniform(0.55, 0.85)
            shrap_value = round(loot_total * shrap_share, 4)
            remaining = max(0.0, loot_total - shrap_value)
            rows.append(("Shrapnel", int(round(shrap_value * 10000)), shrap_value, 1))
            # 0-3 named drops sharing the rest. Real-DB row count averages
            # 1.6 per kill — bias toward 0-1 named items, occasional 2-3.
            n_named = rng.choices((0, 1, 2, 3), weights=(4, 5, 2, 1))[0]
            if n_named > 0 and remaining > 0:
                slices = [rng.random() for _ in range(n_named)]
                slices_sum = sum(slices) or 1.0
                for slice_w in slices:
                    portion = round(remaining * (slice_w / slices_sum), 4)
                    if portion <= 0:
                        continue
                    # Rare drop: 5% chance to pick from the rare pool.
                    if rng.random() < 0.05:
                        item_name = rng.choice(_RARE_LOOT_ITEMS)
                    else:
                        item_name = rng.choice(_NAMED_LOOT_ITEMS)
                    rows.append((item_name, 1, portion, 0))
        for item_name, qty, value, is_shrap in rows:
            db.execute(
                "INSERT INTO kill_loot_items "
                "(kill_id, item_name, quantity, value_ped, is_enhancer_shrapnel) "
                "VALUES (?, ?, ?, ?, ?)",
                (kill["id"], item_name, qty, value, is_shrap),
            )
        kill["_loot_row_count"] = len(rows)

    def _write_skill_gains(
        self,
        db: sqlite3.Connection,
        sess: dict,
        kills: list[dict],
        rng: random.Random,
    ) -> int:
        """Write 8-14 skill_gains rows per session: combat + support + attribute.

        Combat skills carry ped_value (regular_skill_tt feed); attribute rows
        carry amount only (attribute_levels feed via summary helper).
        """
        weapon, _dom_species, _dom_maturity, mode = sess["profile"]
        combat_pool = _RANGED_SKILLS if mode == "ranged" else _MELEE_SKILLS

        # 5-9 combat rows + 1-3 support rows + 1-3 attribute rows.
        n_combat = rng.randint(5, 9)
        n_support = rng.randint(1, 3)
        n_attr = rng.randint(1, 3)

        chosen_combat = rng.sample(combat_pool, k=min(n_combat, len(combat_pool)))
        chosen_support = rng.sample(
            _SUPPORT_SKILLS, k=min(n_support, len(_SUPPORT_SKILLS))
        )
        chosen_attrs = rng.sample(
            _ATTRIBUTES_FAVOURED, k=min(n_attr, len(_ATTRIBUTES_FAVOURED))
        )

        # Time-stamp gains across the session window so analytics charts
        # don't see all gains stacked at start/end.
        window = max(1.0, sess["ended_at"] - sess["started_at"])
        rows_written = 0
        all_gains = (
            [(s, "combat") for s in chosen_combat]
            + [(s, "support") for s in chosen_support]
            + [(s, "attr") for s in chosen_attrs]
        )
        rng.shuffle(all_gains)
        for i, (skill_name, kind) in enumerate(all_gains):
            ts = sess["started_at"] + window * ((i + 0.5) / len(all_gains))
            ts += rng.uniform(-window * 0.05, window * 0.05)
            if kind == "attr":
                # Attribute gains: small fractional amount, ped_value NULL.
                amount = round(rng.uniform(0.001, 0.012), 5)
                db.execute(
                    "INSERT INTO skill_gains "
                    "(session_id, timestamp, skill_name, amount, ped_value) "
                    "VALUES (?, ?, ?, ?, NULL)",
                    (sess["id"], ts, skill_name, amount),
                )
            else:
                # Regular skills: amount distribution centres around ~0.09
                # per gain event with occasional level-burst gains up to
                # ~0.4. ped_value/amount ratio centres on ~0.45.
                burst = rng.random() < 0.10
                if burst:
                    amount = round(rng.uniform(0.10, 0.40), 5)
                else:
                    amount = round(rng.uniform(0.005, 0.090), 5)
                ped_value = round(amount * rng.uniform(0.32, 0.58), 5)
                db.execute(
                    "INSERT INTO skill_gains "
                    "(session_id, timestamp, skill_name, amount, ped_value) "
                    "VALUES (?, ?, ?, ?, ?)",
                    (sess["id"], ts, skill_name, amount, ped_value),
                )
            rows_written += 1
        return rows_written

    # ─── validation ─────────────────────────────────────────────────────────

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []

        if len(refs.mobs) < 4:
            violations.append(
                f"sessions: refs.mobs has {len(refs.mobs)} species, need >= 4 for diversity"
            )
        if len(refs.skill_names) < 25:
            violations.append(
                f"sessions: refs.skill_names has {len(refs.skill_names)} entries, need >= 25"
            )
        weapons = [it for it in refs.items if it.item_type == "weapon"]
        if not weapons:
            violations.append(
                "sessions: refs.items contains no weapon — kill_tool_stats needs at least one"
            )
        # Every weapon in our profiles must exist in the canonical item list, or
        # kill_tool_stats will reference a name no equipment_library row matches.
        weapon_names = {w.name for w in weapons}
        missing_profile_weapons = sorted({p[0] for p in _PROFILES} - weapon_names)
        if missing_profile_weapons:
            violations.append(
                f"sessions: profile weapons not in refs.items: {missing_profile_weapons}"
            )
        return violations


SEEDER = SessionsSeeder()


# Self-test entry point — runs core + this seeder against a temp dir.
if __name__ == "__main__":
    import shutil
    import tempfile

    logging.basicConfig(
        level=logging.INFO, format="%(levelname)s %(name)s: %(message)s"
    )

    from backend.scripts.demo_seed.driver import format_report, run

    tmp = Path(tempfile.mkdtemp(prefix="demoseed_sessions_"))
    try:
        report = run(tmp, extra_seeders=[SEEDER])
        print(format_report(report))
        if report.violations:
            raise SystemExit(1)
    finally:
        shutil.rmtree(tmp)
