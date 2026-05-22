"""Parse Entropia Universe chat.log lines into app events."""

from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from html import unescape
from pathlib import Path
from typing import Any, Callable


class EventType(Enum):
    DAMAGE_DEALT = "damage_dealt"
    CRITICAL_HIT = "critical_hit"
    TARGET_DODGE = "target_dodge"
    TARGET_EVADE = "target_evade"
    TARGET_JAM = "target_jam"
    DAMAGE_RECEIVED = "damage_received"
    PLAYER_DODGE = "player_dodge"
    PLAYER_EVADE = "player_evade"
    PLAYER_JAM = "player_jam"
    MOB_MISS = "mob_miss"
    DEFLECT = "deflect"
    SELF_HEAL = "self_heal"
    LOOT = "loot"
    SKILL_GAIN = "skill_gain"
    ENHANCER_BREAK = "enhancer_break"
    GLOBAL_KILL = "global_kill"
    HOF_KILL = "hof_kill"
    GLOBAL_ITEM = "global_item"
    HOF_ITEM = "hof_item"
    MISSION_COMPLETE = "mission_complete"
    MISSION_RECEIVED = "mission_received"


@dataclass(frozen=True)
class ChatEvent:
    type: EventType
    timestamp: datetime
    data: dict[str, Any]
    raw_line: str


Extractor = Callable[[re.Match[str]], dict[str, Any]]


@dataclass(frozen=True)
class _Rule:
    event_type: EventType
    pattern: re.Pattern[str]
    extract: Extractor
    prefix: str | None = None

    def match(self, text: str) -> tuple[EventType, dict[str, Any]] | None:
        if self.prefix is not None and not text.startswith(self.prefix):
            return None
        matched = self.pattern.search(text)
        if matched is None:
            return None
        return self.event_type, self.extract(matched)


