"""Quest state seeder — sets in-flight / cooling / ready distribution + claim history + analytics links.

Updates the 12 canonical ``quests`` rows (created by core) to a believable
mix of states for the Quest Grid demo view, writes ``quest_claims`` history
to populate Quest Analytics PES totals, and (when sessions exist) writes
``session_quest_completions`` + ``session_quest_analytics_links`` to feed
the Quest Analytics tab.

Quest state model — confirmed against the live code path
(``backend/services/quest_service.py::_is_quest_cooling`` +
``frontend/src/routes/quests/+page.svelte::categoryStatusCounts``):

- **In-flight**: ``started_at`` not NULL.
- **Cooling**: ``started_at`` NULL, but the most recent
  ``session_quest_completions`` row has
  ``completed_at + cooldown_hours * 3600 > now``. There is no
  ``cooldown_expires_at`` column — it is derived per-read.
- **Ready**: neither of the above.

This means cooling-state cannot be expressed via ``started_at``; it requires
a recent completion row. The naive ``started_at = recent_epoch`` recipe
would render those quests as in-flight, not cooling. We therefore write
synthetic-session completion rows (``session_id = 'demo-cooldown-<quest_id>'``)
to drive the cooling state.
"""

from __future__ import annotations

import logging
import random
import sqlite3
from pathlib import Path

from backend.scripts.demo_seed.contract import CanonicalRefs

log = logging.getLogger(__name__)

# Distinctive RNG seed for this domain — keeps RNG streams independent
# from sibling per-domain seeders.
_RNG_SEED = 0xCC55_5_DEC


# Synthetic session_id prefix for the cooling-state anchor rows. These do
# NOT correspond to ``tracking_sessions`` rows; they exist purely so the
# read-side cooldown derivation has something to compute against. The
# analytics queries group by quest_id, not session_id, so these anchors
# don't pollute analytics.
_COOLDOWN_ANCHOR_SESSION_PREFIX = "demo-cooldown-"


