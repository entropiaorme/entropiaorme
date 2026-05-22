"""Structural diff for event-stream fingerprints and DB-state snapshots.

The output is human-readable on failure: pytest surfaces a sentence
naming what changed, where in the event stream it changed, and the
field-level delta with surrounding context. Walking is depth-first,
first-divergence-wins, which keeps the message focused on the proximate
cause of failure rather than dumping every downstream consequence.

Useful enough to debug a regression without resorting to ``diff -u``
on the raw JSON; the field-path notation (``kills[0].mob_name``)
mirrors how a developer would point at the same row mentally.
"""

from __future__ import annotations

import json
from typing import Any


def diff_fingerprint_files(
    expected_text: str,
    actual_text: str,
    context: int = 2,
) -> str | None:
    """Compare two JSONL fingerprint streams.

    Returns ``None`` on byte-equivalence (after blank-line trim) or a
    human-readable message describing the first divergence: either a
    length mismatch with the extra/missing events surfaced, or an
    event-level mismatch annotated with the field path that diverged.
    ``context`` controls how many prior events are echoed beneath the
    divergence message for orientation.
    """
    expected_lines = [ln for ln in expected_text.splitlines() if ln.strip()]
    actual_lines = [ln for ln in actual_text.splitlines() if ln.strip()]
    if len(expected_lines) != len(actual_lines):
        return _length_mismatch_message(expected_lines, actual_lines, context)
    for idx, (exp, act) in enumerate(zip(expected_lines, actual_lines)):
        if exp == act:
            continue
        return _event_divergence_message(idx, expected_lines, actual_lines, context)
    return None


def diff_snapshot_dicts(
    expected: dict[str, Any],
    actual: dict[str, Any],
) -> str | None:
    """Compare two normalised snapshot dicts.

    Returns ``None`` on match, a human-readable message naming the
    first divergent path on mismatch. The path notation follows the
    same convention as event-payload diffs: ``table[row_index].column``.
    """
    divergence = _first_divergence(expected, actual, path=[])
    if divergence is None:
        return None
    path_str, exp_val, act_val = divergence
    return (
        f"DB snapshot diverges at {path_str or '<root>'}:\n"
        f"  expected: {json.dumps(exp_val, sort_keys=True)}\n"
        f"  actual:   {json.dumps(act_val, sort_keys=True)}"
    )


def _length_mismatch_message(
    expected: list[str],
    actual: list[str],
    context: int,
) -> str:
    """Format the diff message for two streams of different length.

    Walks whichever side has surplus events and renders up to
    ``context`` of them inline so the reader sees a sample of the
    extra/missing content; remaining events are summarised as a
    count.
    """
    lines = [
        f"Event stream length mismatch: expected {len(expected)} events, "
        f"got {len(actual)}."
    ]
    if len(actual) > len(expected):
        start = len(expected)
        end = min(len(actual), start + max(context, 1))
        lines.append("Extra events:")
        for idx in range(start, end):
            lines.append(f"  +[{idx}] {actual[idx]}")
        if end < len(actual):
            lines.append(f"  ...and {len(actual) - end} more")
    else:
        start = len(actual)
        end = min(len(expected), start + max(context, 1))
        lines.append("Missing events:")
        for idx in range(start, end):
            lines.append(f"  -[{idx}] {expected[idx]}")
        if end < len(expected):
            lines.append(f"  ...and {len(expected) - end} more")
    return "\n".join(lines)


def _event_divergence_message(
    idx: int,
    expected: list[str],
    actual: list[str],
    context: int,
) -> str:
    """Format the diff message for two streams that differ at event
    ``idx``.

    Surfaces the topic, then walks the two payloads to find the first
    field-level divergence (rendered with ``field path: expected X,
    got Y`` syntax). If only the topic differs, that fact is reported
    directly. ``context`` prior events are echoed beneath for
    orientation when ``idx > 0``.
    """
    exp_obj = json.loads(expected[idx])
    act_obj = json.loads(actual[idx])
    lines = [
        f"Event {idx} of {len(expected)} diverged "
        f"(topic={exp_obj.get('topic')!r}):"
    ]
    field_diff = _first_divergence(
        exp_obj.get("payload"),
        act_obj.get("payload"),
        path=[],
    )
    if field_diff is not None:
        path_str, exp_val, act_val = field_diff
        lines.append(
            f"  field {path_str or '<payload>'}: "
            f"expected {json.dumps(exp_val, sort_keys=True)}, "
            f"got {json.dumps(act_val, sort_keys=True)}"
        )
    elif exp_obj.get("topic") != act_obj.get("topic"):
        lines.append(
            f"  topic expected={exp_obj.get('topic')!r}, "
            f"got={act_obj.get('topic')!r}"
        )
    else:
        # Should not happen since the line-level strings differed, but
        # surface a marker rather than swallowing the case silently.
        lines.append("  (line-level mismatch with no payload divergence)")
    if context > 0 and idx > 0:
        ctx_start = max(0, idx - context)
        lines.append("Context (prior events):")
        for ctx_idx in range(ctx_start, idx):
            lines.append(f"   [{ctx_idx}] {expected[ctx_idx]}")
    return "\n".join(lines)


def _first_divergence(
    expected: Any,
    actual: Any,
    path: list[str],
) -> tuple[str, Any, Any] | None:
    """Depth-first walk returning the first divergent ``(path,
    expected, actual)`` triple, or ``None`` when the two structures
    are equal.

    Dict-key sets are compared (missing-on-one-side is reported as
    ``value vs None`` at the key's path); shared keys recurse. List
    length mismatch returns a ``[len]`` segment with the two lengths
    so the surface message reads as ``kills[len]: 3 vs 2``. Mixed
    types (e.g. dict vs list at the same path) report the two values
    directly.
    """
    if type(expected) is not type(actual):
        return _format_path(path), expected, actual
    if isinstance(expected, dict):
        expected_keys = set(expected.keys())
        actual_keys = set(actual.keys())
        for missing in sorted(expected_keys - actual_keys):
            return _format_path(path + [str(missing)]), expected[missing], None
        for extra in sorted(actual_keys - expected_keys):
            return _format_path(path + [str(extra)]), None, actual[extra]
        for key in sorted(expected_keys):
            sub = _first_divergence(
                expected[key], actual[key], path + [str(key)]
            )
            if sub is not None:
                return sub
        return None
    if isinstance(expected, list):
        if len(expected) != len(actual):
            return (
                _format_path(path + ["[len]"]),
                len(expected),
                len(actual),
            )
        for idx, (exp_item, act_item) in enumerate(zip(expected, actual)):
            sub = _first_divergence(exp_item, act_item, path + [f"[{idx}]"])
            if sub is not None:
                return sub
        return None
    if expected != actual:
        return _format_path(path), expected, actual
    return None


def _format_path(path: list[str]) -> str:
    """Render a path list as ``table[0].column`` style."""
    if not path:
        return ""
    out: list[str] = []
    for part in path:
        if part.startswith("["):
            out.append(part)
        elif out:
            out.append("." + part)
        else:
            out.append(part)
    return "".join(out)