LINE_RE = re.compile(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (.+)$")
QUANTITY_RE = re.compile(r"^(.+?)\s+x\s+\((\d+)\)$")

SYSTEM_MARKER = "[System] []"
GLOBALS_MARKER = "[Globals]"


def _amount(group: int = 1) -> Extractor:
    return lambda match: {"amount": float(match.group(group))}


SYSTEM_RULES: tuple[_Rule, ...] = (
    _Rule(
        EventType.CRITICAL_HIT,
        re.compile(r"Critical hit - Additional damage! You inflicted ([\d.]+) points of damage"),
        _amount(),
        "Critical hit",
    ),
    _Rule(
        EventType.DAMAGE_DEALT,
        re.compile(r"You inflicted ([\d.]+) points of damage"),
        _amount(),
        "You inflicted",
    ),
    _Rule(EventType.TARGET_JAM, re.compile(r"The target Jammed your attack"), lambda _: {}, "The target Jammed"),
    _Rule(EventType.TARGET_DODGE, re.compile(r"The target Dodged your attack"), lambda _: {}, "The target Dodged"),
    _Rule(EventType.TARGET_EVADE, re.compile(r"The target Evaded your attack"), lambda _: {}, "The target Evaded"),
    _Rule(EventType.DAMAGE_RECEIVED, re.compile(r"You took ([\d.]+) points of damage"), _amount(), "You took"),
    _Rule(EventType.DEFLECT, re.compile(r"Damage deflected!"), lambda _: {}, "Damage deflected"),
    _Rule(EventType.PLAYER_EVADE, re.compile(r"You Evaded the attack"), lambda _: {}, "You Evaded"),
    _Rule(EventType.PLAYER_DODGE, re.compile(r"You Dodged the attack"), lambda _: {}, "You Dodged"),
    _Rule(EventType.PLAYER_JAM, re.compile(r"You Jammed the attack"), lambda _: {}, "You Jammed"),
    _Rule(EventType.MOB_MISS, re.compile(r"The attack missed you"), lambda _: {}, "The attack missed"),
    _Rule(EventType.SELF_HEAL, re.compile(r"You healed yourself ([\d.]+) points"), _amount(), "You healed"),
    _Rule(
        EventType.ENHANCER_BREAK,
        re.compile(
            r"Your enhancer (.+?) on your (.+?) broke\. "
            r"You have (\d+) enhancers remaining on the item\. "
            r"You received ([\d.]+) PED Shrapnel\.\s*"
        ),
        lambda match: {
            "enhancer_name": match.group(1),
            "item_name": match.group(2),
            "remaining": int(match.group(3)),
            "shrapnel_ped": float(match.group(4)),
        },
        "Your enhancer",
    ),
    _Rule(
        EventType.MISSION_COMPLETE,
        re.compile(r"^Mission completed \((.+)\)$"),
        lambda match: {"mission_name": match.group(1).strip()},
        "Mission completed",
    ),
    _Rule(
        EventType.MISSION_RECEIVED,
        re.compile(r"^New Mission received \((.+)\)$"),
        lambda match: {"mission_name": match.group(1).strip()},
        "New Mission received",
    ),
    _Rule(
        EventType.SKILL_GAIN,
        re.compile(r"^You have gained ([\d.]+) experience in your (.+) skill$"),
        lambda match: {"amount": float(match.group(1)), "skill_name": match.group(2).strip()},
        "You have gained",
    ),
    _Rule(
        EventType.SKILL_GAIN,
        re.compile(r"^You have gained ([\d.]+) ([A-Z][A-Za-z ]+)$"),
        lambda match: {"amount": float(match.group(1)), "skill_name": match.group(2).strip()},
        "You have gained",
    ),
    _Rule(
        EventType.SKILL_GAIN,
        re.compile(r"^Your ([A-Z][a-z]+) has improved by ([\d.]+)$"),
        lambda match: {"amount": float(match.group(2)), "skill_name": match.group(1)},
        "Your ",
    ),
)

LOOT_RE = re.compile(r"\[System\] \[\] You received (.+?) Value: ([\d.]+) PED")

GLOBAL_RULES: tuple[_Rule, ...] = (
    _Rule(
        EventType.HOF_KILL,
        re.compile(
            r"\[Globals\] \[\] (.+?) killed a creature \((.+?)\) with a value of ([\d.]+) PED! "
            r"A record has been added to the Hall of Fame!"
        ),
        lambda match: {"player": match.group(1), "creature": match.group(2), "value": float(match.group(3))},
    ),
    _Rule(
        EventType.GLOBAL_KILL,
        re.compile(r"\[Globals\] \[\] (.+?) killed a creature \((.+?)\) with a value of ([\d.]+) PED!"),
        lambda match: {"player": match.group(1), "creature": match.group(2), "value": float(match.group(3))},
    ),
    _Rule(
        EventType.HOF_ITEM,
        re.compile(
            r"\[Globals\] \[\] (.+?) has found a rare item \((.+?)\) with a value of ([\d.]+) PE[CD]! "
            r"A record has been added to the Hall of Fame!"
        ),
        lambda match: {"player": match.group(1), "item": match.group(2), "value": float(match.group(3))},
    ),
    _Rule(
        EventType.GLOBAL_ITEM,
        re.compile(
            r"\[Globals\] \[\] (.+?) has found a rare item \((.+?)\) with a value of ([\d.]+) PE[CD]!"
            r"(?! A record)"
        ),
        lambda match: {"player": match.group(1), "item": match.group(2), "value": float(match.group(3))},
    ),
)


def parse_line(line: str) -> ChatEvent | None:
    matched = LINE_RE.match(line.strip())
    if matched is None:
        return None

    timestamp = datetime.strptime(matched.group(1), "%Y-%m-%d %H:%M:%S")
    content = unescape(matched.group(2)) if "&" in matched.group(2) else matched.group(2)

    if SYSTEM_MARKER in content:
        return _parse_system(timestamp, content, line)
    if GLOBALS_MARKER in content:
        return _parse_global(timestamp, content, line)
    return None


def parse_file(path: str | Path) -> list[ChatEvent]:
    events: list[ChatEvent] = []
    with Path(path).open("r", encoding="utf-8", errors="replace") as file:
        for line in file:
            event = parse_line(line)
            if event is not None:
                events.append(event)
    return events


def _parse_system(timestamp: datetime, content: str, raw_line: str) -> ChatEvent | None:
    message = _message(content)
    loot_match = LOOT_RE.search(content)
    if loot_match is not None:
        return ChatEvent(EventType.LOOT, timestamp, _loot_data(loot_match), raw_line)

    for rule in SYSTEM_RULES:
        result = rule.match(message)
        if result is not None:
            event_type, data = result
            return ChatEvent(event_type, timestamp, data, raw_line)
    return None


def _parse_global(timestamp: datetime, content: str, raw_line: str) -> ChatEvent | None:
    for rule in GLOBAL_RULES:
        result = rule.match(content)
        if result is not None:
            event_type, data = result
            return ChatEvent(event_type, timestamp, data, raw_line)
    return None


def _message(content: str) -> str:
    parts = content.split("] ", 2)
    return parts[-1] if len(parts) == 3 else content


def _loot_data(match: re.Match[str]) -> dict[str, Any]:
    raw_name = match.group(1).strip()
    quantity_match = QUANTITY_RE.match(raw_name)
    if quantity_match is None:
        return {"item_name": raw_name, "quantity": 1, "value": float(match.group(2))}
    return {
        "item_name": quantity_match.group(1).strip(),
        "quantity": int(quantity_match.group(2)),
        "value": float(match.group(2)),
    }
