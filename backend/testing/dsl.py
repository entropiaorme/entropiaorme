"""Gameplay DSL for authoring E2E replay-harness scenarios.

A :class:`Scenario` builder emits chat.log lines in the canonical
``<timestamp> [<channel>] [] <message>`` shape the production
:class:`backend.services.chatlog_watcher.ChatlogWatcher` consumes.
Builders are sub-namespaced per event family
(``s.combat.damage_dealt(...)``, ``s.loot.received(...)``, etc.) for
visual grouping at authoring time and to keep the per-namespace
surface tractable as the parser grows.

Time management:

- :meth:`Scenario.at` sets the current timestamp for subsequent
  builder calls. Accepts ``str`` parsed as ``"%Y-%m-%d %H:%M:%S"``
  or :class:`datetime.datetime`.
- :meth:`Scenario.tick` advances the current timestamp by one
  second (configurable) so consecutive event clusters get
  monotonically-rising timestamps without re-typing the absolute
  value. Authors can call :meth:`Scenario.at` at any point to jump
  to an explicit timestamp.

Emission:

- :meth:`Scenario.write` writes ``chat_replay.log`` into the given
  scenario directory (created if absent), with one line per
  recorded event in source order. When the scenario also recorded
  keystrokes via ``s.keystroke.press(...)`` / ``s.keystroke.release(...)``,
  it additionally writes ``keystrokes.jsonl`` in the same shape
  the recorder emits, so scripted and recorded scenarios are
  indistinguishable at the harness layer.

Every parser :class:`backend.services.chatlog_parser.EventType`
is covered by at least one builder. Repair tooling and the
profession panel are not chatlog-sourced and land via the
screen-capture harness layer; the DSL is silent on those.

See ``backend/testing/AUTHORING.md`` for the full authoring
convention and worked examples.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Literal

_TS_FORMAT = "%Y-%m-%d %H:%M:%S"

KeystrokeKind = Literal["press", "release"]


def _coerce_ts(value: str | datetime) -> datetime:
    """Coerce a string or datetime into a naive datetime."""

    if isinstance(value, datetime):
        return value
    return datetime.strptime(value, _TS_FORMAT)


def _format_ts(ts: datetime) -> str:
    """Format a datetime in the chatlog's canonical second-resolution shape."""

    return ts.strftime(_TS_FORMAT)


