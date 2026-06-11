"""Test-only CLI exposing the cost engine for the cross-language differential.

Reads one ``properties_json``-shaped payload per line on stdin and writes, per
line, the normalised ``cost_per_shot_from_props`` result (the same canonical
form the fingerprint uses), so the Rust ``eo-services::cost_engine``
differential proptest can assert byte-identical numeric output over random
equipment payloads. Not shipped; lives beside the oracle it exposes.
"""

from __future__ import annotations

import json
import sys

from backend.services.cost_engine import cost_per_shot_from_props
from backend.testing.normalize_cli import normalize_compact
from backend.testing.stdio import pin_utf8_line_protocol


def main() -> int:
    pin_utf8_line_protocol()
    for line in sys.stdin:
        stripped = line.rstrip("\n")
        if not stripped:
            continue
        props = json.loads(stripped)
        result = cost_per_shot_from_props(props)
        sys.stdout.write(normalize_compact(result) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
