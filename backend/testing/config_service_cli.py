"""Line-server oracle exposing the configuration service round trip.

Each request line carries an optional raw stored file and a list of
update payloads; the oracle materialises them against the real
``ConfigService`` in a fresh temporary directory and replies with the
saved file text plus the resulting config state. The host-dependent
default chat-log path is projected to a sentinel so the comparison pins
everything except the home prefix. Part of the equivalence oracle
surface; never imported by production code.
"""

from __future__ import annotations

import json
import sys
import tempfile
from dataclasses import asdict
from pathlib import Path

from backend.services.config_service import AppConfig, ConfigService

CHATLOG_SENTINEL = "<DEFAULT_CHATLOG>"


def _round_trip(request: dict) -> dict:
    with tempfile.TemporaryDirectory() as tmp:
        data_dir = Path(tmp)
        stored = request.get("stored")
        if stored is not None:
            (data_dir / "settings.json").write_text(
                json.dumps(stored), encoding="utf-8"
            )
        service = ConfigService(data_dir)
        for updates in request.get("updates", []):
            service.update(updates)
        file_text = (data_dir / "settings.json").read_text(encoding="utf-8")
        state = asdict(service.config)

    default_path = AppConfig.default_chatlog_path()
    if state["chatlog_path"] == default_path:
        state["chatlog_path"] = CHATLOG_SENTINEL
    file_text = file_text.replace(json.dumps(default_path)[1:-1], CHATLOG_SENTINEL)
    return {"file": file_text, "state": state}


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        result = _round_trip(json.loads(line))
        sys.stdout.write(json.dumps(result, sort_keys=True, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
