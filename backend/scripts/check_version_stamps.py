"""Assert the application's version stamps move in lock-step.

The app version is written in three places that a release bump must keep
identical:

- ``frontend/package.json`` (``version``),
- ``frontend/src-tauri/Cargo.toml`` (``[package] version``),
- ``frontend/src-tauri/tauri.conf.json`` (``version``).

If a bump updates some but not all of these, the packaged artefacts disagree
about what version they are. This guard reads all three and fails when they are
not identical, so the drift is caught deterministically rather than shipped.

``CURRENT_TOS_VERSION`` in ``frontend/src/lib/tos.ts`` is deliberately NOT part
of this check. It is a separate namespace: it versions the terms-of-service
*document* and bumps only when those terms change, independently of the
application release (today it is ``1.0`` while the app is ``0.1.0``). Coupling
the two would be wrong, and would force a spurious ToS-acceptance reset on every
release.

Run it directly, or import :func:`evaluate` to drive the same logic from a
test::

    python -m backend.scripts.check_version_stamps
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

# The three app-version stamp locations, each with how to read its version.
PACKAGE_JSON = "frontend/package.json"
CARGO_TOML = "frontend/src-tauri/Cargo.toml"
TAURI_CONF = "frontend/src-tauri/tauri.conf.json"


def _read_json_version(path: Path) -> str:
    data = json.loads(path.read_text(encoding="utf-8"))
    return str(data["version"])


def _read_cargo_version(path: Path) -> str:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return str(data["package"]["version"])


def read_stamps(repo_root: Path) -> dict[str, str]:
    """Return each stamp's repo-relative path mapped to its version string."""
    return {
        PACKAGE_JSON: _read_json_version(repo_root / PACKAGE_JSON),
        CARGO_TOML: _read_cargo_version(repo_root / CARGO_TOML),
        TAURI_CONF: _read_json_version(repo_root / TAURI_CONF),
    }


@dataclass(frozen=True)
class Evaluation:
    """Outcome of comparing the version stamps."""

    stamps: dict[str, str]

    @property
    def versions(self) -> set[str]:
        return set(self.stamps.values())

    @property
    def ok(self) -> bool:
        """True when every stamp carries the same version string."""
        return len(self.versions) == 1


def evaluate(repo_root: Path) -> Evaluation:
    return Evaluation(stamps=read_stamps(repo_root))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to inspect (default: the project root)",
    )
    args = parser.parse_args(argv)

    result = evaluate(args.repo_root)

    if result.ok:
        (version,) = tuple(result.versions)
        print(f"check-version-stamps: all app version stamps agree at {version}.")
        for path in result.stamps:
            print(f"  {path}")
        return 0

    print(
        "check-version-stamps: the application version stamps disagree. A "
        "release bump must update all of them in lock-step:\n",
        file=sys.stderr,
    )
    for path, version in result.stamps.items():
        print(f"  {version}\t{path}", file=sys.stderr)
    print(
        "\nUpdate every stamp to the same version. (CURRENT_TOS_VERSION in "
        "frontend/src/lib/tos.ts is a separate namespace and is intentionally "
        "not part of this check.)",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
