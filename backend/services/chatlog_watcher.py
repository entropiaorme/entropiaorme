"""Chat.log file watcher — tails the log and publishes parsed events.

Reads new lines from Entropia Universe's chat.log and publishes them
as typed events on the event bus for the tracker to consume.

Event contract: chat.log timestamps have one-second precision, so all
recognised lines with the same timestamp are treated as one app tick.
When the timestamp advances, or the file goes idle, the tick is closed:

  • Loot lines become one EVENT_LOOT_GROUP.
  • If the tick contains a MISSION_COMPLETE, the quest-reward filter
    callback is invoked.  It may suppress one loot item or skill gain
    from the tick to prevent double-counting quest rewards that also
    appear in the ledger.
"""

import logging
import os
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Callable

from backend.core.event_bus import EventBus
from backend.core.events import (
    EVENT_COMBAT,
    EVENT_LOOT_GROUP,
    EVENT_SKILL_GAIN,
    EVENT_ENHANCER_BREAK,
    EVENT_GLOBAL,
    EVENT_MISSION_RECEIVED,
)
from backend.services.chatlog_parser import ChatEvent, EventType, parse_line

log = logging.getLogger(__name__)

# Map parser EventTypes to event bus event types
_EVENT_MAP: dict[EventType, str] = {
    # Combat
    EventType.DAMAGE_DEALT: EVENT_COMBAT,
    EventType.CRITICAL_HIT: EVENT_COMBAT,
    EventType.DAMAGE_RECEIVED: EVENT_COMBAT,
    EventType.TARGET_DODGE: EVENT_COMBAT,
    EventType.TARGET_EVADE: EVENT_COMBAT,
    EventType.TARGET_JAM: EVENT_COMBAT,
    EventType.PLAYER_DODGE: EVENT_COMBAT,
    EventType.PLAYER_EVADE: EVENT_COMBAT,
    EventType.PLAYER_JAM: EVENT_COMBAT,
    EventType.MOB_MISS: EVENT_COMBAT,
    EventType.DEFLECT: EVENT_COMBAT,
    EventType.SELF_HEAL: EVENT_COMBAT,
    # Loot
    EventType.LOOT: EVENT_LOOT_GROUP,
    # Skills
    EventType.SKILL_GAIN: EVENT_SKILL_GAIN,
    # Equipment
    EventType.ENHANCER_BREAK: EVENT_ENHANCER_BREAK,
    # Globals
    EventType.GLOBAL_KILL: EVENT_GLOBAL,
    EventType.HOF_KILL: EVENT_GLOBAL,
    EventType.GLOBAL_ITEM: EVENT_GLOBAL,
    EventType.HOF_ITEM: EVENT_GLOBAL,
    # Missions
    EventType.MISSION_RECEIVED: EVENT_MISSION_RECEIVED,
}

# Event types that need tick buffering for internal processing but do not
# publish on the event bus (e.g. MISSION_COMPLETE drives the quest-reward
# filter callback without broadcasting to external subscribers).
_INTERNAL_BUFFER_TYPES: frozenset[EventType] = frozenset(
    {
        EventType.MISSION_COMPLETE,
    }
)

TAIL_INTERVAL = 0.1  # seconds between reads
PERF_LOG_INTERVAL_S = 15.0

_COMBAT_MESSAGE_PREFIXES = (
    "Critical hit",
    "You inflicted",
    "The target Jammed",
    "The target Dodged",
    "The target Evaded",
    "You took",
    "Damage deflected",
    "You Evaded",
    "You Dodged",
    "You Jammed",
    "The attack missed",
    "You healed",
)

# Type alias for the quest-reward filter callback.
# Receives (mission_name, loot_items, skill_gains) → suppression dict or None.
QuestRewardFilter = Callable[
    [str, list[dict], list[dict]],
    dict | None,
]


