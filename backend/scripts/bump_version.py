"""Bump the application version stamps in lock-step.

Sets the same version string in the three stamps that
:mod:`backend.scripts.check_version_stamps` governs:

- ``frontend/package.json`` (top-level ``version``),
- ``frontend/src-tauri/Cargo.toml`` (``[workspace.package] version``),
- ``frontend/src-tauri/entropia-orme/tauri.conf.json`` (top-level ``version``).

Edits are surgical: only the version token is rewritten, so file formatting,
key order, and comments are preserved and the JSON manifests are not
reserialised. The ``[workspace.package]`` edit is scoped to that table, so
``[workspace.dependencies]`` version pins are never touched.

``Cargo.lock``'s recorded member versions are intentionally left to refresh on
the next ``cargo`` invocation: the three stamps above are the parity contract
(see :mod:`backend.scripts.check_version_stamps`); the lock is a build artefact.
``CURRENT_TOS_VERSION`` is a separate namespace and is not touched, mirroring the
parity guard.

    python -m backend.scripts.bump_version 0.2.0
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from backend.scripts.check_version_stamps import (
    CARGO_TOML,
    PACKAGE_JSON,
    REPO_ROOT,
    TAURI_CONF,
    evaluate,
)

# Semver core with optional pre-release / build metadata. Numeric core parts
# reject leading zeros (01.2.3 is not valid semver), per the spec.
SEMVER_RE = re.compile(
    r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)

# The top-level "version": "..." token. count=1 in the substitution binds it to
# the first occurrence, which is the top-level object key in both manifests.
_JSON_VERSION_RE = re.compile(r'("version"\s*:\s*")[^"]*(")')

# version = "..." inside the [workspace.package] table only. The [^[]*? guard
# stops the match at the next table header, so [workspace.dependencies] pins are
# out of scope.
_CARGO_VERSION_RE = re.compile(
    r'(\[workspace\.package\][^\[]*?\bversion\s*=\s*")[^"]*(")', re.DOTALL
)


def _sub_once(pattern: re.Pattern[str], version: str, text: str, rel: str) -> str:
    new_text, count = pattern.subn(
        lambda m: f"{m.group(1)}{version}{m.group(2)}", text, count=1
    )
    if count != 1:
        raise SystemExit(f"bump-version: could not locate the version token in {rel}")
    return new_text


def set_version(repo_root: Path, version: str) -> None:
    """Rewrite all three stamps to ``version`` in place, preserving formatting."""
    for rel, pattern in (
        (PACKAGE_JSON, _JSON_VERSION_RE),
        (TAURI_CONF, _JSON_VERSION_RE),
        (CARGO_TOML, _CARGO_VERSION_RE),
    ):
        path = repo_root / rel
        # newline="" writes the string verbatim, so the LF line endings the repo
        # normalises to (.gitattributes eol=lf) are preserved rather than being
        # translated to CRLF by text-mode write on Windows.
        path.write_text(
            _sub_once(pattern, version, path.read_text(encoding="utf-8"), rel),
            encoding="utf-8",
            newline="",
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="target version, e.g. 0.2.0")
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to inspect (default: the project root)",
    )
    args = parser.parse_args(argv)

    if not SEMVER_RE.match(args.version):
        print(
            f"bump-version: {args.version!r} is not a valid semver (X.Y.Z).",
            file=sys.stderr,
        )
        return 2

    set_version(args.repo_root, args.version)

    # Self-verify through the parity guard's own logic so the bump cannot claim
    # success on a stamp it failed to reach.
    result = evaluate(args.repo_root)
    if result.ok and next(iter(result.versions)) == args.version:
        print(f"bump-version: all app version stamps set to {args.version}.")
        for path in result.stamps:
            print(f"  {path}")
        return 0

    print("bump-version: post-write parity check FAILED:", file=sys.stderr)
    for path, version in result.stamps.items():
        print(f"  {version}\t{path}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
