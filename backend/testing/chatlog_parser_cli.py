"""Line-server oracle exposing the chat.log parser.

Each request line carries one raw chat.log line; the reply is the
parsed event (type value, formatted timestamp, data, raw line) or
null. The native port's differential drives every corpus scenario's
replay log plus curated edges through both parsers and compares the
replies byte-for-byte. Part of the equivalence oracle surface; never
imported by production code.
"""

from __future__ import annotations

import json
import sys

from backend.services.chatlog_parser import parse_line
from backend.testing.stdio import pin_utf8_line_protocol


def _reply(raw_line: str) -> dict | None:
    event = parse_line(raw_line)
    if event is None:
        return None
    return {
        "type": event.type.value,
        "timestamp": event.timestamp.strftime("%Y-%m-%d %H:%M:%S"),
        "data": event.data,
        "raw_line": event.raw_line,
    }


def main() -> None:
    pin_utf8_line_protocol()
    for line in sys.stdin:
        request = json.loads(line)
        result = _reply(request["line"])
        sys.stdout.write(json.dumps(result, sort_keys=True, ensure_ascii=False) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
