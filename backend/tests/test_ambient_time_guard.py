"""Tests for the no-ambient-time backend determinism guard.

The guard (``backend/scripts/check_ambient_time.py``) is the static half of
the clock-seam exit condition: golden stability alone passes vacuously on a
clock-coupled surface that carries no golden, so the zero-ambient-reads
guarantee is asserted directly against the tree. These tests assert it is
green against the live tree AND that it has teeth: every forbidden read form
(direct, module-aliased, class-aliased, bare-reference, from-import) turns it
red, strings and comments never false-positive, and the pragma escape hatch
demands a reason.
"""

from __future__ import annotations

from backend.scripts.check_ambient_time import (
    REPO_ROOT,
    Finding,
    evaluate,
    scan_source,
)


def _linenos(findings: list[Finding]) -> list[int]:
    return [f.lineno for f in findings]


def test_backend_production_tree_is_clean() -> None:
    """The live production source has no ambient clock reads."""
    findings = evaluate(REPO_ROOT)
    assert findings == [], "Ambient clock reads in production code:\n" + "\n".join(
        f"  {f.path}:{f.lineno} {f.detail}" for f in findings
    )


def test_scan_flags_every_direct_read_form() -> None:
    """Each forbidden read pattern is caught at its line."""
    src = (
        "import time\n"
        "from datetime import UTC, date, datetime\n"
        "a = time.time()\n"
        "b = time.monotonic()\n"
        "c = time.perf_counter()\n"
        "d = datetime.now(tz=None)\n"
        "e = datetime.utcnow()\n"
        "f = date.today()\n"
    )
    findings = scan_source("backend/services/x.py", src)
    assert _linenos(findings) == [3, 4, 5, 6, 7, 8]


def test_scan_flags_aliased_and_bare_reference_forms() -> None:
    """Module aliases, class aliases, module-path class reads, and bare
    (uncalled) references such as ``default_factory=datetime.now`` are all
    resolved and flagged."""
    src = (
        "import time as _time\n"
        "import datetime as dt\n"
        "from datetime import datetime as dtt\n"
        "from dataclasses import field\n"
        "a = _time.monotonic()\n"
        "b = dt.datetime.now()\n"
        "c = dtt.utcnow()\n"
        "d = field(default_factory=dtt.now)\n"
    )
    findings = scan_source("backend/tracking/x.py", src)
    assert _linenos(findings) == [5, 6, 7, 8]


def test_scan_flags_from_time_imports() -> None:
    """Importing the time-module callables directly is itself a violation
    (the bare-name call would otherwise evade the attribute scan)."""
    src = "from time import monotonic, perf_counter, time\n"
    findings = scan_source("backend/core/x.py", src)
    assert len(findings) == 3
    assert all(f.lineno == 1 for f in findings)


def test_scan_ignores_strings_comments_and_sleep() -> None:
    """AST matching: prose mentions and ``time.sleep`` are not reads."""
    src = (
        "import time\n"
        'DOC = "call time.time() and datetime.now() for the current instant"\n'
        "# a comment mentioning time.monotonic() is fine\n"
        "time.sleep(0.1)\n"
        "startedAt: float | None = None  # unix timestamp (time.time())\n"
    )
    assert scan_source("backend/routers/x.py", src) == []


def test_scan_ignores_clock_seam_usage_and_unrelated_attributes() -> None:
    """Reads through the injected seam and attribute pairs that merely end in
    a forbidden name without rooting in the module are clean."""
    src = (
        "from backend.testing.clock import RealClock\n"
        "clock = RealClock()\n"
        "a = clock.now().timestamp()\n"
        "b = clock.monotonic()\n"
        "c = record.date.today if hasattr(record, 'date') else None\n"
    )
    assert scan_source("backend/services/x.py", src) == []


def test_pragma_with_reason_suppresses_and_bare_pragma_fails() -> None:
    """The escape hatch works only with a written justification."""
    justified = (
        "import time\n"
        "a = time.time()  # ambient-time: allowed (boot banner only, no output)\n"
    )
    assert scan_source("backend/services/x.py", justified) == []

    bare = "import time\na = time.time()  # ambient-time: allowed ()\n"
    findings = scan_source("backend/services/x.py", bare)
    assert len(findings) == 1
    assert "no reason" in findings[0].detail


def test_pragma_inside_a_string_does_not_suppress_a_real_finding() -> None:
    """The escape hatch is honoured only in a comment token.

    Pragma text inside a string literal on the same line as a genuine ambient
    read must not silently suppress the finding: the line carries no comment,
    so the read is still flagged. A whole-line match would wrongly suppress
    it, vacuously defeating the guard.
    """
    src = (
        "import time\n"
        'a = time.time(); s = "# ambient-time: allowed (not a real pragma)"\n'
    )
    findings = scan_source("backend/services/x.py", src)
    assert len(findings) == 1
    assert findings[0].lineno == 2


def test_scan_reports_unparseable_source_loudly() -> None:
    """A syntactically broken production file is a finding, not a skip."""
    findings = scan_source("backend/db/x.py", "def broken(:\n")
    assert len(findings) == 1
    assert "unparseable" in findings[0].detail
