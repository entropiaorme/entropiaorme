"""Unit coverage for the equivalence CLIs and the fixture generators.

The two oracle CLIs (``normalize_cli``, ``cost_engine_cli``) are driven by the
Rust differential fuzzes over stdin/stdout; this pins their line protocol
directly under pytest so the contract is covered without spawning the Rust
side. The generator entry points (``table.write_fixture``,
``yml_family.write_mirrors``) are smoke-tested against a temp path so the
regeneration code stays exercised without rewriting the committed fixtures.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

from backend.testing import cost_engine_cli, normalize_cli
from backend.testing.equivalence import table, yml_family


def test_normalize_cli_server_mode(monkeypatch) -> None:
    """Server mode emits one normalised line per input line, blanks skipped."""
    out = io.StringIO()
    monkeypatch.setattr(
        sys,
        "stdin",
        io.StringIO(
            '{"a": 1.0}\n'
            '{"id": "11111111-1111-1111-1111-111111111111"}\n'
            "\n"  # blank line is skipped
        ),
    )
    monkeypatch.setattr(sys, "stdout", out)
    assert normalize_cli.main([]) == 0
    assert out.getvalue().splitlines() == ['{"a": 1.0}', '{"id": "<UUID_1>"}']


def test_normalize_cli_once_mode(monkeypatch) -> None:
    """``--once`` reads the whole input and writes one form, no trailing newline."""
    out = io.StringIO()
    monkeypatch.setattr(sys, "stdin", io.StringIO('{"b": 2.0, "a": 1.0}'))
    monkeypatch.setattr(sys, "stdout", out)
    assert normalize_cli.main(["--once"]) == 0
    assert out.getvalue() == '{"a": 1.0, "b": 2.0}'


def test_cost_engine_cli_normalises_the_cost_result(monkeypatch) -> None:
    """The cost CLI normalises ``cost_per_shot_from_props`` over a props line."""
    out = io.StringIO()
    props = {"weapon_entity": {"economy": {"decay": 0.05, "ammo_burn": 200}}}
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(props) + "\n"))
    monkeypatch.setattr(sys, "stdout", out)
    assert cost_engine_cli.main() == 0
    # decay 0.05 + ammo 2.0 at TT = 2.05.
    assert json.loads(out.getvalue().strip())["totalCostPerUse"] == 2.05


def test_table_write_fixture_round_trips(monkeypatch, tmp_path: Path) -> None:
    """``write_fixture`` regenerates the conformance fixture at its path."""
    target = tmp_path / "conformance.json"
    monkeypatch.setattr(table, "FIXTURE_PATH", target)
    table.write_fixture()
    assert target.read_text(encoding="utf-8") == table.serialize_table(
        table.build_table()
    )


def test_yml_family_write_mirrors_round_trips(monkeypatch, tmp_path: Path) -> None:
    """``write_mirrors`` regenerates every mirror under its directory."""
    monkeypatch.setattr(yml_family, "MIRROR_DIR", tmp_path)
    yml_family.write_mirrors()
    for stem, yml_path in yml_family.YML_GOLDENS.items():
        assert (tmp_path / f"{stem}.json").read_text(
            encoding="utf-8"
        ) == yml_family.mirror_text(yml_family.load_yml(yml_path))
