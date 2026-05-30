"""Tests for the fingerprint / snapshot diff renderer.

The renderer turns a mismatch into a human-readable message naming what
changed and where. These drive each branch: equal streams, length
mismatches in both directions, event-level field and topic divergences,
and the structural walk's type / missing-key / list-length cases.
"""

from __future__ import annotations

import json

from backend.testing.diff import diff_fingerprint_files, diff_snapshot_dicts


def _event(topic: str, payload: dict) -> str:
    return json.dumps({"topic": topic, "payload": payload})


def test_identical_streams_return_none():
    stream = _event("kill", {"mob": "Atrox"}) + "\n"
    assert diff_fingerprint_files(stream, stream) is None


def test_extra_events_are_surfaced():
    expected = _event("kill", {"n": 1})
    actual = (
        expected + "\n" + _event("kill", {"n": 2}) + "\n" + _event("kill", {"n": 3})
    )
    msg = diff_fingerprint_files(expected, actual)
    assert msg is not None
    assert "length mismatch" in msg
    assert "Extra events:" in msg


def test_missing_events_are_surfaced():
    actual = _event("kill", {"n": 1})
    expected = (
        actual + "\n" + _event("kill", {"n": 2}) + "\n" + _event("kill", {"n": 3})
    )
    msg = diff_fingerprint_files(expected, actual, context=1)
    assert msg is not None
    assert "Missing events:" in msg
    assert "more" in msg


def test_field_level_divergence_names_the_path():
    expected = (
        _event("kill", {"mob": "Atrox"}) + "\n" + _event("kill", {"mob": "Daikiba"})
    )
    actual = (
        _event("kill", {"mob": "Atrox"}) + "\n" + _event("kill", {"mob": "Combibo"})
    )
    msg = diff_fingerprint_files(expected, actual)
    assert msg is not None
    assert "Event 1 of 2 diverged" in msg
    assert "Context (prior events):" in msg


def test_topic_only_divergence_is_reported():
    expected = _event("kill", {"x": 1})
    actual = _event("loot", {"x": 1})
    msg = diff_fingerprint_files(expected, actual)
    assert msg is not None
    assert "topic expected=" in msg


def test_snapshot_match_returns_none():
    snap = {"kills": [{"mob": "Atrox"}]}
    assert diff_snapshot_dicts(snap, dict(snap)) is None


def test_snapshot_type_mismatch():
    msg = diff_snapshot_dicts({"k": [1, 2]}, {"k": {"a": 1}})
    assert msg is not None and "diverges at k" in msg


def test_snapshot_missing_key():
    msg = diff_snapshot_dicts({"a": 1, "b": 2}, {"a": 1})
    assert msg is not None and "diverges at b" in msg


def test_snapshot_extra_key():
    msg = diff_snapshot_dicts({"a": 1}, {"a": 1, "b": 2})
    assert msg is not None and "diverges at b" in msg


def test_snapshot_list_length_mismatch():
    msg = diff_snapshot_dicts({"rows": [1, 2, 3]}, {"rows": [1, 2]})
    assert msg is not None and "[len]" in msg


def test_snapshot_nested_value_divergence():
    msg = diff_snapshot_dicts(
        {"rows": [{"mob": "Atrox"}]}, {"rows": [{"mob": "Daikiba"}]}
    )
    assert msg is not None and "rows[0].mob" in msg


def test_snapshot_root_scalar_divergence():
    msg = diff_snapshot_dicts({}, {})
    assert msg is None
