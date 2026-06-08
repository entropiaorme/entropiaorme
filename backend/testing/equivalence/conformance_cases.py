"""The cross-language Normalizer conformance table (authoritative inputs).

Each case is a ``(name, input_value)`` pair. The committed fixture
``normalizer_conformance.json`` is generated from this list by running each
input through the Python oracle (``normalize_compact``); both the Python and
the Rust legs of the conformance test then assert their own normaliser
reproduces the committed ``expected`` byte-for-byte.

The cases deliberately span every ``_walk`` branch and the byte-exactness
hazards the differential fuzz also targets: integer-valued floats, ties-to-even
rounding at the 4th decimal, the epoch-window boundary, UUID/ISO recognition
and symbol reuse, lexical key sorting, and Python ``ensure_ascii=False`` string
escaping. Inputs stay in the ASCII/JSON-wire domain the native backend emits.
"""

from __future__ import annotations

from typing import Any

# (name, input value): the input is any JSON-expressible value.
CASES: list[tuple[str, Any]] = [
    # --- primitives ---
    ("null", None),
    ("bool_true", True),
    ("bool_false", False),
    ("int_zero", 0),
    ("int_positive", 42),
    ("int_negative", -17),
    ("int_large", 9_999_999_999),
    # The wire integer-domain extremes: serde_json keeps these in i64/u64 (not
    # the f64 arm), so they render exactly like Python; pinning them guards the
    # boundary the module's "Domain scope" documents.
    ("int_i64_min", -9_223_372_036_854_775_808),
    ("int_u64_max", 18_446_744_073_709_551_615),
    # --- floats: rendering and rounding ---
    ("float_integer_valued", 15.0),
    ("float_hundred", 100.0),
    ("float_zero", 0.0),
    ("float_two_places", 5.12),
    ("float_leading_zero", 0.05),
    ("float_round_down", 0.123456),
    ("float_round_to_integer", 2.000049),
    ("float_tie_to_even_down", 0.03125),  # -> 0.0312
    ("float_tie_to_even_up", 0.09375),  # -> 0.0938
    ("float_negative", -5.5),
    ("float_small_boundary", -0.0001),
    # --- timestamps / epoch ---
    ("epoch_in_window", 1_700_000_000.0),
    ("epoch_just_below_window", 999_999_999.0),
    ("epoch_at_min_boundary", 1_000_000_000.0),
    ("epoch_at_max_boundary", 20_000_000_000.0),
    ("epoch_above_window", 20_000_000_001.0),
    ("iso_basic", "2026-01-01T00:00:00"),
    ("iso_with_fraction_offset", "2026-01-01T00:00:00.123456+00:00"),
    ("iso_space_separator", "2026-01-01 00:00:00"),
    ("iso_too_short_passthrough", "2026-01-01"),
    # --- uuids ---
    ("uuid_single", "11111111-1111-1111-1111-111111111111"),
    (
        "uuid_sequence_with_reuse",
        [
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222",
            "11111111-1111-1111-1111-111111111111",
        ],
    ),
    ("uuid_uppercase_passthrough", "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"),
    (
        "epoch_float_and_iso_distinct_symbols",
        [1_700_000_000.0, "2023-11-14T22:13:20"],
    ),
    # --- containers / ordering ---
    ("dict_key_sort", {"b": 1, "a": 2, "c": 3}),
    ("nested_mixed", {"items": [{"v": 1.5}, {"v": 2.0}], "n": None}),
    ("empty_object", {}),
    ("empty_array", []),
    ("array_of_scalars", [1, 2.0, "x", True, None]),
    # --- shared symbol table within one value ---
    (
        "shared_symbols_uuid_then_ts",
        {
            "id": "33333333-3333-3333-3333-333333333333",
            "at": "2026-02-02T03:04:05",
            "same_id": "33333333-3333-3333-3333-333333333333",
        },
    ),
    # --- string escaping (ensure_ascii=False) ---
    ("string_plain", "Shrapnel"),
    ("string_quote_backslash", 'a"b\\c'),
    ("string_short_escapes", "tab\tnew\nret\r"),
    ("string_control_u", chr(0x01)),
    ("string_non_ascii", "café ⚔"),
    ("string_slash_and_del", "a/b" + chr(0x7F)),
]
