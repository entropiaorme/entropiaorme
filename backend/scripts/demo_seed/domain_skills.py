"""Skills domain seeder — populates skill_calibrations for canonical skills + attributes.

Writes one source='scan' anchor row per skill / attribute plus 1-4 incremental
progress rows (source='codex' or 'chatlog') showing growth between the anchor
and "current" level. The frontend Character > Skills / Attributes tabs read
this table directly; the Professions tab derives from skill levels via
game-data weight tables.

Magnitudes sit at EU profession-milestone references (Marksmanship 1000,
Serendipity 3000, Coolness 4000, Combat Sense 5000, Commando 7000): main
combat skills land in the 4000-7000 band, secondary combat in 1500-4000,
off-profession skills in the hundreds, Trade barely above the floor.
Attributes sit at typical hunter ranges (Health ~170, others ~20-90).
"""

from __future__ import annotations

import logging
import random
import sqlite3
from pathlib import Path

from backend.scripts.demo_seed.contract import CanonicalRefs

log = logging.getLogger(__name__)

# Distinctive seed so this domain's RNG stream doesn't correlate with peers.
_RNG_SEED = 0x5C12_5C12

# Per-category target ranges for synthetic active-hunter career. Lower bound
# is the "barely touched" floor; upper bound is the "actively grinding main"
# ceiling. Top picks per category bias to the upper third of the range.
_CATEGORY_LEVEL_RANGES: dict[str, tuple[int, int]] = {
    "Combat (Ranged)": (1500, 7000),  # main profession; tops cluster 5000-7000
    "Combat (Melee)": (200, 1500),  # off-profession; touched but not grinded
    "Support": (500, 2000),  # passive growth from self-heal habits
    "Survival": (1500, 4000),  # Evade/Dodge/Wounded climb with hunting
    "Trade": (30, 300),  # essentially untouched for a hunter
    "Mind": (150, 800),  # slow passive build
}

# Per-attribute target ranges. Health sits well above the rest (~150-200);
# Stamina lags (~20-50 for most hunters); Agility/Strength/Intelligence land
# in the 50-90 band; Psyche bumps slightly higher for ranged hunters since
# Psyche grows from the intelligence/agility-adjacent ranged skill use.
_ATTRIBUTE_RANGES: dict[str, tuple[float, float]] = {
    "Health": (150.0, 200.0),
    "Stamina": (20.0, 50.0),
    "Agility": (55.0, 88.0),
    "Strength": (50.0, 85.0),
    "Psyche": (65.0, 95.0),
    "Intelligence": (55.0, 90.0),
}

_SECONDS_PER_DAY = 86400.0


def _pick_skill_level(rng: random.Random, lo: int, hi: int, is_top: bool) -> float:
    """Pick a level in [lo, hi]. Top-skewed picks bias to the upper third."""
    if is_top:
        bottom = lo + (hi - lo) * 2 // 3
        base = rng.uniform(bottom, hi)
    else:
        # Bottom 60% of the range gets the bulk of non-top skills (long-tail
        # shape: most skills sit far below the active grind in any career).
        base = rng.triangular(lo, hi, lo + (hi - lo) * 0.4)
    # Two decimals — matches the on-screen display precision the frontend uses.
    return round(base, 2)


def _pick_gain(rng: random.Random, current: float) -> float:
    """Gain magnitude over the 90-day window, calibrated by current level.

    Skill curves are logarithmic — a level-5000 grinder gains far fewer
    points/month than a level-500 dabbler, but in absolute terms the high-level
    grinder still moves more than the dabbler because they put far more hours
    in. Numbers tuned to land plausibly in a 90-day window.
    """
    if current >= 4000:
        gain = rng.uniform(60.0, 250.0)  # active main grind
    elif current >= 1500:
        gain = rng.uniform(40.0, 180.0)  # secondary combat / survival
    elif current >= 500:
        gain = rng.uniform(20.0, 100.0)  # mid-tier
    elif current >= 100:
        gain = rng.uniform(8.0, 50.0)  # low-tier
    else:
        gain = rng.uniform(2.0, 25.0)  # near-floor
    return round(gain, 2)


def _pick_attribute_gain(rng: random.Random, current: float) -> float:
    """Attributes grow very slowly — fractional points per 90 days is typical."""
    if current >= 150:
        return round(rng.uniform(0.4, 2.0), 2)  # Health-tier
    elif current >= 60:
        return round(rng.uniform(0.3, 1.8), 2)
    else:
        return round(rng.uniform(0.2, 1.2), 2)  # Stamina-tier (slowest)


