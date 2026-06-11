"""UTF-8 pinning for the cross-process line-protocol boundary.

The testing CLIs speak a JSON-lines protocol over piped stdio to a
peer process. That protocol is UTF-8 by contract regardless of host
locale: on Windows a piped stdio pair otherwise defaults to the ANSI
code page, which silently mangles non-ASCII content crossing the
boundary. Stdout also pins ``\\n`` newlines so replies are byte-stable
across platforms.
"""

from __future__ import annotations

import sys


def pin_utf8_line_protocol() -> None:
    """Reconfigure stdin/stdout for the UTF-8 JSON-lines protocol.

    Guarded with ``hasattr`` because test harnesses (pytest capture)
    substitute stream objects that lack ``reconfigure``.
    """
    if hasattr(sys.stdin, "reconfigure"):
        sys.stdin.reconfigure(encoding="utf-8")
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", newline="\n")
