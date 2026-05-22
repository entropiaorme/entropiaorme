"""Unit tests for the harness Normalizer, FingerprintRecorder, and diff
renderer.

The integration test in ``backend/tests/e2e/test_basic_hunt_10_events.py``
exercises these pieces end-to-end against the real pipeline; the
tests here pin the contract of each piece in isolation so a regression
in normalisation or diff rendering is caught before the integration
test reports the symptom downstream.
"""

from __future__ import annotations

from datetime import datetime

from backend.core.event_bus import EventBus
from backend.testing.diff import diff_fingerprint_files, diff_snapshot_dicts
from backend.testing.fingerprint import FingerprintRecorder, Normalizer


# --- Normalizer ----------------------------------------------------------


def test_normalizer_assigns_uuid_symbols_in_encounter_order() -> None:
    norm = Normalizer()
    uuid_a = "12345678-1234-1234-1234-123456789abc"
    uuid_b = "abcdef01-1234-1234-1234-123456789abc"
    assert norm.normalize(uuid_a) == "<UUID_1>"
    assert norm.normalize(uuid_b) == "<UUID_2>"
    # The same UUID resolves to the same symbol on later calls.
    assert norm.normalize(uuid_a) == "<UUID_1>"


def test_normalizer_unifies_uuid_across_event_and_db_payloads() -> None:
    norm = Normalizer()
    uuid = "12345678-1234-1234-1234-123456789abc"
    event_payload = norm.normalize({"session_id": uuid})
    row_payload = norm.normalize({"id": uuid, "extra": uuid})
    assert event_payload["session_id"] == "<UUID_1>"
    assert row_payload["id"] == "<UUID_1>"
    assert row_payload["extra"] == "<UUID_1>"


def test_normalizer_assigns_timestamp_symbols_in_encounter_order() -> None:
    norm = Normalizer()
    ts_1 = datetime(2026, 5, 19, 10, 0, 0)
    ts_2 = datetime(2026, 5, 19, 10, 0, 5)
    assert norm.normalize(ts_1) == "<TS_1>"
    assert norm.normalize(ts_2) == "<TS_2>"
    assert norm.normalize(ts_1) == "<TS_1>"


def test_normalizer_treats_iso_string_as_timestamp() -> None:
    norm = Normalizer()
    assert norm.normalize("2026-05-22T10:00:00") == "<TS_1>"
    assert norm.normalize("2026-05-22 10:00:00") == "<TS_2>"


def test_normalizer_treats_epoch_float_as_timestamp() -> None:
    norm = Normalizer()
    epoch = 1_779_487_800.0
    assert norm.normalize(epoch) == "<TS_1>"


def test_normalizer_unifies_datetime_and_iso_for_same_instant() -> None:
    """A datetime and its isoformat() string normalise to the same
    symbol because the datetime branch routes through isoformat()."""
    norm = Normalizer()
    ts = datetime(2026, 5, 19, 10, 0, 0)
    assert norm.normalize(ts) == "<TS_1>"
    assert norm.normalize(ts.isoformat()) == "<TS_1>"


def test_normalizer_rounds_floats_outside_epoch_window_to_four_dp() -> None:
    norm = Normalizer()
    assert norm.normalize(3.123_456_789) == 3.1235
    assert norm.normalize(5.12) == 5.12
    assert norm.normalize(0.0) == 0.0


def test_normalizer_preserves_integers_and_booleans() -> None:
    norm = Normalizer()
    assert norm.normalize(500) == 500
    assert isinstance(norm.normalize(500), int)
    assert norm.normalize(True) is True
    assert norm.normalize(False) is False
    assert norm.normalize(None) is None


def test_normalizer_sorts_dict_keys() -> None:
    norm = Normalizer()
    out = norm.normalize({"b": 1, "a": 2, "c": 3})
    assert list(out.keys()) == ["a", "b", "c"]


def test_normalizer_preserves_list_order() -> None:
    norm = Normalizer()
    assert norm.normalize([3, 1, 2]) == [3, 1, 2]


def test_normalizer_reset_clears_symbol_tables() -> None:
    norm = Normalizer()
    uuid = "12345678-1234-1234-1234-123456789abc"
    assert norm.normalize(uuid) == "<UUID_1>"
    norm.reset()
    assert norm.normalize(uuid) == "<UUID_1>"


def test_normalizer_walks_nested_structures() -> None:
    norm = Normalizer()
    uuid = "12345678-1234-1234-1234-123456789abc"
    out = norm.normalize(
        {
            "items": [
                {"value_ped": 5.123456, "uuid": uuid},
                {"value_ped": 0.12},
            ],
            "session_id": uuid,
        }
    )
    assert out == {
        "items": [
            {"uuid": "<UUID_1>", "value_ped": 5.1235},
            {"value_ped": 0.12},
        ],
        "session_id": "<UUID_1>",
    }


# --- FingerprintRecorder -------------------------------------------------


def test_recorder_captures_bus_events_in_publish_order() -> None:
    bus = EventBus()
    norm = Normalizer()
    recorder = FingerprintRecorder(norm)
    recorder.install(bus)

    bus.publish("session_started", {"session_id": "abc"})
    bus.publish("combat", {"type": "damage_dealt", "amount": 10.5})

    assert recorder.events == [
        ("session_started", {"session_id": "abc"}),
        ("combat", {"type": "damage_dealt", "amount": 10.5}),
    ]