class QuestsSeeder:
    name: str = "quests"
    depends_on: tuple[str, ...] = ("core", "sessions")

    def seed(self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path) -> None:
        rng = random.Random(_RNG_SEED)
        demo_now = refs.timeline.demo_now

        self._seed_quest_states(rng, refs, db, demo_now)
        self._seed_quest_claims(rng, refs, db, demo_now)
        self._seed_session_links(rng, refs, db, demo_now)

    # ── State distribution ────────────────────────────────────────────────

    def _seed_quest_states(
        self,
        rng: random.Random,
        refs: CanonicalRefs,
        db: sqlite3.Connection,
        demo_now: float,
    ) -> None:
        """Apply state to each canonical quest by name — keeps the narrative
        legible (Codex chain progresses, daily missions visibly cycle, long
        horizons partly engaged) and self-documenting under maintenance.
        """
        # Lookup helper — quest names are stable across re-seeds.
        by_name = {q.name: q for q in refs.quests}

        # Hand-picked distribution. 3 in-flight, 3 cooling, 4 ready.
        # In-flight quests get started_at = demo_now - random(15min..2h).
        in_flight = [
            "Codex: Caboria II",  # chain mid-progress
            "Atrox Cull (Daily)",  # active daily
            "Codex Master: Caboria",  # long-horizon engaged
        ]
        # Cooling quests get a synthetic recent completion row so the
        # cooldown derivation fires. All must have cooldown_hours not NULL.
        cooling = [
            "Argonaut Hunt (Daily)",  # 22h cooldown, recently done
            "Bounty: Atrox Stalker",  # 168h cooldown, mid-cycle
            "Combibo Patrol",  # 22h cooldown, recently done
        ]
        # Everything else stays ready (started_at NULL, no recent completion):
        # Codex I (chain done), Codex III (chain front),
        # Bounty: Argonaut Provider, Codex Master: Daikiba.

        for name in in_flight:
            q = by_name.get(name)
            if q is None:
                log.warning(
                    "In-flight quest %r not in canonical refs — skipping.", name
                )
                continue
            offset_seconds = rng.uniform(15 * 60, 2 * 3600)
            started = demo_now - offset_seconds
            db.execute(
                "UPDATE quests SET started_at = ? WHERE id = ?",
                (started, q.db_id),
            )

        # Cooling: ensure started_at stays NULL, then write a synthetic
        # completion row with completed_at chosen so cooldown is partly
        # consumed (visible cooldown timer, not just-completed).
        for name in cooling:
            q = by_name.get(name)
            if q is None:
                log.warning("Cooling quest %r not in canonical refs — skipping.", name)
                continue
            # Read cooldown_hours back from the row (canonical authority).
            row = db.execute(
                "SELECT cooldown_hours FROM quests WHERE id = ?", (q.db_id,)
            ).fetchone()
            cd_hours = row[0] if row else None
            if not cd_hours or cd_hours <= 0:
                log.warning(
                    "Cooling quest %r has no cooldown_hours — falling back to ready.",
                    name,
                )
                continue
            # Pick a completion timestamp that leaves a visible chunk of
            # cooldown remaining. Aim for 30–80% of cooldown elapsed.
            elapsed_frac = rng.uniform(0.30, 0.80)
            completed_at = demo_now - elapsed_frac * cd_hours * 3600
            session_id = f"{_COOLDOWN_ANCHOR_SESSION_PREFIX}{q.db_id}"
            db.execute(
                "INSERT INTO session_quest_completions "
                "(session_id, quest_id, completed_at) VALUES (?, ?, ?)",
                (session_id, q.db_id, completed_at),
            )
            # Ensure started_at is NULL — core seeder leaves it NULL but be
            # explicit so re-runs after a partial seed stay coherent.
            db.execute(
                "UPDATE quests SET started_at = NULL WHERE id = ?",
                (q.db_id,),
            )

        log.info(
            "%s seeder: quest states set — %d in-flight, %d cooling, %d ready.",
            self.name,
            len(in_flight),
            len(cooling),
            len(refs.quests) - len(in_flight) - len(cooling),
        )

    # ── quest_claims history ──────────────────────────────────────────────

    def _seed_quest_claims(
        self,
        rng: random.Random,
        refs: CanonicalRefs,
        db: sqlite3.Connection,
        demo_now: float,
    ) -> None:
        """Write historical PES claims for skill-reward quests.

        ``quest_claims`` is the PES side of quest reward history (non-skill
        rewards land in ``ledger_entries`` instead — see
        ``quest_service.complete_quest``). We populate it for the skill
        reward quests so the analytics tab's per-quest PES totals show real
        values. Distributed across the last 60 days for a believable curve.

        Per-claim value scale comes from EU's Codex mechanics: each rank-up
        grants a small skill reward (typically 0.03 PED for low-tier daily
        codex picks up to a few PED for high-rank Iron/Codex-Master ticks).
        Tuned to mean ~0.21 PES, max ~0.76 PES.
        """
        # Per-quest claim plan: (count, (lo, hi)) where the band is per-claim
        # PES. Chain quests pay a single one-time headline reward at chain
        # completion (much larger band, near canonical reward_ped). Iron /
        # Codex Master quests are progressive — each claim is a single rank
        # tick (sub-PES to a few PES, lognormal-weighted small).
        claim_plan: dict[str, tuple[int, tuple[float, float]]] = {
            "Codex: Caboria I": (
                1,
                (22.5, 27.5),
            ),  # chain prize at completion (~headline)
            "Codex: Caboria II": (0, (0.0, 0.0)),  # in-flight, not yet claimed
            "Codex: Caboria III": (0, (0.0, 0.0)),  # ready, not yet claimed
            "Codex Master: Caboria": (6, (0.20, 2.50)),  # long horizon; bigger ticks
            "Codex Master: Daikiba": (5, (0.20, 2.50)),  # long horizon; bigger ticks
        }
        # Daily / bounty quests are non-skill: their reward history goes
        # through ledger_entries (out of scope for this seeder), so they get
        # zero quest_claims rows.

        by_name = {q.name: q for q in refs.quests}
        total = 0
        for name, (count, (lo, hi)) in claim_plan.items():
            q = by_name.get(name)
            if q is None or count == 0:
                continue
            for _ in range(count):
                # Per-rank ticks lean small via a square-of-uniform skew —
                # most claims land near the bottom of the band, with the
                # occasional bigger one. Mimics real codex rank-up cadence.
                if hi <= 5.0:
                    skew = rng.random() ** 2
                    ped = lo + (hi - lo) * skew
                else:
                    # Headline-band claims (chain prize): centred uniform.
                    ped = rng.uniform(lo, hi)
                # Claim history spans [demo_now - 60d, demo_now - 1d] so the
                # most recent claim isn't right on top of demo_now.
                age_seconds = rng.uniform(86400, 60 * 86400)
                claimed_at = demo_now - age_seconds
                db.execute(
                    "INSERT INTO quest_claims "
                    "(quest_id, quest_name, ped_value, claimed_at) "
                    "VALUES (?, ?, ?, ?)",
                    (q.db_id, q.name, round(ped, 2), claimed_at),
                )
                total += 1

        log.info("%s seeder: wrote %d quest_claims rows.", self.name, total)

    # ── Session linkage (depends on sessions_domain) ──────────────────────

    def _seed_session_links(
        self,
        rng: random.Random,
        refs: CanonicalRefs,
        db: sqlite3.Connection,
        demo_now: float,
    ) -> None:
        """Write session_quest_completions + session_quest_analytics_links
        for tracking_sessions written by sessions_domain.

        Self-test runs without sessions_domain registered, so ``tracking_sessions``
        is empty — gracefully no-op. The driver's depends_on resolution
        guarantees sessions_domain runs first when integrated.
        """
        rows = db.execute(
            "SELECT id, started_at, ended_at FROM tracking_sessions ORDER BY started_at"
        ).fetchall()
        if not rows:
            log.warning(
                "%s seeder: no tracking_sessions present — skipping "
                "session_quest_completions + session_quest_analytics_links. "
                "(Expected when running this seeder standalone; under the "
                "full driver, sessions_domain runs first and these populate.)",
                self.name,
            )
            return

        session_records = [(r[0], r[1], r[2]) for r in rows if r[0] is not None]
        if not session_records:
            log.warning("%s seeder: tracking_sessions present but no IDs.", self.name)
            return

        # Gather mob → quest_ids mapping so completions can prefer matching
        # the session's dominant species when possible. Falls through to
        # random pairing if no quest covers the session's mobs.
        mob_to_quests: dict[str, list[int]] = {}
        for q in refs.quests:
            for mob in q.mob_names:
                mob_to_quests.setdefault(mob, []).append(q.db_id)

        # Per-session dominant mob lookup — most-killed species in the session.
        def dominant_mob(session_id: str) -> str | None:
            row = db.execute(
                "SELECT mob_species, COUNT(*) AS c FROM kills "
                "WHERE session_id = ? AND mob_species IS NOT NULL "
                "GROUP BY mob_species ORDER BY c DESC LIMIT 1",
                (session_id,),
            ).fetchone()
            return row[0] if row else None

        # ── session_quest_completions: 5–10 sessions × 1–2 quests each ───
        target_completion_sessions = min(len(session_records), rng.randint(5, 10))
        chosen_for_completion = rng.sample(session_records, target_completion_sessions)
        completion_count = 0
        seen_pairs: set[tuple[str, int]] = set()
        for sid, sstart, _send in chosen_for_completion:
            mob = dominant_mob(sid)
            candidate_quest_ids = mob_to_quests.get(mob, []) if mob else []
            if not candidate_quest_ids:
                # Random match — analytics still works.
                candidate_quest_ids = [q.db_id for q in refs.quests]
            # Real-DB shape: most sessions have exactly 1 completion (~72%),
            # a handful have 2+. Bias the distribution to mostly-1.
            n_links = 1 if rng.random() < 0.75 else 2
            for qid in rng.sample(
                candidate_quest_ids, min(n_links, len(candidate_quest_ids))
            ):
                if (sid, qid) in seen_pairs:
                    continue
                seen_pairs.add((sid, qid))
                # Place completion near the session end (or start if no end).
                base_ts = sstart if sstart else demo_now
                if _send and _send > base_ts:
                    base_ts = _send
                completed_at = float(base_ts)
                try:
                    db.execute(
                        "INSERT INTO session_quest_completions "
                        "(session_id, quest_id, completed_at) "
                        "VALUES (?, ?, ?)",
                        (sid, qid, completed_at),
                    )
                    completion_count += 1
                except sqlite3.IntegrityError:
                    # UNIQUE(session_id, quest_id) collision — extremely
                    # unlikely with synthetic anchor sessions but harmless.
                    continue

        # ── session_quest_analytics_links: 8-12 split ~55% quest / 30% playlist / 15% declined
        # Mix sized to a 10/5/3 of 18 split (56%/28%/17%) so the
        # analytics page has a visibly populated all-three-types breakdown.
        # The 'declined' link_type means the user explicitly opted this session
        # out of curated analytics — see quest_service.decline_session_link.
        target_links = min(len(session_records), rng.randint(8, 12))
        chosen_for_links = rng.sample(session_records, target_links)
        link_count = 0
        playlist_ids = [p.db_id for p in refs.playlists]
        for idx, (sid, _sstart, _send) in enumerate(chosen_for_links):
            roll = rng.random()
            if roll < 0.15:
                kind = "declined"
            elif roll < 0.45 and playlist_ids:
                kind = "playlist"
            else:
                kind = "quest"

            try:
                if kind == "declined":
                    db.execute(
                        "INSERT INTO session_quest_analytics_links "
                        "(session_id, link_type, quest_id, playlist_id, linked_at) "
                        "VALUES (?, 'declined', NULL, NULL, ?)",
                        (sid, demo_now),
                    )
                elif kind == "playlist":
                    pid = rng.choice(playlist_ids)
                    db.execute(
                        "INSERT INTO session_quest_analytics_links "
                        "(session_id, link_type, quest_id, playlist_id, linked_at) "
                        "VALUES (?, 'playlist', NULL, ?, ?)",
                        (sid, pid, demo_now),
                    )
                else:
                    # Prefer a quest that matches this session's dominant mob.
                    mob = dominant_mob(sid)
                    candidate_quest_ids = mob_to_quests.get(mob, []) if mob else []
                    if not candidate_quest_ids:
                        candidate_quest_ids = [q.db_id for q in refs.quests]
                    qid = rng.choice(candidate_quest_ids)
                    db.execute(
                        "INSERT INTO session_quest_analytics_links "
                        "(session_id, link_type, quest_id, playlist_id, linked_at) "
                        "VALUES (?, 'quest', ?, NULL, ?)",
                        (sid, qid, demo_now),
                    )
                link_count += 1
            except sqlite3.IntegrityError:
                continue

        log.info(
            "%s seeder: wrote %d session_quest_completions + %d "
            "session_quest_analytics_links across %d sessions.",
            self.name,
            completion_count,
            link_count,
            len(session_records),
        )

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []
        if len(refs.quests) < 8:
            violations.append(
                f"refs.quests has only {len(refs.quests)} entries; "
                "expected at least 8 for the quest grid demo."
            )
        if len(refs.playlists) < 2:
            violations.append(
                f"refs.playlists has only {len(refs.playlists)} entries; "
                "expected at least 2 for analytics playlist links."
            )
        return violations


SEEDER = QuestsSeeder()


# Self-test — runs core + this seeder against a temp dir, prints the report.
if __name__ == "__main__":
    import shutil
    import tempfile

    logging.basicConfig(
        level=logging.INFO, format="%(levelname)s %(name)s: %(message)s"
    )

    from backend.scripts.demo_seed.driver import format_report, run

    # The driver enforces depends_on order; sessions_domain isn't registered
    # in the self-test, so this seeder gracefully no-ops on the session-tied
    # rows. To let it actually run alongside core, we patch its depends_on
    # to drop "sessions_domain" for the self-test only.
    SEEDER.depends_on = (
        "core",
    )  # self-test only; the real depends_on is the class-level tuple above

    tmp = Path(tempfile.mkdtemp(prefix="demoseed_quests_"))
    try:
        report = run(tmp, extra_seeders=[SEEDER])
        print(format_report(report))
        if report.violations:
            raise SystemExit(1)
    finally:
        shutil.rmtree(tmp)
