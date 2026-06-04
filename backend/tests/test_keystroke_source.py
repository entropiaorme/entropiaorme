"""Tests for ``KeystrokeSource`` and its production / mock implementations.

``MockKeystrokeSource`` is covered alongside the listener tests; here the
focus is on ``PynputKeystrokeSource``:

- the allow-list filter at the OS-hook boundary (the input-listening
  minimisation policy made structural);
- the ``start()`` / ``stop()`` lifecycle exercised through a fake
  ``pynput.keyboard`` module so the real OS-hook thread never spawns.

``_dispatch`` is invoked directly with duck-typed key objects in the
filter tests; the lifecycle tests swap ``sys.modules["pynput"]`` with a
fake whose ``keyboard.Listener`` records its constructor callbacks and
exposes them to the test.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from datetime import datetime
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest

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


# ----------------------------------------------------------------------
# start() / stop() lifecycle, exercised through a fake pynput.keyboard
# ----------------------------------------------------------------------


@dataclass
class _FakeListener:
    """Stand-in for ``pynput.keyboard.Listener``.

    Records the ``on_press`` / ``on_release`` callbacks the source
    installed, plus ``start()`` and ``stop()`` invocation counts, so a
    test can drive the source through its lifecycle and replay
    keystrokes back through the recorded callbacks without involving
    a real OS hook.
    """

    on_press: Any = None
    on_release: Any = None
    daemon: bool = False
    started: int = 0
    stopped: int = 0
    instances: list[_FakeListener] = field(default_factory=list)

    def start(self) -> None:
        self.started += 1

    def stop(self) -> None:
        self.stopped += 1


@pytest.fixture
def fake_pynput(monkeypatch: pytest.MonkeyPatch):
    """Install a fake ``pynput.keyboard`` module that yields ``_FakeListener``.

    Yields a SimpleNamespace exposing the most-recently-constructed
    listener so the test can drive it (``ns.listener.on_press(key)``)
    or assert on its lifecycle counters (``ns.listener.started``).
    """
    state = SimpleNamespace(listener=None)

    def _make_listener(on_press=None, on_release=None):
        listener = _FakeListener(on_press=on_press, on_release=on_release)
        state.listener = listener
        return listener

    pynput = ModuleType("pynput")
    keyboard = ModuleType("pynput.keyboard")
    keyboard.Listener = _make_listener  # type: ignore[attr-defined]
    pynput.keyboard = keyboard  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "pynput", pynput)
    monkeypatch.setitem(sys.modules, "pynput.keyboard", keyboard)
    return state


def test_start_constructs_and_starts_listener(fake_pynput) -> None:
    """``start()`` builds the fake listener, marks it daemon, calls start."""
    source = PynputKeystrokeSource()
    source.start()

    listener = fake_pynput.listener
    assert listener is not None
    assert listener.daemon is True
    assert listener.started == 1


def test_start_names_the_listener_thread_when_configured(fake_pynput) -> None:
    """A configured ``thread_name`` labels the listener thread (observability)."""
    source = PynputKeystrokeSource(thread_name="hotbar-key-listener")
    source.start()

    assert fake_pynput.listener.name == "hotbar-key-listener"


def test_start_is_idempotent(fake_pynput) -> None:
    """A second ``start()`` while already running is a no-op."""
    source = PynputKeystrokeSource()
    source.start()
    first = fake_pynput.listener
    source.start()  # second call must not replace or restart anything.

    # No new listener instance, no second start, source._listener still set.
    assert fake_pynput.listener is first
    assert first.started == 1


def test_stop_stops_the_listener_and_is_idempotent(fake_pynput) -> None:
    """``stop()`` calls listener.stop and clears the slot; second call no-op."""
    source = PynputKeystrokeSource()
    source.start()
    listener = fake_pynput.listener

    source.stop()
    assert listener.stopped == 1
    assert source._listener is None

    source.stop()
    assert listener.stopped == 1  # unchanged


def test_listener_on_press_routes_to_dispatch(fake_pynput) -> None:
    """The listener's recorded ``on_press`` reaches subscribers as press."""
    source = PynputKeystrokeSource()
    received: list[KeystrokeEvent] = []
    source.subscribe(received.append)
    source.start()

    fake_pynput.listener.on_press(_CharKey("1"))

    assert len(received) == 1
    assert received[0].key == "1"
    assert received[0].kind == "press"


def test_listener_on_release_routes_to_dispatch(fake_pynput) -> None:
    """The listener's recorded ``on_release`` reaches subscribers as release."""
    source = PynputKeystrokeSource()
    received: list[KeystrokeEvent] = []
    source.subscribe(received.append)
    source.start()

    fake_pynput.listener.on_release(_NamedKey("space"))

    assert len(received) == 1
    assert received[0].key == "space"
    assert received[0].kind == "release"


def test_start_is_inert_when_pynput_is_missing(
    monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    """``start()`` logs a warning and leaves the source inert under ImportError.

    Mirrors the pre-seam listeners' behaviour: missing ``pynput`` disables
    the feature with a single log line, never crashes the app.
    """
    # Setting the module entry to None makes `from pynput import keyboard`
    # raise ImportError, the exact path the legacy listeners handled.
    monkeypatch.setitem(sys.modules, "pynput", None)

    source = PynputKeystrokeSource()
    with caplog.at_level("WARNING"):
        source.start()

    assert source._listener is None
    assert any("pynput not installed" in record.message for record in caplog.records)


def test_stop_without_start_is_a_noop() -> None:
    """``stop()`` on an idle source touches no module-level state."""
    source = PynputKeystrokeSource()
    # Pre-condition: no listener installed.
    assert source._listener is None
    source.stop()  # must not raise.
    assert source._listener is None
