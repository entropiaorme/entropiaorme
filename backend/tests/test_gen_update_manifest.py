"""Tests for the Tauri updater-manifest generator.

The generator (``backend/scripts/gen_update_manifest.py``) assembles the static
``latest.json`` the Tauri 2 updater reads: a ``version`` (plus optional
``notes`` / ``pub_date``) and a ``platforms`` map whose entry carries the full
``.sig`` contents and the artefact URL. These tests pin the manifest shape, the
field-omission behaviour, the input validation, and the CLI's file handling
(reading the signature, embedding notes from a file, the missing-file exit).
"""

from __future__ import annotations

import json
from pathlib import Path

from backend.scripts import gen_update_manifest as gen


def test_build_manifest_full_shape() -> None:
    manifest = gen.build_manifest(
        "0.2.0",
        "untrusted comment: sig\nABCDEF==",
        "https://example.com/app.msi",
        notes="Fixes things.",
        pub_date="2026-06-24T00:00:00Z",
    )
    assert manifest["version"] == "0.2.0"
    assert manifest["notes"] == "Fixes things."
    assert manifest["pub_date"] == "2026-06-24T00:00:00Z"
    platform = manifest["platforms"]["windows-x86_64"]
    # The full .sig contents are embedded verbatim, not a path.
    assert platform["signature"] == "untrusted comment: sig\nABCDEF=="
    assert platform["url"] == "https://example.com/app.msi"


def test_optional_fields_omitted_when_absent() -> None:
    manifest = gen.build_manifest("0.2.0", "sig", "https://example.com/app.msi")
    assert "notes" not in manifest
    assert "pub_date" not in manifest
    assert set(manifest) == {"version", "platforms"}


def test_custom_target_key() -> None:
    manifest = gen.build_manifest(
        "0.2.0", "sig", "https://example.com/app.msi", target="windows-aarch64"
    )
    assert "windows-aarch64" in manifest["platforms"]


def test_rejects_empty_version() -> None:
    try:
        gen.build_manifest("", "sig", "https://example.com/app.msi")
    except ValueError:
        pass
    else:  # pragma: no cover - the assert below is the failure signal
        raise AssertionError("empty version must raise")


def test_rejects_blank_signature() -> None:
    try:
        gen.build_manifest("0.2.0", "   \n  ", "https://example.com/app.msi")
    except ValueError:
        pass
    else:  # pragma: no cover
        raise AssertionError("blank signature must raise")


def test_rejects_non_https_url() -> None:
    try:
        gen.build_manifest("0.2.0", "sig", "http://example.com/app.msi")
    except ValueError:
        pass
    else:  # pragma: no cover
        raise AssertionError("a non-https url must raise (the manifest is fetched over TLS)")


def test_render_round_trips_as_json() -> None:
    manifest = gen.build_manifest("0.2.0", "sig", "https://example.com/app.msi")
    text = gen.render(manifest)
    assert text.endswith("\n")
    assert json.loads(text) == manifest


def test_cli_reads_signature_and_writes_output(tmp_path: Path) -> None:
    sig = tmp_path / "app.msi.sig"
    sig.write_text("untrusted comment: x\nSIGNATUREBYTES==\n", encoding="utf-8")
    notes = tmp_path / "notes.md"
    notes.write_text("## Release\nGood stuff.", encoding="utf-8")
    out = tmp_path / "latest.json"

    rc = gen.main(
        [
            "--version",
            "0.2.0",
            "--signature-file",
            str(sig),
            "--url",
            "https://example.com/app.msi",
            "--notes-file",
            str(notes),
            "--pub-date",
            "2026-06-24T00:00:00Z",
            "--output",
            str(out),
        ]
    )

    assert rc == 0
    manifest = json.loads(out.read_text(encoding="utf-8"))
    assert manifest["version"] == "0.2.0"
    assert manifest["notes"] == "## Release\nGood stuff."
    assert manifest["pub_date"] == "2026-06-24T00:00:00Z"
    entry = manifest["platforms"]["windows-x86_64"]
    assert entry["signature"] == "untrusted comment: x\nSIGNATUREBYTES==\n"
    assert entry["url"] == "https://example.com/app.msi"


def test_cli_missing_signature_file_exits_2(tmp_path: Path) -> None:
    rc = gen.main(
        [
            "--version",
            "0.2.0",
            "--signature-file",
            str(tmp_path / "nope.sig"),
            "--url",
            "https://example.com/app.msi",
        ]
    )
    assert rc == 2


def test_cli_non_https_url_exits_2(tmp_path: Path) -> None:
    sig = tmp_path / "app.msi.sig"
    sig.write_text("sig", encoding="utf-8")
    rc = gen.main(
        [
            "--version",
            "0.2.0",
            "--signature-file",
            str(sig),
            "--url",
            "http://insecure.example.com/app.msi",
        ]
    )
    assert rc == 2