class Scenario:
    """Builder for a single scripted scenario.

    Holds the in-progress chat-line list and the "current timestamp"
    every builder method attaches to its emitted line. Sub-namespace
    attributes (:attr:`combat`, :attr:`loot`, ...) expose the typed
    builders; each carries a back-reference to the scenario and
    appends lines through :meth:`_emit`.

    The scenario is build-only: it accumulates lines in memory and
    flushes them at :meth:`write` time. No validation runs at
    authoring time; the DSL round-trip property test in
    ``backend/tests/test_dsl.py`` covers parser-format drift.
    """

    def __init__(self, name: str) -> None:
        self.name = name
        self._lines: list[str] = []
        # Keystroke records mirror the recorder's ``KeystrokeTap`` shape so a
        # scripted scenario's keystrokes.jsonl is indistinguishable from a
        # recorded one at the harness layer.
        self._keystrokes: list[dict[str, object]] = []
        self._now: datetime | None = None
        # The first :meth:`at` call pins the recording epoch (offset_s = 0 there).
        self._epoch: datetime | None = None

        # Sub-namespaces back-reference the scenario so each builder
        # method can append through ``_emit`` while reading clean at
        # the call site (``s.combat.damage_dealt(...)``).
        self.combat = _CombatBuilders(self)
        self.loot = _LootBuilders(self)
        self.skill = _SkillBuilders(self)
        self.enhancer = _EnhancerBuilders(self)
        self.globals = _GlobalsBuilders(self)
        self.mission = _MissionBuilders(self)
        self.keystroke = _KeystrokeBuilders(self)

    # --- time management ----------------------------------------------

    def at(self, timestamp: str | datetime) -> Scenario:
        """Set the current timestamp for subsequent builders.

        Returns ``self`` so authoring scripts can chain a starting
        ``.at(...)`` onto the constructor (``Scenario("x").at(...)``).

        The first call also pins the keystroke-recording epoch: every
        subsequent :meth:`_record_keystroke` records ``offset_s`` as a
        delta from this point, mirroring the recorder's monotonic-clock
        reference at recording start.
        """

        self._now = _coerce_ts(timestamp)
        if self._epoch is None:
            self._epoch = self._now
        return self

    def tick(self, seconds: int = 1) -> Scenario:
        """Advance the current timestamp by ``seconds`` (default 1).

        Authors use ``tick()`` as a visual flush marker between
        event clusters; the time advance keeps consecutive lines
        from sharing a timestamp without re-typing an explicit
        :meth:`at` call. Raises if called before any :meth:`at`,
        or if ``seconds`` is not a positive integer (a non-positive
        advance would move time backwards or stall it and break the
        monotonic-timestamp guarantee scenarios rely on).
        """

        if self._now is None:
            raise RuntimeError(
                "Scenario.tick() called before any Scenario.at(...); "
                "set an initial timestamp first."
            )
        if seconds < 1:
            raise ValueError(f"Scenario.tick() requires seconds >= 1, got {seconds}.")
        self._now = self._now + timedelta(seconds=seconds)
        return self

    # --- emission -----------------------------------------------------

    def _emit(self, channel: str, message: str) -> None:
        """Append one chat line at the current timestamp.

        Internal entry point for sub-namespace builders. Raises if
        no timestamp has been set so missing ``.at(...)`` calls fail
        fast rather than producing a malformed log.
        """

        if self._now is None:
            raise RuntimeError(
                "Scenario builder called before Scenario.at(...); "
                "set an initial timestamp first."
            )
        self._lines.append(f"{_format_ts(self._now)} [{channel}] [] {message}\n")

    def _record_keystroke(self, key: str, kind: KeystrokeKind) -> None:
        """Append one keystroke edge at the current timestamp.

        Internal entry point for the ``keystroke`` sub-namespace builder.
        ``offset_s`` is the seconds delta from the scenario's first
        :meth:`at` (the epoch), matching the recorder's monotonic clock
        reference. ``wall`` is the current timestamp interpreted as UTC
        and emitted in ISO-8601 form, again matching the recorder.
        """

        if self._now is None or self._epoch is None:
            raise RuntimeError(
                "Scenario.keystroke.* called before Scenario.at(...); "
                "set an initial timestamp first."
            )
        offset_s = round((self._now - self._epoch).total_seconds(), 6)
        wall = self._now.replace(tzinfo=UTC).isoformat()
        self._keystrokes.append(
            {
                "key": key,
                "kind": kind,
                "offset_s": offset_s,
                "wall": wall,
            }
        )

    def write(self, scenario_dir: str | Path) -> Path:
        """Write ``chat_replay.log`` into ``scenario_dir``.

        Also emits ``keystrokes.jsonl`` next to it whenever any
        keystroke builder fired, with the same JSON shape the recorder
        writes (one record per line, ``sort_keys=True``, trailing
        newline), so scripted and recorded scenarios are
        indistinguishable at the harness layer.

        The directory is created if absent so an authoring script can
        be a single ``Scenario(...).write(path)`` pipeline. Returns the
        resolved path for ``chat_replay.log``.
        """

        target_dir = Path(scenario_dir)
        target_dir.mkdir(parents=True, exist_ok=True)
        out = target_dir / "chat_replay.log"
        out.write_text("".join(self._lines), encoding="utf-8")
        if self._keystrokes:
            keystrokes_path = target_dir / "keystrokes.jsonl"
            with keystrokes_path.open("w", encoding="utf-8", newline="") as fh:
                for record in self._keystrokes:
                    fh.write(json.dumps(record, sort_keys=True) + "\n")
        return out.resolve()

    def lines(self) -> list[str]:
        """Return the accumulated chat-log lines without writing.

        Used by the round-trip property test and by ad-hoc author
        debugging; production scenario builds use :meth:`write`.
        """

        return list(self._lines)

    def keystrokes(self) -> list[dict[str, object]]:
        """Return the accumulated keystroke records without writing.

        Used by tests and ad-hoc inspection; production builds use
        :meth:`write`.
        """

        return list(self._keystrokes)


# === sub-namespaces =================================================


class _CombatBuilders:
    """Combat-line builders (offensive + defensive)."""

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def damage_dealt(self, amount: float) -> None:
        """Player-inflicted damage line."""

        self._s._emit("System", f"You inflicted {amount} points of damage")

    def critical_hit(self, amount: float) -> None:
        """Critical-hit damage line.

        Emitted in the format ``Critical hit - Additional damage!
        You inflicted X points of damage`` so the parser's
        prefix-anchored CRITICAL_HIT rule wins over the generic
        DAMAGE_DEALT rule.
        """

        self._s._emit(
            "System",
            f"Critical hit - Additional damage! You inflicted {amount} points of damage",
        )

    def target_dodge(self) -> None:
        """Target dodged the player's attack."""

        self._s._emit("System", "The target Dodged your attack")

    def target_evade(self) -> None:
        """Target evaded the player's attack."""

        self._s._emit("System", "The target Evaded your attack")

    def target_jam(self) -> None:
        """Target jammed the player's attack."""

        self._s._emit("System", "The target Jammed your attack")

    def damage_received(self, amount: float) -> None:
        """Player took damage from a mob."""

        self._s._emit("System", f"You took {amount} points of damage")

    def player_dodge(self) -> None:
        """Player dodged an incoming attack."""

        self._s._emit("System", "You Dodged the attack")

    def player_evade(self) -> None:
        """Player evaded an incoming attack."""

        self._s._emit("System", "You Evaded the attack")

    def player_jam(self) -> None:
        """Player jammed an incoming attack."""

        self._s._emit("System", "You Jammed the attack")

    def mob_miss(self) -> None:
        """Mob's attack missed the player."""

        self._s._emit("System", "The attack missed you")

    def deflect(self) -> None:
        """Player armour deflected an incoming hit."""

        self._s._emit("System", "Damage deflected!")

    def self_heal(self, amount: float) -> None:
        """Player healed themselves via a heal tool."""

        self._s._emit("System", f"You healed yourself {amount} points")


