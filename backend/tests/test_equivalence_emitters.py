"""Python faithfulness leg of the cross-language emitter proof.

The Rust emitter proof (``eo-wire/tests/emitters_proof.rs``) feeds the committed
raw-capture fixtures through the native emitters and asserts byte-equality with
the committed ``basic_hunt_10_events`` goldens. This leg proves the same
committed raw fixtures still reproduce those goldens under the PYTHON oracle
emitters, so a stale or hand-edited raw fixture cannot pass the Rust leg
silently: both legs pin the same (raw input -> committed golden) mapping, one
per language.

Regenerate the raw fixtures with the on-demand dumper when the scenario or the
oracle changes::

    EO_DUMP_RAW=1 .venv/Scripts/python.exe -m pytest \
        backend/tests/e2e/test_equivalence_raw_dump.py -q
"""

from __future__ import annotations

import base64
import json
from pathlib import Path
from typing import Any

from backend.testing import db_snapshot
from backend.testing.fingerprint import Normalizer
from backend.testing.http_fingerprint import (
    HttpCapture,
    HttpRequest,
    HttpResponse,
    normalise_body,
    normalise_path,
    project_headers,
)

SCENARIO = (
    Path(__file__).parent / "e2e" / "corpus" / "scripted" / "basic_hunt_10_events"
)
RAW = SCENARIO / "raw_captures"
EXPECTED = SCENARIO / "expected"


def _load(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def test_fingerprint_and_db_snapshot_reproduce_goldens() -> None:
    """One shared Normalizer over the raw events then the raw DB rows must
    reproduce both the committed fingerprint.jsonl and db_state.json, exactly
    as ``GoldenSet`` threads one normaliser across the two surfaces."""
    normalizer = Normalizer()

    events = _load(RAW / "events.json")
    fingerprint_lines = [
        json.dumps(
            {
                "topic": event["topic"],
                "payload": normalizer.normalize(event["payload"]),
            },
            sort_keys=True,
            ensure_ascii=False,
        )
        for event in events
    ]
    actual_fingerprint = (
        "\n".join(fingerprint_lines) + "\n" if fingerprint_lines else ""
    )
    assert actual_fingerprint == (EXPECTED / "fingerprint.jsonl").read_text(
        encoding="utf-8"
    )

    # The DB snapshot continues the same normaliser over the pre-fetched rows,
    # iterated in catalogue order (the order that drives symbol assignment).
    db_rows = _load(RAW / "db_rows.json")
    snapshot = {
        spec.name: [normalizer.normalize(row) for row in db_rows.get(spec.name, [])]
        for spec in db_snapshot.CATALOGUE
    }
    actual_db = db_snapshot.serialize(snapshot)
    assert actual_db == (EXPECTED / "db_state.json").read_text(encoding="utf-8")


def test_http_fingerprints_reproduce_goldens() -> None:
    """A fresh Normalizer over the raw responses, in dump order, must reproduce
    every committed per-endpoint HTTP golden (body normalised before path, as
    ``HttpFingerprinter.capture`` orders it)."""
    normalizer = Normalizer()
    captures = _load(RAW / "http_responses.json")
    assert captures, "http raw captures must not be empty"

    for capture in captures:
        endpoint_id = capture["endpoint_id"]
        headers = capture["headers"]
        content_type = next(
            (v for k, v in headers.items() if k.lower() == "content-type"), None
        )
        body_bytes = base64.b64decode(capture["body_b64"])

        projected_headers = project_headers(headers)
        body = normalise_body(body_bytes, content_type, normalizer)
        path = normalise_path(capture["path"], normalizer)

        golden = HttpCapture(
            request=HttpRequest(
                method=capture["method"],
                path=path,
                query=dict(capture["query"]),
            ),
            response=HttpResponse(
                status_code=capture["status_code"],
                headers=projected_headers,
                body=body,
            ),
        ).to_golden_dict()
        actual = json.dumps(golden, sort_keys=True, indent=2, ensure_ascii=False) + "\n"

        expected = (EXPECTED / "http_responses" / f"{endpoint_id}.json").read_text(
            encoding="utf-8"
        )
        assert actual == expected, f"HTTP golden diverged for {endpoint_id}"