def test_recorder_forwards_to_existing_subscribers() -> None:
    bus = EventBus()
    norm = Normalizer()
    received: list[tuple[str, dict]] = []

    def subscriber(data):
        received.append(("combat", data))

    bus.subscribe("combat", subscriber)
    recorder = FingerprintRecorder(norm)
    recorder.install(bus)

    bus.publish("combat", {"type": "damage_dealt", "amount": 10.5})

    assert received == [("combat", {"type": "damage_dealt", "amount": 10.5})]
    assert recorder.events == [
        ("combat", {"type": "damage_dealt", "amount": 10.5}),
    ]


def test_recorder_serialise_emits_jsonl_with_sorted_keys() -> None:
    bus = EventBus()
    norm = Normalizer()
    recorder = FingerprintRecorder(norm)
    recorder.install(bus)

    bus.publish("combat", {"amount": 10.5, "type": "damage_dealt"})

    serialised = recorder.serialize()
    # Single line ending with newline, keys lexically sorted at JSON level.
    assert serialised == (
        '{"payload": {"amount": 10.5, "type": "damage_dealt"}, '
        '"topic": "combat"}\n'
    )


def test_recorder_serialise_empty_stream_returns_empty_string() -> None:
    norm = Normalizer()
    recorder = FingerprintRecorder(norm)
    assert recorder.serialize() == ""


def test_recorder_install_on_second_bus_unwraps_first() -> None:
    """Re-installing on a different bus must unwrap the prior bus.

    Otherwise the prior bus would keep dispatching through the
    recorder's shadow function, silently mixing one scenario's
    events into the next test's recorded stream.
    """
    bus_a = EventBus()
    bus_b = EventBus()
    recorder = FingerprintRecorder(Normalizer())

    recorder.install(bus_a)
    recorder.install(bus_b)

    bus_a.publish("from_a", {"x": 1})
    bus_b.publish("from_b", {"x": 2})

    # Only the bus the recorder currently wraps should feed it.
    assert recorder.events == [("from_b", {"x": 2})]


def test_recorder_uninstall_restores_original_publish() -> None:
    """After uninstall, publishing on the bus stops feeding the recorder.

    Bound-method identity is brittle (Python rebinds on every attribute
    access) so the test asserts the behavioural restoration instead:
    a publish after uninstall does not extend the recorded event list.
    """
    bus = EventBus()
    recorder = FingerprintRecorder(Normalizer())
    recorder.install(bus)

    bus.publish("first", {"x": 1})
    assert recorder.events == [("first", {"x": 1})]

    recorder.uninstall()
    bus.publish("second", {"x": 2})
    assert recorder.events == [("first", {"x": 1})]


# --- diff renderer -------------------------------------------------------


def test_diff_fingerprint_matches_returns_none() -> None:
    text = '{"payload": {"x": 1}, "topic": "a"}\n'
    assert diff_fingerprint_files(text, text) is None


def test_diff_fingerprint_length_mismatch_surfaces_extras() -> None:
    expected = '{"payload": {}, "topic": "a"}\n'
    actual = (
        '{"payload": {}, "topic": "a"}\n'
        '{"payload": {}, "topic": "b"}\n'
    )
    msg = diff_fingerprint_files(expected, actual)
    assert msg is not None
    assert "length mismatch" in msg
    assert "expected 1 events, got 2" in msg
    assert '"topic": "b"' in msg


def test_diff_fingerprint_event_divergence_names_field_path() -> None:
    expected = (
        '{"payload": {"x": 1}, "topic": "a"}\n'
        '{"payload": {"value": 5.12}, "topic": "loot"}\n'
    )
    actual = (
        '{"payload": {"x": 1}, "topic": "a"}\n'
        '{"payload": {"value": 5.2}, "topic": "loot"}\n'
    )
    msg = diff_fingerprint_files(expected, actual)
    assert msg is not None
    assert "Event 1 of 2" in msg
    assert "topic='loot'" in msg
    assert "field value" in msg
    assert "expected 5.12" in msg
    assert "got 5.2" in msg


def test_diff_snapshot_dict_returns_none_on_match() -> None:
    snapshot = {"kills": [{"id": "x", "mob": "Argonaut"}]}
    assert diff_snapshot_dicts(snapshot, snapshot) is None


def test_diff_snapshot_dict_surfaces_field_path() -> None:
    expected = {"kills": [{"id": "x", "mob_name": "Argonaut"}]}
    actual = {"kills": [{"id": "x", "mob_name": "Caldorite"}]}
    msg = diff_snapshot_dicts(expected, actual)
    assert msg is not None
    assert "kills[0].mob_name" in msg
    assert '"Argonaut"' in msg
    assert '"Caldorite"' in msg


def test_diff_snapshot_dict_surfaces_list_length_mismatch() -> None:
    expected = {"kills": [{"id": "x"}, {"id": "y"}]}
    actual = {"kills": [{"id": "x"}]}
    msg = diff_snapshot_dicts(expected, actual)
    assert msg is not None
    assert "kills[len]" in msg