def _build_rows_for(
    rng: random.Random,
    name: str,
    current_level: float,
    gain: float,
    anchor_days_ago_range: tuple[int, int],
    demo_now: float,
    n_progress_range: tuple[int, int] = (1, 3),
) -> list[tuple[str, float, str, float]]:
    """Build the (skill_name, level, source, scanned_at) row list for one skill.

    Anchor row first, then N progress rows ending at current_level, all
    monotonically increasing in level and timestamp. Progress rows weight
    'chatlog' (per-event chat-parsed deltas) heavily over 'codex' since
    chatlog rows dominate the source distribution of a typical session by
    several orders of magnitude.
    """
    anchor_level = round(current_level - gain, 2)
    anchor_days_ago = rng.randint(*anchor_days_ago_range)
    anchor_t = demo_now - anchor_days_ago * _SECONDS_PER_DAY

    n_progress = rng.randint(*n_progress_range)

    # Timestamps strictly between anchor_t and demo_now, sorted ascending.
    span = demo_now - anchor_t
    progress_ts = sorted(
        anchor_t + span * rng.uniform(0.1, 0.95) for _ in range(n_progress)
    )
    # Force the final progress row to be "today" so the gain shows in the UI.
    progress_ts[-1] = demo_now - rng.uniform(60.0, 6 * 3600.0)
    progress_ts.sort()

    # Monotonic level fractions in (0, 1], ending at 1.0 (== current_level).
    fractions = sorted(rng.uniform(0.15, 0.9) for _ in range(n_progress - 1))
    fractions.append(1.0)

    rows: list[tuple[str, float, str, float]] = [(name, anchor_level, "scan", anchor_t)]
    for ts, frac in zip(progress_ts, fractions, strict=False):
        level = round(anchor_level + gain * frac, 2)
        # Weighted: chatlog 80%, codex 20%, mirroring the typical ratio
        # while keeping codex-driven jumps visible in the timeline.
        source = "chatlog" if rng.random() < 0.8 else "codex"
        rows.append((name, level, source, ts))
    return rows


class SkillsSeeder:
    name: str = "skills"
    depends_on: tuple[str, ...] = ("core",)

    def seed(self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path) -> None:
        rng = random.Random(_RNG_SEED)
        demo_now = refs.timeline.demo_now

        all_rows: list[tuple[str, float, str, float]] = []

        # Skills, by category: pick ~25% "top" skills per category that grind
        # harder; top skills also get more progress rows (chatlog cadence is
        # heavily skewed, since main skills accumulate thousands of events
        # while passive skills sit at 1).
        for category, skill_names in refs.skill_categories.items():
            lo, hi = _CATEGORY_LEVEL_RANGES.get(category, (500, 2000))
            top_count = max(1, len(skill_names) // 4)
            top_indices = set(rng.sample(range(len(skill_names)), top_count))
            for idx, skill_name in enumerate(skill_names):
                is_top = idx in top_indices
                current_level = _pick_skill_level(rng, lo, hi, is_top=is_top)
                gain = _pick_gain(rng, current_level)
                # Don't let gain exceed the level itself (avoid negative anchors).
                gain = min(gain, current_level - 1.0)
                rows = _build_rows_for(
                    rng,
                    skill_name,
                    current_level,
                    gain,
                    anchor_days_ago_range=(10, 60),
                    demo_now=demo_now,
                    n_progress_range=(2, 4) if is_top else (1, 3),
                )
                all_rows.extend(rows)

        # Attributes — six fixed names, per-attribute ranges (Health stands out
        # high; Stamina sits low; the remaining four cluster mid-range).
        for attr_name in refs.attribute_names:
            attr_lo, attr_hi = _ATTRIBUTE_RANGES.get(attr_name, (40.0, 90.0))
            current_level = round(rng.uniform(attr_lo, attr_hi), 2)
            gain = _pick_attribute_gain(rng, current_level)
            gain = min(gain, current_level - 1.0)
            rows = _build_rows_for(
                rng,
                attr_name,
                current_level,
                gain,
                anchor_days_ago_range=(20, 40),
                demo_now=demo_now,
            )
            all_rows.extend(rows)

        # INSERT in chronological order so any "latest" tie-breakers in
        # downstream tooling that fall back on insertion order behave naturally.
        all_rows.sort(key=lambda r: r[3])
        db.executemany(
            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) "
            "VALUES (?, ?, ?, ?)",
            all_rows,
        )

        log.info(
            "%s seeder: wrote %d skill_calibrations rows (%d skills + %d attributes).",
            self.name,
            len(all_rows),
            len(refs.skill_names),
            len(refs.attribute_names),
        )

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []
        if len(refs.skill_names) < 25:
            violations.append(
                f"skill_names sanity bound — expected >= 25, got {len(refs.skill_names)}"
            )
        if len(refs.attribute_names) != 6:
            violations.append(
                f"attribute_names sanity bound — expected exactly 6, got {len(refs.attribute_names)}"
            )
        return violations


SEEDER: SkillsSeeder = SkillsSeeder()


# Self-test entry point — runs core + this seeder against a temp dir.
if __name__ == "__main__":
    import shutil
    import tempfile

    logging.basicConfig(
        level=logging.INFO, format="%(levelname)s %(name)s: %(message)s"
    )

    from backend.scripts.demo_seed.driver import format_report, run

    tmp = Path(tempfile.mkdtemp(prefix="demoseed_skills_"))
    try:
        report = run(tmp, extra_seeders=[SEEDER])
        print(format_report(report))
        if report.violations:
            raise SystemExit(1)
    finally:
        shutil.rmtree(tmp)
