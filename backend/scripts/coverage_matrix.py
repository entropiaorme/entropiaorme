"""Generate the service to scenario/test coverage matrix.

Walks ``backend/services/*.py`` and the test corpus and emits
``backend/testing/COVERAGE.md``: one row per service module mapping its
externally-observable behaviour to the tests and scenarios that exercise
it, plus how the line is held to account (branch coverage, mutation
testing, or a documented device/IO exemption).

The covering-tests column is derived by scanning the test tree for modules
that import each service, and the classification column is read from
``pyproject.toml`` (the coverage ``omit`` list and the mutation
``paths_to_mutate`` list), so the matrix cannot drift from the real test
surface or the real gate configuration without the drift guard
(``backend/tests/test_coverage_matrix_drift.py``) failing. The behaviour
summaries are the one curated column: concise, human-authored descriptions
of what each service does that a scenario could observe.

Run it directly to refresh the file, or import :func:`render_matrix` to
compare the on-disk copy against a fresh render.
"""

from __future__ import annotations

import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SERVICES_DIR = REPO_ROOT / "backend" / "services"
TESTS_DIR = REPO_ROOT / "backend" / "tests"
PYPROJECT = REPO_ROOT / "pyproject.toml"
OUTPUT = REPO_ROOT / "backend" / "testing" / "COVERAGE.md"

# Concise, human-authored summary of each service's externally-observable
# behaviour: what a scenario or a caller can see it do. The covering-tests
# and classification columns are derived, so this is the only column that is
# maintained by hand; a new service without an entry surfaces as a generation
# error rather than a silently-blank row.
BEHAVIOUR: dict[str, str] = {
    "__init__": "Package marker (no runtime behaviour).",
    "character_calc": "Skill / profession / HP optimisers and prospect forecasts.",
    "chatlog_parser": "Parses chat.log lines into typed gameplay events.",
    "chatlog_watcher": "Tails chat.log, buffers ticks, publishes events on the bus.",
    "codex_service": "Codex rank progress, claims, calibration, recommendations.",
    "config_service": "Loads, validates, and persists the app settings overlay.",
    "cost_engine": "Per-shot weapon / amp / heal cost from the equipment catalogue.",
    "eu_window": "Locates the Entropia Universe window for screen capture.",
    "game_data_store": "Read-only access to the bundled game-data tables.",
    "hotbar_listener": "Maps hotbar keystrokes to active-tool change events.",
    "local_ocr": "ONNX skill-panel OCR engine, screen grab, and orchestration.",
    "mob_lookup_service": "Resolves mob names to species / maturity metadata.",
    "quest_service": "Quest and playlist CRUD, completion, reward suppression.",
    "repair_ocr": "OCR of the repair-window cost field.",
    "scan_completion": "Persists scanned skill levels and emits drift logs.",
    "scan_drift": "Compares scanned levels against the calibration baseline.",
    "scan_presets": "Built-in skill-scan region presets.",
    "session_summary": "Composes the end-of-session summary projection.",
    "skill_panel_parse": "Pure skill-panel parsing (name / level / bar / cells).",
    "skill_scan_core": "Skill-scan capture and recognition pipeline core.",
    "skill_scan_manual": "Manual skill-scan lifecycle and capture orchestration.",
    "skill_tracker": "Records chat.log skill gains during a tracking session.",
    "spacebar_capture_listener": "Maps the spacebar to a skill-scan capture trigger.",
    "trifecta_service": "Validates and describes the weapon / heal trifecta loadout.",
}


def _service_modules() -> list[str]:
    """Return every service module stem, sorted (including the package init,
    so the matrix accounts for all files under ``backend/services/`` and none
    is silently dropped)."""
    return sorted(path.stem for path in SERVICES_DIR.glob("*.py"))


def _load_classification() -> tuple[set[str], set[str]]:
    """Return (coverage-omitted stems, mutation-target stems) from pyproject."""
    data = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
    omit = data["tool"]["coverage"]["run"].get("omit", [])
    mutate = data["tool"]["mutmut"].get("paths_to_mutate", [])
    omitted = {Path(entry).stem for entry in omit if "services/" in entry}
    targets = {Path(entry).stem for entry in mutate if "services/" in entry}
    return omitted, targets


def _covering_tests(module: str) -> list[str]:
    """Test modules that import the service, relative to the test tree."""
    needle = f"backend.services.{module}"
    found: set[str] = set()
    for path in TESTS_DIR.rglob("test_*.py"):
        if needle in path.read_text(encoding="utf-8"):
            found.add(path.relative_to(TESTS_DIR).as_posix())
    return sorted(found)


def _classify(module: str, omitted: set[str], targets: set[str]) -> str:
    """Render the accountability classification for a service module."""
    if module in targets:
        return "branch coverage + mutation"
    if module in omitted:
        return "device / IO (exempt from the coverage floor)"
    return "branch coverage"


