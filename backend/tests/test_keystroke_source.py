"""Tests for ``KeystrokeSource`` and its production / mock implementations.

``MockKeystrokeSource`` is covered alongside the listener tests; here the
focus is the ``PynputKeystrokeSource`` allow-list filter that enforces
the input-listening minimisation policy at the OS-hook boundary.

The pynput listener thread is not started in these tests: ``_dispatch``
is invoked directly with fake ``pynput`` key objects (any duck-typed
object exposing ``char`` or ``name``), keeping the filter contract pinned
without spawning a real OS hook.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime

from backend.testing.keystroke_source import (
    KeystrokeEvent,
    PynputKeystrokeSource,
    _pynput_key_name,
)


@dataclass(frozen=True)
class _CharKey:
    """Duck-type a pynput alphanumeric key (exposes ``.char``)."""

    char: str


@dataclass(frozen=True)
class _NamedKey:
    """Duck-type a pynput special key (exposes ``.name``, no ``.char``).

    Mirrors ``pynput.keyboard.Key.space``: ``getattr(key, "char", None)``
    is ``None`` because the attribute does not exist on the enum member.
    """

    name: str


def test_key_name_alphanumeric() -> None:
    """``key.char`` wins when present."""
    assert _pynput_key_name(_CharKey("1")) == "1"
    assert _pynput_key_name(_CharKey("a")) == "a"


def test_key_name_special() -> None:
    """``key.name`` is used when ``key.char`` is absent."""
    assert _pynput_key_name(_NamedKey("space")) == "space"
    assert _pynput_key_name(_NamedKey("f1")) == "f1"


def test_key_name_returns_none_for_unmappable() -> None:
    """A key exposing neither attribute is dropped (returns ``None``)."""

    class _Blank:
        pass

    assert _pynput_key_name(_Blank()) is None


def test_allowlist_filters_non_admitted_keys() -> None:
    """Keys outside the allow-list never reach a subscriber."""
    source = PynputKeystrokeSource(key_allowlist={"1", "2"})
    received: list[KeystrokeEvent] = []
    source.subscribe(received.append)

    source._dispatch(_CharKey("1"), "press")
    source._dispatch(_CharKey("9"), "press")  # filtered
    source._dispatch(_CharKey("2"), "release")

    assert [(e.key, e.kind) for e in received] == [("1", "press"), ("2", "release")]


def test_no_allowlist_admits_everything() -> None:
    """``key_allowlist=None`` admits every key."""
    source = PynputKeystrokeSource()
    received: list[KeystrokeEvent] = []
    source.subscribe(received.append)

    source._dispatch(_CharKey("a"), "press")
    source._dispatch(_NamedKey("ctrl"), "press")
    source._dispatch(_CharKey("9"), "press")

    assert [e.key for e in received] == ["a", "ctrl", "9"]


def test_dispatched_event_carries_timestamp() -> None:
    """Each dispatched event records the wall clock at dispatch time."""
    source = PynputKeystrokeSource()
    received: list[KeystrokeEvent] = []
    source.subscribe(received.append)

    before = datetime.now().timestamp()
    source._dispatch(_CharKey("1"), "press")
    after = datetime.now().timestamp()

    assert len(received) == 1
    event_ts = received[0].timestamp.timestamp()
    assert before <= event_ts <= after + 0.1  # small fudge for clock skew


def test_subscriber_exception_does_not_break_dispatch() -> None:
    """One callback raising must not skip subsequent callbacks."""
    source = PynputKeystrokeSource()
    seen: list[str] = []

    def _raises(_event: KeystrokeEvent) -> None:
        raise RuntimeError("subscriber raised")

    source.subscribe(_raises)
    source.subscribe(lambda e: seen.append(e.key))

    source._dispatch(_CharKey("1"), "press")
    assert seen == ["1"]


def test_unmappable_key_dropped_before_filter() -> None:
    """A key with no recognisable name is dropped before the allow-list."""

    class _Blank:
        pass

    source = PynputKeystrokeSource(key_allowlist={"1"})
    received: list[KeystrokeEvent] = []
    source.subscribe(received.append)

    source._dispatch(_Blank(), "press")
    assert received == []
