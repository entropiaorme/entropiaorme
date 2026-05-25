"""Quest service — CRUD, cooldowns, playlists, completion with ledger integration."""

import logging
import re
import time
import unicodedata
import uuid
from datetime import datetime, timezone
from difflib import SequenceMatcher

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_MISSION_RECEIVED,
    EVENT_SESSION_STARTED,
    EVENT_SESSION_STOPPED,
)
from backend.db.app_database import AppDatabase

log = logging.getLogger(__name__)

# Stripped from chat.log mission names before matching against quest DB
_REPEATABLE_SUFFIX = re.compile(r"\s*\(repeatable\)\s*$", re.IGNORECASE)

_FUZZY_THRESHOLD = 0.8

PLAYLIST_GROUP_IMMEDIATE = "immediate"
PLAYLIST_GROUP_LONG_HORIZON = "long_horizon"


def _normalize_quest_name(name: str) -> str:
    """Normalise quest name for comparison: NFKD → ASCII, lowercase, strip."""
    return (
        unicodedata.normalize("NFKD", name)
        .encode("ascii", "ignore")
        .decode("ascii")
        .strip()
        .lower()
    )


class QuestService:
    """Quest operations: CRUD, cooldown tracking, playlists, completion flow."""

    def __init__(self, app_db: AppDatabase, event_bus: EventBus | None = None):
        self._db = app_db
        self._conn = app_db.conn
        self._current_session_id: str | None = None

        if event_bus:
            event_bus.subscribe(EVENT_SESSION_STARTED, self._on_session_start)
            event_bus.subscribe(EVENT_SESSION_STOPPED, self._on_session_stop)
            event_bus.subscribe(EVENT_MISSION_RECEIVED, self._on_mission_received)

    def _on_session_start(self, data: dict) -> None:
        self._current_session_id = data.get("session_id")
        log.info(
            "Quest service tracking session %s",
            self._current_session_id[:8] if self._current_session_id else "?",
        )

    def _on_session_stop(self, data: dict) -> None:
        self._current_session_id = None

    def _on_mission_received(self, data: dict) -> None:
        mission_name = data.get("mission_name", "")
        if mission_name:
            self.start_quest_from_mission(mission_name)

    # ── Quest CRUD ───────────────────────────────────────────────────────────

    _QUEST_SELECT = """
        SELECT q.*,
               (SELECT MAX(completed_at)
                FROM session_quest_completions
                WHERE quest_id = q.id) AS last_completed_at
        FROM quests q
    """

    def get_quests(self, active_only: bool = True) -> list[dict]:
        """List all quests, enriched with mobs and playlist membership."""
        where = "WHERE q.is_active = 1" if active_only else ""
        rows = self._conn.execute(
            f"{self._QUEST_SELECT} {where} ORDER BY q.created_at ASC"
        ).fetchall()
        quests = [self._row_to_quest(r) for r in rows]
        for q in quests:
            q["mobs"] = self._get_quest_mobs(q["id"])
            q["playlist_ids"] = self._get_quest_playlist_ids(q["id"])
        return quests

    def get_quest(self, quest_id: int) -> dict | None:
        """Get a single quest by ID, enriched."""
        row = self._conn.execute(
            f"{self._QUEST_SELECT} WHERE q.id = ?", (quest_id,)
        ).fetchone()
        if not row:
            return None
        q = self._row_to_quest(row)
        q["mobs"] = self._get_quest_mobs(quest_id)
        q["playlist_ids"] = self._get_quest_playlist_ids(quest_id)
        return q

    def create_quest(self, data: dict) -> dict:
        """Create a quest and return it."""
        cur = self._conn.execute(
            """INSERT INTO quests (name, planet, waypoint, cooldown_hours,
               reward_ped, reward_is_skill, expected_reward_markup_percent,
               notes, chain_name, chain_position, chain_total,
               category, reward_description)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
            (
                data["name"],
                data.get("planet", "Calypso"),
                data.get("waypoint"),
                data.get("cooldown_hours"),
                data.get("reward_ped"),
                1 if data.get("reward_is_skill") else 0,
                self._normalize_expected_reward_markup(
                    data.get("reward_ped"),
                    data.get("reward_is_skill"),
                    data.get("expected_reward_markup_percent"),
                ),
                data.get("notes"),
                data.get("chain_name"),
                data.get("chain_position"),
                data.get("chain_total"),
                data.get("category"),
                data.get("reward_description"),
            ),
        )
        quest_id = cur.lastrowid

        mobs = data.get("mobs", [])
        if mobs:
            self._set_quest_mobs(quest_id, mobs)

        self._conn.commit()
        return self.get_quest(quest_id)

    def update_quest(self, quest_id: int, data: dict) -> dict | None:
        """Update a quest's fields. Returns updated quest or None if not found."""
        existing = self.get_quest(quest_id)
        if not existing:
            return None

        allowed = {
            "name",
            "planet",
            "waypoint",
            "cooldown_hours",
            "reward_ped",
            "reward_is_skill",
            "notes",
            "chain_name",
            "chain_position",
            "chain_total",
            "category",
            "reward_description",
            "expected_reward_markup_percent",
        }
        updates = {}
        for key in allowed:
            if key in data:
                val = data[key]
                if key == "reward_is_skill":
                    val = 1 if val else 0
                updates[key] = val

        if any(
            key in data
            for key in (
                "reward_ped",
                "reward_is_skill",
                "expected_reward_markup_percent",
            )
        ):
            reward_ped = (
                data["reward_ped"]
                if "reward_ped" in data
                else existing.get("reward_ped")
            )
            reward_is_skill = (
                data["reward_is_skill"]
                if "reward_is_skill" in data
                else existing.get("reward_is_skill")
            )
            expected_markup = (
                data["expected_reward_markup_percent"]
                if "expected_reward_markup_percent" in data
                else existing.get("expected_reward_markup_percent")
            )
            updates["expected_reward_markup_percent"] = (
                self._normalize_expected_reward_markup(
                    reward_ped,
                    reward_is_skill,
                    expected_markup,
                )
            )

        if updates:
            set_clause = ", ".join(f"{k} = ?" for k in updates)
            self._conn.execute(
                f"UPDATE quests SET {set_clause} WHERE id = ?",
                (*updates.values(), quest_id),
            )

        if "mobs" in data:
            self._set_quest_mobs(quest_id, data["mobs"])

        self._conn.commit()
        return self.get_quest(quest_id)

    def delete_quest(self, quest_id: int) -> bool:
        """Soft-delete a quest."""
        cur = self._conn.execute(
            "UPDATE quests SET is_active = 0 WHERE id = ? AND is_active = 1",
            (quest_id,),
        )
        self._conn.commit()
        if cur.rowcount > 0:
            # Remove from all playlists
            self._conn.execute(
                "DELETE FROM quest_playlist_items WHERE quest_id = ?",
                (quest_id,),
            )
            self._conn.commit()
            return True
        return False

    # ── Quest actions ────────────────────────────────────────────────────────

    def start_quest(self, quest_id: int) -> dict | None:
        """Mark a quest as in-progress."""
        cur = self._conn.execute(
            "UPDATE quests SET started_at = ? WHERE id = ? AND is_active = 1",
            (time.time(), quest_id),
        )
        self._conn.commit()
        return self.get_quest(quest_id) if cur.rowcount > 0 else None

    def complete_quest(self, quest_id: int) -> dict | None:
        """Complete a quest: clear in-progress state, record the reward, and
        link the completion to the active tracking session.

        Liquid rewards (`reward_is_skill = 0`) emit a markup row into
        `ledger_entries`. Skill rewards (`reward_is_skill = 1`) emit a
        `quest_claims` row instead — they are PES, not PED, and stay out
        of liquid P&L.

        Cooldown and completion counts are derived at read time from
        session_quest_completions — no counter column is written here.
        When no session is active, a session-less completion row is recorded
        so that cooldown still reflects the completion.
        """
        quest = self.get_quest(quest_id)
        if not quest:
            return None

        now = time.time()
        self._conn.execute(
            "UPDATE quests SET started_at = NULL WHERE id = ?",
            (quest_id,),
        )
        self._conn.commit()

        if quest["reward_ped"] and quest["reward_ped"] > 0:
            if quest.get("reward_is_skill"):
                self._conn.execute(
                    "INSERT INTO quest_claims (quest_id, quest_name, ped_value, claimed_at) "
                    "VALUES (?, ?, ?, ?)",
                    (quest_id, quest["name"], float(quest["reward_ped"]), now),
                )
                self._conn.commit()
                log.info(
                    "Auto-created quest claim for '%s': %.2f PES",
                    quest["name"],
                    quest["reward_ped"],
                )
            else:
                ledger_id = str(uuid.uuid4())
                date_str = datetime.fromtimestamp(now, tz=timezone.utc).isoformat()
                self._conn.execute(
                    "INSERT INTO ledger_entries (id, date, type, description, amount, tag) VALUES (?, ?, ?, ?, ?, ?)",
                    (
                        ledger_id,
                        date_str,
                        "markup",
                        f"Quest: {quest['name']}",
                        quest["reward_ped"],
                        "quest_reward",
                    ),
                )
                self._conn.commit()
                log.info(
                    "Auto-created ledger entry for quest '%s': %.2f PED",
                    quest["name"],
                    quest["reward_ped"],
                )

        session_id = self._resolve_session_for_completion()
        self._record_session_completion(session_id, quest_id, now)

        return self.get_quest(quest_id)

    def cancel_quest(self, quest_id: int, undo_reward: bool = False) -> dict | None:
        """Undo an in-progress quest, or reset an active cooldown back to ready.

        Cooldown is derived from `session_quest_completions`, so resetting
        it means deleting the most recent completion row for the quest.
        """
        quest = self.get_quest(quest_id)
        if not quest:
            return None

        if quest.get("started_at") is not None:
            self._conn.execute(
                "UPDATE quests SET started_at = NULL WHERE id = ? AND is_active = 1",
                (quest_id,),
            )
            self._conn.commit()
            return self.get_quest(quest_id)

        if not self._is_quest_cooling(quest):
            return quest

        self._conn.execute(
            """DELETE FROM session_quest_completions
               WHERE id = (
                   SELECT id FROM session_quest_completions
                   WHERE quest_id = ?
                   ORDER BY completed_at DESC, id DESC
                   LIMIT 1
               )""",
            (quest_id,),
        )

        if undo_reward and quest.get("reward_ped") and quest["reward_ped"] > 0:
            if quest.get("reward_is_skill"):
                deleted = self._delete_latest_quest_claim(quest_id)
                if deleted:
                    log.info(
                        "Removed latest quest claim for '%s': %.2f PES",
                        quest["name"],
                        quest["reward_ped"],
                    )
            else:
                deleted = self._delete_latest_quest_reward_entry(
                    quest["name"],
                    float(quest["reward_ped"]),
                )
                if deleted:
                    log.info(
                        "Removed latest quest reward ledger entry for '%s': %.2f PED",
                        quest["name"],
                        quest["reward_ped"],
                    )

        self._conn.commit()
        return self.get_quest(quest_id)

    def _resolve_session_for_completion(self) -> str | None:
        """Return the session ID to link a quest completion to.

        Manual quest completions outside active tracking stay session-less.
        """
        return self._current_session_id

    def get_session_link_suggestion(self, session_id: str) -> dict:
        """Suggest a curated analytics link for a completed session."""
        existing = self._get_session_analytics_link_row(session_id)
        if existing:
            reason = (
                "declined" if existing["link_type"] == "declined" else "already_linked"
            )
            return {
                "suggestion_type": "none",
                "reason": reason,
                "quest_id": existing["quest_id"],
                "quest_name": self._get_quest_name(existing["quest_id"]),
                "playlist_id": existing["playlist_id"],
                "playlist_name": self._get_playlist_name(existing["playlist_id"]),
            }

        quest_ids = self._get_session_completed_quest_ids(session_id)
        if not quest_ids:
            return {
                "suggestion_type": "none",
                "reason": "no_completions",
                "quest_id": None,
                "quest_name": None,
                "playlist_id": None,
                "playlist_name": None,
            }

        if len(quest_ids) == 1:
            quest_id = quest_ids[0]
            return {
                "suggestion_type": "quest",
                "reason": "single_quest",
                "quest_id": quest_id,
                "quest_name": self._get_quest_name(quest_id),
                "playlist_id": None,
                "playlist_name": None,
            }

        playlist_ids = self._find_matching_playlists(quest_ids)
        if len(playlist_ids) == 1:
            playlist_id = playlist_ids[0]
            return {
                "suggestion_type": "playlist",
                "reason": "exact_playlist",
                "quest_id": None,
                "quest_name": None,
                "playlist_id": playlist_id,
                "playlist_name": self._get_playlist_name(playlist_id),
            }

        reason = "ambiguous_playlist" if playlist_ids else "unclean"
        return {
            "suggestion_type": "none",
            "reason": reason,
            "quest_id": None,
            "quest_name": None,
            "playlist_id": None,
            "playlist_name": None,
        }

    def accept_session_link_suggestion(self, session_id: str) -> dict:
        """Persist the current curated analytics suggestion for a session."""
        suggestion = self.get_session_link_suggestion(session_id)
        suggestion_type = suggestion["suggestion_type"]
        if suggestion_type == "quest":
            self._set_session_analytics_link(
                session_id,
                "quest",
                quest_id=suggestion["quest_id"],
            )
        elif suggestion_type == "playlist":
            self._set_session_analytics_link(
                session_id,
                "playlist",
                playlist_id=suggestion["playlist_id"],
            )
        else:
            raise ValueError(
                f"No linkable suggestion for session {session_id}: {suggestion['reason']}"
            )
        return suggestion

    def decline_session_link(self, session_id: str) -> None:
        """Persist that the user declined curated analytics linkage."""
        self._set_session_analytics_link(session_id, "declined")

    # ── Playlist CRUD ────────────────────────────────────────────────────────

    def get_playlists(self, active_only: bool = True) -> list[dict]:
        """List all playlists with classified items in order."""
        where = "WHERE is_active = 1" if active_only else ""
        rows = self._conn.execute(
            f"SELECT * FROM quest_playlists {where} ORDER BY created_at ASC"
        ).fetchall()
        playlists = []
        for row in rows:
            pl = self._row_to_playlist(row)
            items = self._get_playlist_items(pl["id"])
            immediate_ids, long_horizon_ids = self._split_playlist_item_groups(items)
            pl["quest_ids"] = [i["quest_id"] for i in items]
            pl["immediate_quest_ids"] = immediate_ids
            pl["long_horizon_quest_ids"] = long_horizon_ids
            pl["items"] = items
            playlists.append(pl)
        return playlists

    def get_playlist(self, playlist_id: int) -> dict | None:
        """Get a single playlist by ID."""
        row = self._conn.execute(
            "SELECT * FROM quest_playlists WHERE id = ?", (playlist_id,)
        ).fetchone()
        if not row:
            return None
        pl = self._row_to_playlist(row)
        items = self._get_playlist_items(playlist_id)
        immediate_ids, long_horizon_ids = self._split_playlist_item_groups(items)
        pl["quest_ids"] = [i["quest_id"] for i in items]
        pl["immediate_quest_ids"] = immediate_ids
        pl["long_horizon_quest_ids"] = long_horizon_ids
        pl["items"] = items
        return pl

    def create_playlist(self, data: dict) -> dict:
        """Create a playlist with classified items."""
        cur = self._conn.execute(
            "INSERT INTO quest_playlists (name, planet, estimated_minutes) VALUES (?, ?, ?)",
            (
                data["name"],
                data.get("planet", "Calypso"),
                data.get("estimated_minutes", 30),
            ),
        )
        playlist_id = cur.lastrowid

        items = self._normalize_playlist_items(data)
        self._set_playlist_items(playlist_id, items)
        self._conn.commit()

        return self.get_playlist(playlist_id)

    def update_playlist(self, playlist_id: int, data: dict) -> dict | None:
        """Update a playlist's fields and/or classified quest groups."""
        existing = self.get_playlist(playlist_id)
        if not existing:
            return None

        allowed = {"name", "planet", "estimated_minutes"}
        updates = {k: data[k] for k in allowed if k in data}

        if updates:
            set_clause = ", ".join(f"{k} = ?" for k in updates)
            self._conn.execute(
                f"UPDATE quest_playlists SET {set_clause} WHERE id = ?",
                (*updates.values(), playlist_id),
            )

        if "items" in data:
            self._set_playlist_items(playlist_id, self._normalize_playlist_items(data))
        elif "quest_ids" in data:
            self._set_playlist_items(playlist_id, self._normalize_playlist_items(data))

        self._conn.commit()
        return self.get_playlist(playlist_id)

    def delete_playlist(self, playlist_id: int) -> bool:
        """Soft-delete a playlist."""
        cur = self._conn.execute(
            "UPDATE quest_playlists SET is_active = 0 WHERE id = ? AND is_active = 1",
            (playlist_id,),
        )
        self._conn.commit()
        if cur.rowcount > 0:
            self._conn.execute(
                "DELETE FROM quest_playlist_items WHERE playlist_id = ?",
                (playlist_id,),
            )
            self._conn.commit()
            return True
        return False

    # ── Analytics ────────────────────────────────────────────────────────────

    def get_quest_analytics(self) -> list[dict]:
        """Per-quest sustainability metrics across all linked sessions.

        Returns raw totals; frontend derives averages.
        Only includes quests that have at least one linked session.
        """
        quest_rows = self._conn.execute(
            """SELECT q.id, q.name, q.planet, q.category, q.reward_ped,
                      q.reward_is_skill, q.expected_reward_markup_percent
               FROM quests q
               WHERE q.is_active = 1
               ORDER BY q.name"""
        ).fetchall()

        results = []
        for qr in quest_rows:
            stats = self._compute_quest_session_stats(qr[0])
            if stats["linked_sessions"] == 0:
                continue
            results.append(
                {
                    "quest_id": qr[0],
                    "quest_name": qr[1],
                    "planet": qr[2],
                    "category": qr[3],
                    "reward_ped": qr[4] or 0,
                    "reward_is_skill": bool(qr[5]),
                    "expected_reward_markup_percent": qr[6],
                    "total_expected_reward_ped": self._expected_reward_total(
                        qr[4] or 0,
                        bool(qr[5]),
                        qr[6],
                        stats["linked_sessions"],
                    ),
                    **stats,
                }
            )
        return results

    def _compute_quest_session_stats(self, quest_id: int) -> dict:
        """Aggregate economics for all sessions where this quest was completed.

        Looks up sessions via the curated analytics link table,
        then delegates to _compute_session_set_stats.
        """
        rows = self._conn.execute(
            """SELECT session_id FROM session_quest_analytics_links
               WHERE quest_id = ? AND link_type = 'quest'""",
            (quest_id,),
        ).fetchall()
        session_ids = [r[0] for r in rows]
        return self._compute_session_set_stats(session_ids)

    # ── Playlist analytics ────────────────────────────────────────────────

    def get_all_playlist_analytics(self) -> list[dict]:
        """Per-playlist sustainability metrics from curated linked sessions."""
        playlists = self.get_playlists(active_only=True)
        results = []
        for pl in playlists:
            stats = self.get_playlist_analytics(pl["id"])
            if stats:
                results.append(stats)
        return results

    def get_playlist_analytics(self, playlist_id: int) -> dict | None:
        """Analytics for a single playlist from curated linked sessions."""
        pl = self.get_playlist(playlist_id)
        if not pl:
            return None

        immediate_ids = self._get_playlist_quest_ids(
            playlist_id, PLAYLIST_GROUP_IMMEDIATE
        )
        long_horizon_ids = self._get_playlist_quest_ids(
            playlist_id, PLAYLIST_GROUP_LONG_HORIZON
        )
        if not immediate_ids:
            return {
                "playlist_id": playlist_id,
                "playlist_name": pl["name"],
                "quest_count": 0,
                "long_horizon_quest_count": len(long_horizon_ids),
                "matched_sessions": 0,
                "total_reward_ped": 0,
                "total_immediate_reward_ped": 0,
                "total_bonus_reward_ped": 0,
                "total_skill_reward_ped": 0,
                "total_immediate_skill_reward_ped": 0,
                "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 0,
                "total_expected_immediate_reward_ped": 0,
                "total_expected_bonus_reward_ped": 0,
                "total_duration": 0,
                "weapon_cost": 0,
                "heal_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            }

        session_ids = self._get_curated_playlist_session_ids(playlist_id)
        stats = (
            self._compute_session_set_stats(session_ids)
            if session_ids
            else {
                "linked_sessions": 0,
                "total_duration": 0,
                "weapon_cost": 0,
                "heal_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            }
        )
        reward_stats = self._compute_playlist_reward_stats(
            session_ids, immediate_ids, long_horizon_ids
        )

        return {
            "playlist_id": playlist_id,
            "playlist_name": pl["name"],
            "quest_count": len(immediate_ids),
            "long_horizon_quest_count": len(long_horizon_ids),
            **reward_stats,
            "matched_sessions": stats["linked_sessions"],
            **stats,
        }

    def _find_matching_playlists(self, completed_quest_ids: list[int]) -> list[int]:
        """Find playlists whose immediate set is complete and extras stay within playlist scope."""
        completed_set = set(completed_quest_ids)
        matches = []
        for playlist in self.get_playlists(active_only=True):
            immediate_set = set(playlist.get("immediate_quest_ids", []))
            if not immediate_set:
                continue
            long_horizon_set = set(playlist.get("long_horizon_quest_ids", []))
            playlist_scope = immediate_set | long_horizon_set
            if immediate_set.issubset(completed_set) and completed_set.issubset(
                playlist_scope
            ):
                matches.append(playlist["id"])
        return matches

    def _get_curated_playlist_session_ids(self, playlist_id: int) -> list[str]:
        rows = self._conn.execute(
            """SELECT session_id FROM session_quest_analytics_links
               WHERE playlist_id = ? AND link_type = 'playlist'""",
            (playlist_id,),
        ).fetchall()
        return [r[0] for r in rows]

    def _compute_session_set_stats(self, session_ids: list[str]) -> dict:
        """Aggregate economics for a set of sessions (by ID)."""
        if not session_ids:
            return {
                "linked_sessions": 0,
                "total_duration": 0,
                "weapon_cost": 0,
                "heal_cost": 0,
                "enhancer_cost": 0,
                "armour_cost": 0,
                "loot_tt": 0,
                "skill_tt": 0,
            }

        placeholders = ",".join("?" * len(session_ids))

        sess_row = self._conn.execute(
            f"""SELECT COUNT(*),
                       COALESCE(SUM(s.ended_at - s.started_at), 0),
                       COALESCE(SUM(s.heal_cost), 0),
                       COALESCE(SUM(s.armour_cost), 0)
                FROM tracking_sessions s
                WHERE s.id IN ({placeholders}) AND s.is_active = 0""",
            session_ids,
        ).fetchone()

        weapon_cost = self._conn.execute(
            f"""SELECT COALESCE(SUM(ts.cost_per_shot * ts.shots_fired), 0)
                FROM kill_tool_stats ts
                JOIN kills k ON k.id = ts.kill_id
                WHERE k.session_id IN ({placeholders})""",
            session_ids,
        ).fetchone()[0]

        enhancer_cost = self._conn.execute(
            f"""SELECT COALESCE(SUM(k.enhancer_cost), 0)
                FROM kills k
                WHERE k.session_id IN ({placeholders})""",
            session_ids,
        ).fetchone()[0]

        loot_tt = self._conn.execute(
            f"""SELECT COALESCE(SUM(k.loot_total_ped), 0)
                FROM kills k
                WHERE k.session_id IN ({placeholders})""",
            session_ids,
        ).fetchone()[0]

        skill_tt = self._conn.execute(
            f"""SELECT COALESCE(SUM(sg.ped_value), 0)
                FROM skill_gains sg
                WHERE sg.session_id IN ({placeholders})""",
            session_ids,
        ).fetchone()[0]

        return {
            "linked_sessions": sess_row[0],
            "total_duration": sess_row[1],
            "weapon_cost": weapon_cost,
            "heal_cost": sess_row[2],
            "enhancer_cost": enhancer_cost,
            "armour_cost": sess_row[3],
            "loot_tt": loot_tt,
            "skill_tt": skill_tt,
        }

    def _compute_playlist_reward_stats(
        self,
        session_ids: list[str],
        immediate_ids: list[int],
        long_horizon_ids: list[int],
    ) -> dict:
        if not session_ids:
            return {
                "total_reward_ped": 0,
                "total_immediate_reward_ped": 0,
                "total_bonus_reward_ped": 0,
                "total_skill_reward_ped": 0,
                "total_immediate_skill_reward_ped": 0,
                "total_bonus_skill_reward_ped": 0,
                "total_expected_reward_ped": 0,
                "total_expected_immediate_reward_ped": 0,
                "total_expected_bonus_reward_ped": 0,
            }

        total_immediate = self._sum_session_quest_rewards(session_ids, immediate_ids)
        total_bonus = self._sum_session_quest_rewards(session_ids, long_horizon_ids)
        total_immediate_skill = self._sum_session_quest_rewards(
            session_ids,
            immediate_ids,
            skill_only=True,
        )
        total_bonus_skill = self._sum_session_quest_rewards(
            session_ids,
            long_horizon_ids,
            skill_only=True,
        )
        total_expected_immediate = self._sum_session_quest_rewards(
            session_ids,
            immediate_ids,
            expected=True,
        )
        total_expected_bonus = self._sum_session_quest_rewards(
            session_ids,
            long_horizon_ids,
            expected=True,
        )
        return {
            "total_reward_ped": total_immediate + total_bonus,
            "total_immediate_reward_ped": total_immediate,
            "total_bonus_reward_ped": total_bonus,
            "total_skill_reward_ped": total_immediate_skill + total_bonus_skill,
            "total_immediate_skill_reward_ped": total_immediate_skill,
            "total_bonus_skill_reward_ped": total_bonus_skill,
            "total_expected_reward_ped": total_expected_immediate
            + total_expected_bonus,
            "total_expected_immediate_reward_ped": total_expected_immediate,
            "total_expected_bonus_reward_ped": total_expected_bonus,
        }

    def _sum_session_quest_rewards(
        self,
        session_ids: list[str],
        quest_ids: list[int],
        *,
        expected: bool = False,
        skill_only: bool | None = None,
    ) -> float:
        if not session_ids or not quest_ids:
            return 0
        session_placeholders = ",".join("?" * len(session_ids))
        quest_placeholders = ",".join("?" * len(quest_ids))
        reward_expr = "q.reward_ped"
        if expected:
            reward_expr = """CASE
                WHEN q.reward_is_skill = 1 OR q.reward_ped IS NULL THEN q.reward_ped
                WHEN q.expected_reward_markup_percent IS NULL THEN q.reward_ped
                ELSE q.reward_ped * q.expected_reward_markup_percent / 100.0
            END"""
        skill_filter = ""
        if skill_only is True:
            skill_filter = " AND q.reward_is_skill = 1"
        elif skill_only is False:
            skill_filter = " AND q.reward_is_skill = 0"
        row = self._conn.execute(
            f"""SELECT COALESCE(SUM({reward_expr}), 0)
                FROM session_quest_completions sqc
                JOIN quests q ON q.id = sqc.quest_id
                WHERE sqc.session_id IN ({session_placeholders})
                  AND sqc.quest_id IN ({quest_placeholders})
                  {skill_filter}""",
            (*session_ids, *quest_ids),
        ).fetchone()
        return row[0] or 0

    # ── Chat.log mission detection ─────────────────────────────────────────

    def match_quest_by_mission_name(self, mission_name: str) -> dict | None:
        """Find a quest whose name matches a chat.log mission name.

        Strips "(repeatable)" suffix, then tries:
        1. Normalised exact match (Unicode → ASCII, case-insensitive)
        2. Normalised substring containment (min 5 chars to avoid false positives)
        3. Fuzzy match via SequenceMatcher (highest score >= 0.8 wins)
        """
        stripped = _REPEATABLE_SUFFIX.sub("", mission_name).strip()
        mission_norm = _normalize_quest_name(stripped)
        quests = self.get_quests(active_only=True)

        # 1. Normalised exact match
        for q in quests:
            if _normalize_quest_name(q["name"]) == mission_norm:
                return q

        # 2. Normalised substring — quest name contained in mission name
        for q in quests:
            qnorm = _normalize_quest_name(q["name"])
            if len(qnorm) >= 5 and qnorm in mission_norm:
                return q

        # 3. Fuzzy match — pick highest-scoring quest above threshold
        best_score = 0.0
        best_quest = None
        for q in quests:
            qnorm = _normalize_quest_name(q["name"])
            score = SequenceMatcher(None, qnorm, mission_norm).ratio()
            if score > best_score:
                best_score = score
                best_quest = q

        return best_quest if best_score >= _FUZZY_THRESHOLD else None

    def start_quest_from_mission(self, mission_name: str) -> None:
        """Called when a 'New Mission received' chat.log event fires.

        Matches the mission name to a known quest in the user's library and
        starts tracking it as if the user clicked Start.
        """
        quest = self.match_quest_by_mission_name(mission_name)
        if not quest:
            log.info(
                "Mission received '%s' — no matching quest in DB, ignoring",
                mission_name,
            )
            return
        if quest.get("started_at"):
            log.debug("Quest '%s' already started, skipping auto-start", quest["name"])
            return
        self.start_quest(quest["id"])
        log.info(
            "Started quest '%s' (id=%d) from chat.log mission '%s'",
            quest["name"],
            quest["id"],
            mission_name,
        )
        self._record_notable_event("quest_started", quest["name"], 0)

    def quest_reward_filter(
        self,
        mission_name: str,
        loot_items: list[dict],
        skill_gains: list[dict],
    ) -> dict | None:
        """Called by the chatlog watcher when a tick contains MISSION_COMPLETE.

        Matches the mission against known quests, auto-completes it, and
        returns which loot item or skill gain to suppress from tracking.

        Args:
            mission_name: From ``Mission completed (<name>)``
            loot_items:   ``[{"item_name", "quantity", "value"}, ...]``
            skill_gains:  ``[{"skill_name", "amount"}, ...]``

        Returns:
            None — no match or no suppression needed.
            ``{"suppress_loot_index": int | None, "suppress_skill_index": int | None}``
        """
        quest = self.match_quest_by_mission_name(mission_name)
        if not quest:
            log.info(
                "Mission '%s' — no matching quest in DB, no suppression", mission_name
            )
            return None

        # Auto-complete the quest (clears in-progress state, records the
        # reward in either ledger_entries or quest_claims based on
        # reward_is_skill, and records the session-quest completion).
        self.complete_quest(quest["id"])
        log.info(
            "Auto-completed quest '%s' (id=%d) from chat.log mission '%s'",
            quest["name"],
            quest["id"],
            mission_name,
        )

        reward_ped = quest.get("reward_ped")
        is_skill = bool(quest.get("reward_is_skill"))
        result = None
        suppressed_desc = None

        if is_skill:
            # The in-game skill_gain pop-up is the same PES reward we just
            # recorded as a quest_claims row; suppress it so the skill_gains
            # ledger doesn't double-count.
            if skill_gains:
                result = {"suppress_loot_index": None, "suppress_skill_index": 0}
                suppressed_desc = f"skill reward suppressed"
        elif reward_ped is not None and loot_items:
            if reward_ped > 0:
                best_idx = None
                best_diff = float("inf")
                for i, item in enumerate(loot_items):
                    diff = abs(item.get("value", 0.0) - reward_ped)
                    if diff < best_diff and diff <= 0.02:
                        best_diff = diff
                        best_idx = i
                if best_idx is not None:
                    result = {
                        "suppress_loot_index": best_idx,
                        "suppress_skill_index": None,
                    }
                    item_name = loot_items[best_idx].get("item_name", "?")
                    suppressed_desc = f"{item_name} ({reward_ped:.2f} PED) suppressed"
                else:
                    log.warning(
                        "Quest '%s' reward %.2f PED — no matching loot item in tick (items: %s)",
                        quest["name"],
                        reward_ped,
                        [(i.get("item_name"), i.get("value")) for i in loot_items],
                    )
            else:
                min_idx = min(
                    range(len(loot_items)),
                    key=lambda i: loot_items[i].get("value", 0.0),
                )
                result = {"suppress_loot_index": min_idx, "suppress_skill_index": None}
                item_name = loot_items[min_idx].get("item_name", "?")
                suppressed_desc = f"{item_name} suppressed"

        # Record overlay event. Skill quest rewards get a PES-flavoured event
        # type so the SessionDetail card renders the unit correctly.
        desc = quest["name"]
        if suppressed_desc:
            desc += f": {suppressed_desc}"
        event_type = "quest_completed_pes" if is_skill else "quest_completed"
        self._record_notable_event(event_type, desc, reward_ped or 0)

        return result

    def _record_notable_event(
        self, event_type: str, description: str, value_ped: float
    ) -> None:
        """Insert a notable event for the overlay if a tracking session is active."""
        if not self._current_session_id:
            return
        try:
            self._conn.execute(
                """INSERT INTO notable_events
                   (session_id, kill_id, event_type, mob_or_item, value_ped, timestamp)
                   VALUES (?, NULL, ?, ?, ?, ?)""",
                (
                    self._current_session_id,
                    event_type,
                    description,
                    value_ped,
                    time.time(),
                ),
            )
            self._conn.commit()
        except Exception:
            log.debug("Could not record notable event")

    def _record_session_completion(
        self,
        session_id: str | None,
        quest_id: int,
        completed_at: float | None = None,
    ) -> None:
        """Record a quest completion for cooldown and analytics purposes.

        When a tracking session is active, the row is keyed by that session
        so curated analytics linkage can aggregate per session. When no
        session is active, a synthetic `manual-<uuid>` key is used so the
        completion still contributes to the derived cooldown without
        colliding with subsequent manual completions of the same quest.
        """
        key = session_id if session_id is not None else f"manual-{uuid.uuid4()}"
        ts = completed_at if completed_at is not None else time.time()
        self._conn.execute(
            "INSERT OR IGNORE INTO session_quest_completions (session_id, quest_id, completed_at) VALUES (?, ?, ?)",
            (key, quest_id, ts),
        )
        self._conn.commit()
        if session_id is not None:
            log.info(
                "Recorded quest %d completion in session %s", quest_id, session_id[:8]
            )
        else:
            log.info("Recorded manual completion for quest %d", quest_id)

    def _get_session_completed_quest_ids(self, session_id: str) -> list[int]:
        rows = self._conn.execute(
            """SELECT DISTINCT quest_id
               FROM session_quest_completions
               WHERE session_id = ?
               ORDER BY quest_id""",
            (session_id,),
        ).fetchall()
        return [int(r[0]) for r in rows]

    def _get_session_analytics_link_row(self, session_id: str):
        return self._conn.execute(
            """SELECT session_id, link_type, quest_id, playlist_id
               FROM session_quest_analytics_links
               WHERE session_id = ?""",
            (session_id,),
        ).fetchone()

    def _set_session_analytics_link(
        self,
        session_id: str,
        link_type: str,
        *,
        quest_id: int | None = None,
        playlist_id: int | None = None,
    ) -> None:
        self._conn.execute(
            """INSERT INTO session_quest_analytics_links (session_id, link_type, quest_id, playlist_id, linked_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(session_id) DO UPDATE SET
                   link_type = excluded.link_type,
                   quest_id = excluded.quest_id,
                   playlist_id = excluded.playlist_id,
                   linked_at = excluded.linked_at""",
            (session_id, link_type, quest_id, playlist_id, time.time()),
        )
        self._conn.commit()

    def _get_quest_name(self, quest_id: int | None) -> str | None:
        if quest_id is None:
            return None
        row = self._conn.execute(
            "SELECT name FROM quests WHERE id = ?",
            (quest_id,),
        ).fetchone()
        return row[0] if row else None

    def _get_playlist_name(self, playlist_id: int | None) -> str | None:
        if playlist_id is None:
            return None
        row = self._conn.execute(
            "SELECT name FROM quest_playlists WHERE id = ?",
            (playlist_id,),
        ).fetchone()
        return row[0] if row else None

    def _delete_latest_quest_claim(self, quest_id: int) -> bool:
        row = self._conn.execute(
            """SELECT id FROM quest_claims
               WHERE quest_id = ?
               ORDER BY claimed_at DESC, id DESC
               LIMIT 1""",
            (quest_id,),
        ).fetchone()
        if not row:
            return False
        self._conn.execute("DELETE FROM quest_claims WHERE id = ?", (row[0],))
        return True

    def _delete_latest_quest_reward_entry(
        self, quest_name: str, reward_ped: float
    ) -> bool:
        row = self._conn.execute(
            """SELECT id FROM ledger_entries
               WHERE type = 'markup'
                 AND tag = 'quest_reward'
                 AND description = ?
                 AND amount = ?
               ORDER BY date DESC, id DESC
               LIMIT 1""",
            (f"Quest: {quest_name}", reward_ped),
        ).fetchone()
        if not row:
            return False
        self._conn.execute("DELETE FROM ledger_entries WHERE id = ?", (row[0],))
        return True

    # ── Mob autocomplete ─────────────────────────────────────────────────────

    def get_all_mob_names(self) -> list[str]:
        """All distinct mob names across active quests, for autocomplete."""
        rows = self._conn.execute(
            """SELECT DISTINCT qm.mob_name FROM quest_mobs qm
               JOIN quests q ON q.id = qm.quest_id
               WHERE q.is_active = 1
               ORDER BY qm.mob_name"""
        ).fetchall()
        return [r[0] for r in rows]

    # ── Internal helpers ─────────────────────────────────────────────────────

    def _normalize_db_id(self, value):
        """Normalise DB id values from older rows/adapters into SQLite-safe scalars."""
        if isinstance(value, memoryview):
            value = value.tobytes()
        if isinstance(value, (bytes, bytearray)):
            value = bytes(value).decode("utf-8", errors="strict")
        if isinstance(value, str):
            stripped = value.strip()
            if stripped.isdigit():
                return int(stripped)
            return stripped
        return (
            int(value)
            if isinstance(value, bool) or hasattr(value, "__int__")
            else value
        )

    def _scalar_from_row(self, row, key: str):
        """Extract a single scalar from varying sqlite row shapes, or None if malformed."""
        if row is None:
            return None
        if isinstance(row, dict):
            return row.get(key)
        if hasattr(row, "keys"):
            try:
                if key in row.keys():
                    return row[key]
            except Exception:
                pass
        try:
            return row[0]
        except (IndexError, KeyError, TypeError):
            return None

    def _row_to_quest(self, row) -> dict:
        """Convert a sqlite3.Row to a quest dict with computed cooldown fields."""
        d = dict(row)
        if "id" in d:
            d["id"] = self._normalize_db_id(d["id"])
        # Compute cooldown_expires_at from last_completed_at + cooldown_hours
        last = d.get("last_completed_at")
        cd_hours = d.get("cooldown_hours")
        if last is not None and cd_hours is not None and cd_hours > 0:
            expires_ts = last + cd_hours * 3600
            d["cooldown_expires_at"] = datetime.fromtimestamp(
                expires_ts, tz=timezone.utc
            ).isoformat()
        else:
            d["cooldown_expires_at"] = None
        return d

    def _is_quest_cooling(self, quest: dict) -> bool:
        last = quest.get("last_completed_at")
        cd_hours = quest.get("cooldown_hours")
        if last is None or cd_hours is None or cd_hours <= 0:
            return False
        return (last + cd_hours * 3600) > time.time()

    def _row_to_playlist(self, row) -> dict:
        d = dict(row)
        if "id" in d:
            d["id"] = self._normalize_db_id(d["id"])
        return d

    def _normalize_expected_reward_markup(
        self,
        reward_ped: float | None,
        reward_is_skill: bool | int | None,
        expected_markup: float | None,
    ) -> float | None:
        if (
            reward_is_skill
            or reward_ped is None
            or reward_ped <= 0
            or expected_markup is None
        ):
            return None
        return float(expected_markup)

    def _expected_reward_total(
        self,
        reward_ped: float,
        reward_is_skill: bool,
        expected_markup: float | None,
        completions: int,
    ) -> float:
        if completions <= 0:
            return 0
        if reward_is_skill or reward_ped <= 0 or expected_markup is None:
            return reward_ped * completions
        return reward_ped * (expected_markup / 100.0) * completions

    def _get_quest_mobs(self, quest_id: int) -> list[str]:
        rows = self._conn.execute(
            "SELECT mob_name FROM quest_mobs WHERE quest_id = ? ORDER BY mob_name",
            (quest_id,),
        ).fetchall()
        mobs = []
        for row in rows:
            mob_name = self._scalar_from_row(row, "mob_name")
            if isinstance(mob_name, str) and mob_name:
                mobs.append(mob_name)
        return mobs

    def _set_quest_mobs(self, quest_id: int, mobs: list[str]) -> None:
        self._conn.execute("DELETE FROM quest_mobs WHERE quest_id = ?", (quest_id,))
        for mob in mobs:
            mob = mob.strip()
            if mob:
                self._conn.execute(
                    "INSERT OR IGNORE INTO quest_mobs (quest_id, mob_name) VALUES (?, ?)",
                    (quest_id, mob),
                )

    def _get_quest_playlist_ids(self, quest_id: int) -> list[int]:
        rows = self._conn.execute(
            """SELECT DISTINCT qpi.playlist_id FROM quest_playlist_items qpi
               JOIN quest_playlists qp ON qp.id = qpi.playlist_id
               WHERE qpi.quest_id = ? AND qp.is_active = 1""",
            (quest_id,),
        ).fetchall()
        playlist_ids = []
        for row in rows:
            playlist_id = self._scalar_from_row(row, "playlist_id")
            if playlist_id is not None:
                playlist_ids.append(self._normalize_db_id(playlist_id))
        return playlist_ids

    def _get_playlist_quest_ids(
        self, playlist_id: int, group_type: str | None = None
    ) -> list[int]:
        sql = "SELECT quest_id FROM quest_playlist_items WHERE playlist_id = ?"
        params: list[object] = [playlist_id]
        if group_type is not None:
            sql += " AND group_type = ?"
            params.append(group_type)
        sql += " ORDER BY sort_order"
        rows = self._conn.execute(sql, params).fetchall()
        quest_ids = []
        for row in rows:
            quest_id = self._scalar_from_row(row, "quest_id")
            if quest_id is not None:
                quest_ids.append(self._normalize_db_id(quest_id))
        return quest_ids

    def _get_playlist_items(self, playlist_id: int) -> list[dict]:
        """Get playlist items with quest_id, description, and group type."""
        rows = self._conn.execute(
            """SELECT quest_id, description, group_type
               FROM quest_playlist_items
               WHERE playlist_id = ?
               ORDER BY group_type = ?, sort_order""",
            (playlist_id, PLAYLIST_GROUP_LONG_HORIZON),
        ).fetchall()
        return [
            {"quest_id": r[0], "description": r[1], "group_type": r[2]} for r in rows
        ]

    def _set_playlist_items(self, playlist_id: int, items: list) -> None:
        """Set playlist items with explicit grouping."""
        self._conn.execute(
            "DELETE FROM quest_playlist_items WHERE playlist_id = ?",
            (playlist_id,),
        )
        for i, item in enumerate(items):
            if isinstance(item, dict):
                qid = item["quest_id"]
                desc = item.get("description")
                group_type = item.get("group_type", PLAYLIST_GROUP_IMMEDIATE)
            else:
                qid = item
                desc = None
                group_type = PLAYLIST_GROUP_IMMEDIATE
            if group_type not in {
                PLAYLIST_GROUP_IMMEDIATE,
                PLAYLIST_GROUP_LONG_HORIZON,
            }:
                raise ValueError(f"Invalid playlist group type: {group_type}")
            self._conn.execute(
                """INSERT INTO quest_playlist_items
                   (playlist_id, quest_id, sort_order, description, group_type)
                   VALUES (?, ?, ?, ?, ?)""",
                (playlist_id, qid, i, desc, group_type),
            )

    def _normalize_playlist_items(self, data: dict) -> list[dict]:
        """Normalise playlist payloads to classified items."""
        if data.get("items") is not None:
            return [
                {
                    "quest_id": item["quest_id"],
                    "description": item.get("description"),
                    "group_type": item.get("group_type", PLAYLIST_GROUP_IMMEDIATE),
                }
                for item in data["items"]
            ]
        return [
            {
                "quest_id": quest_id,
                "description": None,
                "group_type": PLAYLIST_GROUP_IMMEDIATE,
            }
            for quest_id in data.get("quest_ids", [])
        ]

    def _split_playlist_item_groups(
        self, items: list[dict]
    ) -> tuple[list[int], list[int]]:
        immediate_ids = [
            i["quest_id"]
            for i in items
            if i.get("group_type") != PLAYLIST_GROUP_LONG_HORIZON
        ]
        long_horizon_ids = [
            i["quest_id"]
            for i in items
            if i.get("group_type") == PLAYLIST_GROUP_LONG_HORIZON
        ]
        return immediate_ids, long_horizon_ids
