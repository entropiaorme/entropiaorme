"""Line-server oracle exposing the static-table functions.

The native port's differential tests drive this one JSON request per
line and compare the JSON reply byte-for-byte (both sides serialise
with sorted keys through their Python-faithful encoders). Part of the
equivalence oracle surface; never imported by production code.
"""

from __future__ import annotations

import json
import sys

from backend.data import codex_categories, tt_value_curve


def _dispatch(request: dict) -> object:
    op = request["op"]
    if op == "tt_value_at":
        return tt_value_curve.tt_value_at(request["level"])
    if op == "tt_value_of_gain":
        return tt_value_curve.tt_value_of_gain(
            request["from_level"], request["to_level"]
        )
    if op == "levels_for_tt_value":
        return tt_value_curve.levels_for_tt_value(
            request["from_level"], request["ped_value"]
        )
    if op == "max_tt_curve_level":
        return tt_value_curve.max_tt_curve_level()
    if op == "get_codex_category":
        return codex_categories.get_codex_category(request["skill_name"])
    if op == "build_rank_breakdown":
        return codex_categories.build_rank_breakdown(
            request["base_cost"], request.get("codex_type")
        )
    raise ValueError(f"unknown op: {op}")


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        result = _dispatch(json.loads(line))
        sys.stdout.write(json.dumps(result, sort_keys=True, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