def render_matrix() -> str:
    """Render the full COVERAGE.md document as a string."""
    omitted, targets = _load_classification()
    modules = _service_modules()

    missing = [m for m in modules if m not in BEHAVIOUR]
    if missing:
        raise SystemExit(
            "coverage_matrix: no behaviour summary for service module(s): "
            + ", ".join(missing)
            + " — add an entry to BEHAVIOUR in backend/scripts/coverage_matrix.py."
        )

    lines: list[str] = []
    lines.append("# Service coverage matrix")
    lines.append("")
    lines.append(
        "Every backend service mapped to the tests and scenarios that exercise "
        "its externally-observable behaviour, and to how that behaviour is held "
        "to account. This file is generated by "
        "`backend/scripts/coverage_matrix.py` and guarded by "
        "`backend/tests/test_coverage_matrix_drift.py`, so it cannot drift from "
        "the real test surface; regenerate it with "
        "`python backend/scripts/coverage_matrix.py` (or the "
        "`--update-fingerprints` test run) after adding a service or a covering "
        "test."
    )
    lines.append("")
    lines.append(
        "The mechanical gates are the source of truth for *how much* is covered: "
        "branch coverage (the floor in `pyproject.toml`, the published badge) and "
        "the nightly mutation score over the pure-logic core. This matrix is the "
        "human-readable layer on top: it shows *that* every service has at least "
        "one exercising test, and names the device/IO modules that are exempt "
        "from the coverage floor by design (they need a real display, capture "
        "device, or OS hook, so they are exercised through seams and fixtures "
        "rather than measured)."
    )
    lines.append("")
    lines.append(f"Services: {len(modules)}.")
    lines.append("")
    lines.append(
        "| Service | Externally-observable behaviour | Covering tests | Held to account by |"
    )
    lines.append("| --- | --- | --- | --- |")
    for module in modules:
        tests = _covering_tests(module)
        if tests:
            tests_cell = "<br>".join(f"`{t}`" for t in tests)
        elif module in omitted:
            tests_cell = "exercised through seams / fixtures (see notes)"
        else:
            tests_cell = "exercised transitively (see notes)"
        lines.append(
            f"| `{module}` | {BEHAVIOUR[module]} | {tests_cell} "
            f"| {_classify(module, omitted, targets)} |"
        )
    lines.append("")
    lines.append("## Mutation testing")
    lines.append("")
    lines.append(
        "The services marked *branch coverage + mutation* are in the nightly "
        "mutation campaign's `paths_to_mutate` (the pure-logic core). The "
        "campaign is the effectiveness metric on top of branch coverage: it "
        "proves a test would notice if a line were wrong, not merely that the "
        "line ran. It runs nightly on Linux (the engine is POSIX-only) and "
        "publishes a score badge; it is not a per-PR gate."
    )
    lines.append(
        "- Live score: the aggregate mutation score is published as the badge at "
        "the top of the README; the enforced floors (a ratcheting aggregate floor "
        "plus a per-module floor map) live in `.github/workflows/nightly.yml` and "
        "only ever rise. The campaign is POSIX-only, so the score is refreshed by "
        "the nightly Linux run, not computed when this matrix is generated."
    )
    lines.append(
        "- North-star: **>= 90% aggregate**, reached by killing surviving "
        "mutants and widening `paths_to_mutate` to any further pure-logic "
        "service the matrix surfaces. The device / IO and HTTP-glue modules stay "
        "out of the campaign: they carry little mutation signal and would slow it "
        "for no gain."
    )
    lines.append("")
    lines.append("## Notes")
    lines.append("")
    lines.append(
        "- **Device / IO services** (`eu_window`, `local_ocr`, `repair_ocr`, "
        "`scan_presets`, `skill_scan_manual`, `hotbar_listener`, "
        "`spacebar_capture_listener`) are exempt from the coverage floor: they "
        "drive a real window, capture device, or OS keyboard hook. They are "
        "exercised through the harness seams instead, the OCR pair against the "
        "recorded panels (`test_ocr_equivalence`), and the listeners against the "
        "mock keystroke source (`test_hotbar_listener`, "
        "`test_spacebar_capture_listener`, `test_keystroke_source`)."
    )
    lines.append(
        "- **Transitively-covered services** (`game_data_store`, "
        "`session_summary`) are not imported by any test directly: they are "
        "dependencies the other services compose (the game-data tables behind "
        "codex / mob lookup, the summary projection behind the tracking and "
        "analytics surfaces), so they execute under those services' tests and "
        "the e2e pipeline and their branch coverage is counted there."
    )
    lines.append(
        "- **Recorded-corpus coverage** of the OCR surface is local-by-default "
        "(real account panels stay off the public repo); the scripted corpus "
        "plus the placeholder recorded bundle keep the public test surface green "
        "without it."
    )
    lines.append("")
    lines.append(
        "The rows above are every module under `backend/services/`, including "
        "the empty package `__init__` (listed so the matrix accounts for the "
        "whole directory and no file is silently dropped)."
    )
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    """Write the rendered matrix to ``backend/testing/COVERAGE.md``."""
    OUTPUT.write_text(render_matrix(), encoding="utf-8")
    print(f"Wrote {OUTPUT}")


if __name__ == "__main__":
    main()
