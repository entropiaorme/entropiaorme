"""Test-only CLI exposing the equivalence Normalizer for cross-language checks.

Reads JSON values and emits their canonical normalised form: the compact
``json.dumps(sort_keys=True, ensure_ascii=False)`` rendering the per-event
fingerprint uses. This is the reference oracle the Rust ``eo-wire::normalizer``
differential fuzz compares against, and the generator for the committed
cross-language conformance table. It is not shipped; it lives beside the
oracle it exposes and carries no behaviour of its own.

Modes:
  (default) line server: read one compact JSON value per line on stdin, write
    its normalised form per line on stdout, flushing each, until EOF. A fresh
    ``Normalizer`` per line keeps the symbol tables independent so each value
    normalises in isolation (matching ``Normalizer::new()`` per case on the
    Rust side).
  --once: read one JSON value (the whole of stdin) and write its normalised
    form with no trailing newline.
"""

from __future__ import annotations

import json
import sys
from typing import Any

from backend.testing.fingerprint import Normalizer
from backend.testing.stdio import pin_utf8_line_protocol


def normalize_compact(value: Any) -> str:
    """Return ``value`` normalised and serialised as the fingerprint does."""
    return json.dumps(Normalizer().normalize(value), sort_keys=True, ensure_ascii=False)


def main(argv: list[str]) -> int:
    pin_utf8_line_protocol()

    if "--once" in argv:
        value = json.loads(sys.stdin.read())
        sys.stdout.write(normalize_compact(value))
        return 0

    for line in sys.stdin:
        stripped = line.rstrip("\n")
        if not stripped:
            continue
        sys.stdout.write(normalize_compact(json.loads(stripped)) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
