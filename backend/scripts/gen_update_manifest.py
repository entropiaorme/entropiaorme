"""Generate the Tauri updater manifest (``latest.json``) for a release.

The Tauri 2 updater reads a static, per-channel ``latest.json`` describing the
newest release: a top-level ``version`` (plus optional ``notes`` and
``pub_date``) and a ``platforms`` map keyed by ``"{target}-{arch}"`` (e.g.
``windows-x86_64``). Each platform entry carries the *full contents* of the
artefact's minisign ``.sig`` file and the URL the updater downloads from.

The trust model lives in those two fields: the client verifies the signature
over the downloaded artefact bytes against the public key embedded in the app
before running them, so the manifest itself need not be signed. This script only
assembles the manifest; the ``.sig`` is produced by signing the artefact during
the build (see ``scripts/build-installer.ps1``).

The signed artefact is the bare per-user MSI (what the updater runs via
``msiexec`` to perform an in-place upgrade), not the Burn ``setup.exe`` used for
first install; ``--url`` must point at that MSI release asset.

    python -m backend.scripts.gen_update_manifest \
        --version 0.2.0 \
        --signature-file dist/entropiaorme-0.2.0-x64.msi.sig \
        --url https://github.com/entropiaorme/entropiaorme/releases/download/v0.2.0/entropiaorme-0.2.0-x64.msi \
        --notes-file notes.md --pub-date 2026-06-24T00:00:00Z \
        --output dist/latest.json
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# The single target this Windows-only app ships. Reserved as a parameter so an
# arm64 slot is a one-line change if cross-architecture builds ever happen.
DEFAULT_TARGET = "windows-x86_64"


def build_manifest(
    version: str,
    signature: str,
    url: str,
    *,
    notes: str | None = None,
    pub_date: str | None = None,
    target: str = DEFAULT_TARGET,
) -> dict:
    """Assemble the updater manifest dict.

    ``signature`` is the full text content of the artefact's ``.sig`` file, not a
    path. ``notes`` / ``pub_date`` are omitted from the manifest when ``None``.
    """
    if not version:
        raise ValueError("version must be a non-empty string")
    if not signature.strip():
        raise ValueError("signature must be non-empty (the artefact's .sig contents)")
    if not url.startswith("https://"):
        raise ValueError(f"url must be an https:// URL, got {url!r}")

    manifest: dict = {"version": version}
    if notes is not None:
        manifest["notes"] = notes
    if pub_date is not None:
        manifest["pub_date"] = pub_date
    manifest["platforms"] = {target: {"signature": signature, "url": url}}
    return manifest


def render(manifest: dict) -> str:
    """Serialise the manifest as pretty JSON with a trailing newline (LF)."""
    return json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="release version, e.g. 0.2.0")
    parser.add_argument(
        "--signature-file",
        required=True,
        type=Path,
        help="path to the artefact's minisign .sig file (its full contents are embedded)",
    )
    parser.add_argument(
        "--url",
        required=True,
        help="https URL the updater downloads the signed artefact from",
    )
    parser.add_argument("--notes", help="release notes (Markdown) to embed")
    parser.add_argument(
        "--notes-file",
        type=Path,
        help="path to a file whose contents become the release notes (overrides --notes)",
    )
    parser.add_argument("--pub-date", help="publish date, ISO 8601 (e.g. 2026-06-24T00:00:00Z)")
    parser.add_argument(
        "--target",
        default=DEFAULT_TARGET,
        help=f"platform key (default: {DEFAULT_TARGET})",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="write the manifest here (default: stdout)",
    )
    args = parser.parse_args(argv)

    if not args.signature_file.is_file():
        print(
            f"gen-update-manifest: signature file not found: {args.signature_file}",
            file=sys.stderr,
        )
        return 2
    signature = args.signature_file.read_text(encoding="utf-8")

    notes = args.notes
    if args.notes_file is not None:
        if not args.notes_file.is_file():
            print(
                f"gen-update-manifest: notes file not found: {args.notes_file}",
                file=sys.stderr,
            )
            return 2
        notes = args.notes_file.read_text(encoding="utf-8")

    try:
        manifest = build_manifest(
            args.version,
            signature,
            args.url,
            notes=notes,
            pub_date=args.pub_date,
            target=args.target,
        )
    except ValueError as err:
        print(f"gen-update-manifest: {err}", file=sys.stderr)
        return 2

    output = render(manifest)
    if args.output is not None:
        # newline="" preserves the LF the render emits rather than translating to
        # CRLF on Windows, matching the repo's eol=lf normalisation.
        args.output.write_text(output, encoding="utf-8", newline="")
        print(f"gen-update-manifest: wrote {args.output}")
    else:
        sys.stdout.write(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
