"""Skill gain tracker — records chat.log skill events during tracking sessions.

Subscribes to EVENT_SKILL_GAIN on the event bus. During active sessions,
records each gain to the skill_gains table with TT value computation.
Also increments the calibrated skill level so TT values stay accurate
between full scans.
"""

import logging
import threading
import time as _time
from datetime import datetime

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_SKILL_GAIN,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.data.tt_value_curve import tt_value_of_gain
from backend.services.character_calc import ATTRIBUTE_SKILLS

log = logging.getLogger(__name__)


class SkillTracker:
    """Records skill gains from chat.log during active tracking sessions."""

    def __init__(self, event_bus: EventBus, app_db):
        self._event_bus = event_bus
        self._app_db = app_db
        self._db = app_db.conn
        self._db_lock: threading.RLock = app_db.lock
        self._active = False
        self._session_id: str | None = None

        # In-memory session totals
        self._session_skills: dict[str, float] = {}  # name → total amount
        self._session_skill_tt: dict[str, float] = {}  # name → total TT PED

        # Codex claim suppression: {skill_name: expiry_epoch}
        self._suppressed_claims: dict[str, float] = {}

        event_bus.subscribe(EVENT_SKILL_GAIN, self._on_skill_gain)
        event_bus.subscribe(EVENT_SESSION_STARTED, self._on_session_start)
        event_bus.subscribe(EVENT_SESSION_STOPPED, self._on_session_stop)

    def _on_session_start(self, data: dict) -> None:
        self._active = True
        self._session_id = data.get("session_id")
        self._session_skills.clear()
        self._session_skill_tt.clear()
        # A suppression armed in a prior session must not carry into this one.
        self._suppressed_claims.clear()
        log.info(
            "Skill tracking started for session %s",
            self._session_id[:8] if self._session_id else "?",
        )

    def _on_session_stop(self, data: dict) -> None:
        if self._session_skills:
            total_exp = sum(self._session_skills.values())
            total_tt = sum(self._session_skill_tt.values())
            log.info(
                "Skill tracking stopped: %d skills, %.4f exp, %.4f PED TT",
                len(self._session_skills),
                total_exp,
                total_tt,
            )
        self._active = False
        self._session_id = None
        # Drop any still-armed codex suppression so it can't bleed into the
        # next session.
        self._suppressed_claims.clear()

    def _on_skill_gain(self, data: dict) -> None:
        if not self._active or not self._session_id:
            return

        skill_name: str = data["skill_name"]
        amount: float = data["amount"]
        timestamp: datetime = data["timestamp"]
        ts_epoch = (
            timestamp.timestamp()
            if isinstance(timestamp, datetime)
            else float(timestamp)
        )

        # Check codex claim suppression: swallow the next matching gain so the
        # in-game skill-up a codex claim produces isn't double-counted alongside
        # the ledger entry the claim already recorded.
        if skill_name in self._suppressed_claims:
            expiry = self._suppressed_claims[skill_name]
            del self._suppressed_claims[skill_name]
            if _time.time() < expiry:
                log.info(
                    "Codex-claim gain suppressed: %s +%.4f levels", skill_name, amount
                )
                return
            # Expired: fall through and process normally
            log.info("Suppression for %s expired, processing normally", skill_name)

        # Get current calibrated level for TT computation
        old_level = self._get_current_level(skill_name)
        ped_value = None
        is_attribute = skill_name in ATTRIBUTE_SKILLS

        if old_level is not None:
            new_level = old_level + amount
            # Only compute TT value for regular skills — no attribute curve exists yet
            if not is_attribute:
                ped_value = tt_value_of_gain(old_level, new_level)
            # Insert incremental calibration point (for both skills and attributes)
            with self._db_lock:
                self._db.execute(
                    "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) VALUES (?, ?, 'chatlog', ?)",
                    (skill_name, new_level, ts_epoch),
                )

        # Insert skill gain record
        with self._db_lock:
            self._db.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) VALUES (?, ?, ?, ?, ?)",
                (self._session_id, ts_epoch, skill_name, amount, ped_value),
            )
            self._db.commit()

        # Update in-memory session totals
        self._session_skills[skill_name] = (
            self._session_skills.get(skill_name, 0.0) + amount
        )
        if ped_value is not None:
            self._session_skill_tt[skill_name] = (
                self._session_skill_tt.get(skill_name, 0.0) + ped_value
            )

    def _get_current_level(self, skill_name: str) -> float | None:
        """Get the latest calibrated level for a skill."""
        with self._db_lock:
            row = self._db.execute(
                "SELECT level FROM skill_calibrations WHERE skill_name = ? ORDER BY scanned_at DESC LIMIT 1",
                (skill_name,),
            ).fetchone()
        return float(row[0]) if row else None

    def suppress_next(self, skill_name: str, timeout: float = 30.0) -> None:
        """Register a pending codex claim — suppress the next matching skill gain.

        When the player claims a codex rank in-game, the resulting skill gain
        shows up in chat.log. This method marks that upcoming gain for
        suppression so it isn't double-counted alongside the ledger entry the
        claim already recorded.
        """
        self._suppressed_claims[skill_name] = _time.time() + timeout
        log.info("Suppressing next %s gain (expires in %.0fs)", skill_name, timeout)