class ChatlogWatcher:
    """Tails chat.log and publishes parsed events to the event bus.

    Events are buffered by timestamp ("tick").  When a tick closes, all
    events in that tick are emitted together — loot items grouped into a
    single EVENT_LOOT_GROUP, other events individually.

    If a tick contains a MISSION_COMPLETE and a ``quest_reward_filter``
    callback is installed, the callback may suppress one loot item or
    skill gain from the tick to prevent quest-reward double-counting.
    """

    def __init__(
        self,
        event_bus: EventBus,
        chatlog_path: str | Path,
        quest_reward_filter: QuestRewardFilter | None = None,
    ):
        self._event_bus = event_bus
        self._path = Path(chatlog_path)
        self._running = False
        self._thread: threading.Thread | None = None
        self._quest_reward_filter = quest_reward_filter

        # Tick buffer — accumulates all events sharing one timestamp
        self._tick_ts: datetime | None = None
        self._tick_events: list[ChatEvent] = []

        # Rate-limited watcher perf counters. These stay debug-only.
        self._perf_window_started = time.monotonic()
        self._perf_lines_seen = 0
        self._perf_lines_skipped = 0
        self._perf_events_parsed = 0
        self._perf_flushes = 0
        self._perf_parse_seconds = 0.0
        self._perf_max_tick_size = 0

    @property
    def is_running(self) -> bool:
        return self._running

    def start(self) -> None:
        """Start tailing chat.log in a background thread."""
        if self._running:
            return
        if not self._path.is_file():
            log.warning("Chat.log not found: %s — watcher not started", self._path)
            return

        self._running = True
        self._thread = threading.Thread(
            target=self._tail_loop,
            daemon=True,
            name="chatlog-watcher",
        )
        self._thread.start()
        log.info("Started watching: %s", self._path)

    def stop(self) -> None:
        """Stop the watcher."""
        self._running = False
        if self._thread:
            self._thread.join(timeout=2)
            self._thread = None
        log.info("Stopped")

    def restart(self, new_path: str | Path) -> None:
        """Stop, update the path, and start again."""
        self.stop()
        self._path = Path(new_path)
        self._tick_ts = None
        self._tick_events = []
        self.start()

    def _tail_loop(self) -> None:
        """Main loop: seek to end, then read new lines as they appear."""
        try:
            with open(self._path, "r", encoding="utf-8", errors="replace") as f:
                # Seek to end — don't replay history
                f.seek(0, os.SEEK_END)

                while self._running:
                    line = f.readline()
                    if line:
                        self._process_line(line)
                    else:
                        self._flush_tick()
                        time.sleep(TAIL_INTERVAL)

                # Flush any remaining tick on shutdown
                self._flush_tick()
        except Exception as e:
            log.error("Watcher error: %s", e)
            self._running = False

    def _process_line(self, line: str) -> None:
        """Parse a line and add it to the current tick buffer.

        Unparsed / unmapped lines are silently skipped — they do NOT
        break the tick boundary (chat messages from other channels can
        interleave with System events at the same timestamp).
        """
        self._perf_lines_seen += 1
        if self._can_skip_idle_combat_line(line):
            self._perf_lines_skipped += 1
            self._maybe_log_perf_summary()
            return

        debug_enabled = log.isEnabledFor(logging.DEBUG)
        parse_started = time.perf_counter() if debug_enabled else 0.0
        event = parse_line(line)
        if debug_enabled:
            self._perf_parse_seconds += time.perf_counter() - parse_started
        if not event:
            self._maybe_log_perf_summary()
            return

        if event.type not in _EVENT_MAP and event.type not in _INTERNAL_BUFFER_TYPES:
            self._maybe_log_perf_summary()
            return

        # New timestamp → close previous tick first
        if self._tick_ts is not None and event.timestamp != self._tick_ts:
            self._flush_tick()

        self._tick_ts = event.timestamp
        self._tick_events.append(event)
        self._perf_events_parsed += 1
        self._perf_max_tick_size = max(self._perf_max_tick_size, len(self._tick_events))
        self._maybe_log_perf_summary()

    def _can_skip_idle_combat_line(self, line: str) -> bool:
        """Fast-path: skip combat parsing when nobody subscribes to combat events."""
        if self._event_bus.has_subscribers(EVENT_COMBAT):
            return False
        marker = "[System] [] "
        idx = line.find(marker)
        if idx == -1:
            return False
        msg = line[idx + len(marker) :]
        return msg.startswith(_COMBAT_MESSAGE_PREFIXES)

    def _flush_tick(self) -> None:
        """Emit all buffered events for the current tick, then reset.

        Loot items are grouped into a single EVENT_LOOT_GROUP.
        If the tick contains MISSION_COMPLETE, the quest-reward filter
        is called to suppress the matching reward item/gain.
        """
        if not self._tick_events:
            return
        self._perf_flushes += 1

        # Partition events by category
        loot_events: list[ChatEvent] = []
        skill_events: list[ChatEvent] = []
        mission_events: list[ChatEvent] = []
        enhancer_events: list[ChatEvent] = []
        other_events: list[ChatEvent] = []

        for event in self._tick_events:
            if event.type == EventType.LOOT:
                loot_events.append(event)
            elif event.type == EventType.SKILL_GAIN:
                skill_events.append(event)
            elif event.type in (EventType.MISSION_COMPLETE, EventType.MISSION_RECEIVED):
                mission_events.append(event)
            elif event.type == EventType.ENHANCER_BREAK:
                enhancer_events.append(event)
            else:
                other_events.append(event)

        # ── Quest-reward suppression ────────────────────────────────────────
        mission_completes = [
            e for e in mission_events if e.type == EventType.MISSION_COMPLETE
        ]

        if mission_completes and self._quest_reward_filter:
            for mc in mission_completes:
                mission_name = mc.data["mission_name"]
                loot_data = [
                    {
                        "item_name": e.data.get("item_name", ""),
                        "quantity": e.data.get("quantity", 1),
                        "value": e.data.get("value", 0.0),
                    }
                    for e in loot_events
                ]
                skill_data = [
                    {
                        "skill_name": e.data.get("skill_name", ""),
                        "amount": e.data.get("amount", 0.0),
                    }
                    for e in skill_events
                ]

                try:
                    result = self._quest_reward_filter(
                        mission_name, loot_data, skill_data
                    )
                except Exception:
                    log.exception("Quest reward filter error for '%s'", mission_name)
                    result = None

                if result:
                    li = result.get("suppress_loot_index")
                    if li is not None and 0 <= li < len(loot_events):
                        suppressed = loot_events.pop(li)
                        log.info(
                            "QUEST SUPPRESSED loot: %s (%.4f PED) for mission '%s'",
                            suppressed.data.get("item_name"),
                            suppressed.data.get("value", 0),
                            mission_name,
                        )
                    si = result.get("suppress_skill_index")
                    if si is not None and 0 <= si < len(skill_events):
                        suppressed = skill_events.pop(si)
                        log.info(
                            "QUEST SUPPRESSED skill: %s +%.4f for mission '%s'",
                            suppressed.data.get("skill_name"),
                            suppressed.data.get("amount", 0),
                            mission_name,
                        )

        refund_matches = self._match_enhancer_shrapnel(loot_events, enhancer_events)

        # ── Emit enhancer breaks before loot finalisation ───────────────────
        for event in enhancer_events:
            self._event_bus.publish(
                EVENT_ENHANCER_BREAK,
                {
                    "type": event.type.value,
                    "timestamp": event.timestamp,
                    **event.data,
                },
            )

        # ── Emit loot group ─────────────────────────────────────────────────
        if loot_events:
            items = []
            total = 0.0
            for idx, e in enumerate(loot_events):
                val = e.data.get("value", 0.0)
                items.append(
                    {
                        "item_name": e.data.get("item_name", ""),
                        "quantity": e.data.get("quantity", 1),
                        "value_ped": val,
                        "is_enhancer_shrapnel": refund_matches[idx],
                    }
                )
                total += val
            self._event_bus.publish(
                EVENT_LOOT_GROUP,
                {
                    "type": EventType.LOOT.value,
                    "timestamp": self._tick_ts,
                    "items": items,
                    "total_ped": round(total, 4),
                },
            )

        # ── Emit mission events ─────────────────────────────────────────────
        for event in mission_events:
            bus_event = _EVENT_MAP.get(event.type)
            if not bus_event:
                continue
            self._event_bus.publish(
                bus_event,
                {
                    "type": event.type.value,
                    "timestamp": event.timestamp,
                    **event.data,
                },
            )

        # ── Emit skill events ───────────────────────────────────────────────
        for event in skill_events:
            self._event_bus.publish(
                EVENT_SKILL_GAIN,
                {
                    "type": event.type.value,
                    "timestamp": event.timestamp,
                    **event.data,
                },
            )

        # ── Emit everything else ────────────────────────────────────────────
        for event in other_events:
            bus_event = _EVENT_MAP.get(event.type)
            if bus_event:
                self._event_bus.publish(
                    bus_event,
                    {
                        "type": event.type.value,
                        "timestamp": event.timestamp,
                        **event.data,
                    },
                )

        # Reset tick
        self._tick_ts = None
        self._tick_events = []
        self._maybe_log_perf_summary()

    def _maybe_log_perf_summary(self) -> None:
        if not log.isEnabledFor(logging.DEBUG):
            return
        now = time.monotonic()
        elapsed = now - self._perf_window_started
        if elapsed < PERF_LOG_INTERVAL_S:
            return
        log.debug(
            "Chatlog watcher perf: %.1fs lines=%d parsed=%d skipped=%d flushes=%d max_tick=%d parse_ms=%.1f",
            elapsed,
            self._perf_lines_seen,
            self._perf_events_parsed,
            self._perf_lines_skipped,
            self._perf_flushes,
            self._perf_max_tick_size,
            self._perf_parse_seconds * 1000.0,
        )
        self._perf_window_started = now
        self._perf_lines_seen = 0
        self._perf_lines_skipped = 0
        self._perf_events_parsed = 0
        self._perf_flushes = 0
        self._perf_parse_seconds = 0.0
        self._perf_max_tick_size = 0

    @staticmethod
    def _match_enhancer_shrapnel(
        loot_events: list[ChatEvent],
        enhancer_events: list[ChatEvent],
    ) -> list[bool]:
        """Flag same-tick shrapnel loot that matches enhancer refund values."""
        matches = [False] * len(loot_events)
        refunds = [
            float(event.data.get("shrapnel_ped", 0.0) or 0.0)
            for event in enhancer_events
            if float(event.data.get("shrapnel_ped", 0.0) or 0.0) > 0
        ]

        for refund_ped in refunds:
            for idx, loot_event in enumerate(loot_events):
                if matches[idx]:
                    continue
                if str(loot_event.data.get("item_name", "")).lower() != "shrapnel":
                    continue
                loot_ped = float(loot_event.data.get("value", 0.0) or 0.0)
                if abs(loot_ped - refund_ped) < 1e-9:
                    matches[idx] = True
                    break

        return matches