class _LootBuilders:
    """Loot-line builders."""

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def received(
        self,
        item_name: str,
        value_ped: float,
        quantity: int = 1,
    ) -> None:
        """Player received a loot drop.

        ``quantity > 1`` produces the ``x (N)`` quantity-bearing
        line shape the parser recognises via ``QUANTITY_RE``;
        ``quantity == 1`` produces the single-item shape so the
        unit-stripping path is also exercised.
        """

        if quantity == 1:
            body = f"You received {item_name} Value: {value_ped:.2f} PED"
        else:
            body = f"You received {item_name} x ({quantity}) Value: {value_ped:.2f} PED"
        self._s._emit("System", body)


class _SkillBuilders:
    """Skill-gain line builders.

    Three real-game skill-gain line shapes exist (see
    :data:`backend.services.chatlog_parser.SYSTEM_RULES`); the
    modern format ``You have gained 0.0500 Bioregenesis`` is the
    default and matches the production observation cadence used by
    the rest of the harness. Other shapes can be added as named
    builders if a scenario specifically wants to pin them.
    """

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def gained(self, amount: float, skill: str) -> None:
        """Modern-format skill-gain line.

        Float-formatted to four decimals so goldens stay stable
        regardless of caller-side rounding; the parser tolerates
        trailing-zero variants but a fixed-width emission keeps
        diff output clean.
        """

        self._s._emit("System", f"You have gained {amount:.4f} {skill}")


class _EnhancerBuilders:
    """Enhancer-line builders."""

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def broken(
        self,
        enhancer_name: str,
        item_name: str,
        shrapnel_ped: float,
        remaining: int = 0,
    ) -> None:
        """Damage enhancer broke off an item.

        ``remaining`` defaults to ``0`` because the most-observed
        case is "the last enhancer on the item just broke." Set
        explicitly for scenarios that mid-stack break.
        """

        self._s._emit(
            "System",
            (
                f"Your enhancer {enhancer_name} on your {item_name} broke. "
                f"You have {remaining} enhancers remaining on the item. "
                f"You received {shrapnel_ped:.2f} PED Shrapnel."
            ),
        )


class _GlobalsBuilders:
    """Globals + HoF builders.

    The two ``[Globals]``-channel events the parser recognises:
    kill (creature) and item (rare item drop). Each accepts a
    ``hof`` flag that appends the Hall-of-Fame suffix so the
    parser's two-rule precedence (HOF_KILL before GLOBAL_KILL) is
    exercised by scripted scenarios that pin the HoF path.
    """

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def kill(
        self,
        player: str,
        creature: str,
        value_ped: float,
        hof: bool = False,
    ) -> None:
        """Global kill announcement.

        Set ``hof=True`` to append the Hall-of-Fame suffix that
        promotes the line from GLOBAL_KILL to HOF_KILL at parse
        time.
        """

        body = (
            f"{player} killed a creature ({creature}) "
            f"with a value of {value_ped:.2f} PED!"
        )
        if hof:
            body += " A record has been added to the Hall of Fame!"
        self._s._emit("Globals", body)

    def item(
        self,
        player: str,
        item: str,
        value_ped: float,
        hof: bool = False,
    ) -> None:
        """Global rare-item announcement.

        Set ``hof=True`` to append the Hall-of-Fame suffix; the
        parser's HOF_ITEM rule wins over GLOBAL_ITEM via prefix
        precedence when the suffix is present.
        """

        body = (
            f"{player} has found a rare item ({item}) "
            f"with a value of {value_ped:.2f} PED!"
        )
        if hof:
            body += " A record has been added to the Hall of Fame!"
        self._s._emit("Globals", body)


class _MissionBuilders:
    """Mission-line builders."""

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def received(self, mission_name: str) -> None:
        """A new mission was added to the player's mission log."""

        self._s._emit("System", f"New Mission received ({mission_name})")

    def completed(self, mission_name: str) -> None:
        """A mission completed (triggers reward-suppression elsewhere)."""

        self._s._emit("System", f"Mission completed ({mission_name})")


class _KeystrokeBuilders:
    """Keystroke-event builders.

    Records press / release edges into the scenario's keystroke stream,
    written to ``keystrokes.jsonl`` alongside ``chat_replay.log`` at
    :meth:`Scenario.write` time in the same JSON shape the recorder's
    ``KeystrokeTap`` emits.

    Hotbar slots use the digit-key vocabulary (``"1"`` to ``"9"`` plus
    ``"0"``); the manual-scan space key uses the literal ``"space"``.
    These are the keys the production source filters on, so authoring
    matches what a recorded session would have captured.
    """

    def __init__(self, scenario: Scenario) -> None:
        self._s = scenario

    def press(self, key: str) -> None:
        """Record a press edge for ``key`` at the current timestamp."""
        self._s._record_keystroke(key, "press")

    def release(self, key: str) -> None:
        """Record a release edge for ``key`` at the current timestamp."""
        self._s._record_keystroke(key, "release")
