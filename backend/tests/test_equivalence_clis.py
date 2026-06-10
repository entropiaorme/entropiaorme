"""Unit coverage for the equivalence CLIs and the fixture generators.

The oracle CLIs (``normalize_cli``, ``cost_engine_cli``,
``static_tables_cli``, ``config_service_cli``) are driven by the
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

from backend.testing import (
    config_service_cli,
    cost_engine_cli,
    normalize_cli,
    static_tables_cli,
)
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


def test_static_tables_cli_serves_every_op(monkeypatch) -> None:
    """One sorted-keys JSON reply per request line, blanks skipped."""
    out = io.StringIO()
    requests = [
        {"op": "tt_value_at", "level": 123.45},
        {"op": "tt_value_of_gain", "from_level": 10, "to_level": 20},
        {"op": "levels_for_tt_value", "from_level": 10, "ped_value": 1.5},
        {"op": "max_tt_curve_level"},
        {"op": "get_codex_category", "skill_name": "Aim"},
        {"op": "build_rank_breakdown", "base_cost": 10.0, "codex_type": "MobLooter"},
    ]
    stdin = "\n".join(json.dumps(r) for r in requests) + "\n\n"
    monkeypatch.setattr(sys, "stdin", io.StringIO(stdin))
    monkeypatch.setattr(sys, "stdout", out)
    static_tables_cli.main()
    lines = out.getvalue().splitlines()
    assert len(lines) == len(requests)

    from backend.data import tt_value_curve

    assert json.loads(lines[0]) == tt_value_curve.tt_value_at(123.45)
    assert json.loads(lines[1]) == tt_value_curve.tt_value_of_gain(10, 20)
    assert json.loads(lines[2]) == tt_value_curve.levels_for_tt_value(10, 1.5)
    assert json.loads(lines[3]) == tt_value_curve.max_tt_curve_level()
    assert json.loads(lines[4]) == "cat1"
    breakdown = json.loads(lines[5])
    assert len(breakdown) == 25
    assert breakdown[4]["cat4Bonus"] is True
    # Replies serialise with sorted keys (the cross-language comparison form).
    assert lines[5].index('"cat4Bonus"') < lines[5].index('"category"')


def test_static_tables_cli_rejects_unknown_ops(monkeypatch) -> None:
    import pytest

    monkeypatch.setattr(sys, "stdin", io.StringIO('{"op": "no-such-op"}\n'))
    monkeypatch.setattr(sys, "stdout", io.StringIO())
    with pytest.raises(ValueError, match="unknown op"):
        static_tables_cli.main()


def test_config_service_cli_round_trips_with_the_sentinel(monkeypatch) -> None:
    """The round trip materialises stored files, applies updates, and
    projects the host-dependent default chat-log path to its sentinel."""
    out = io.StringIO()
    request = {
        "stored": {"extensionKey": 1, "player_name": "Kept"},
        "updates": [{"mob_tracking_tag": "tagged"}],
    }
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(request) + "\n\n"))
    monkeypatch.setattr(sys, "stdout", out)
    config_service_cli.main()
    reply = json.loads(out.getvalue().splitlines()[0])
    assert reply["state"]["chatlog_path"] == config_service_cli.CHATLOG_SENTINEL
    assert reply["state"]["mob_tracking_tag"] == "tagged"
    assert reply["file"].startswith('{\n  "extensionKey": 1')
    assert "<DEFAULT_CHATLOG>" in reply["file"]


def test_static_tables_cli_serves_the_game_data_ops(monkeypatch) -> None:
    out = io.StringIO()
    requests = [
        {"op": "game_counts"},
        {"op": "game_find", "endpoint": "mobs", "item_id": "no-such"},
        {"op": "mob_has", "species": "", "maturity": ""},
        {"op": "mob_suggest", "query": " ", "limit": 5},
    ]
    stdin = "\n".join(json.dumps(r) for r in requests) + "\n"
    monkeypatch.setattr(sys, "stdin", io.StringIO(stdin))
    monkeypatch.setattr(sys, "stdout", out)
    static_tables_cli.main()
    lines = out.getvalue().splitlines()
    counts = json.loads(lines[0])
    assert counts.get("mobs", 0) > 0, "the real snapshot catalogue loads"
    assert json.loads(lines[1]) is None
    assert json.loads(lines[2]) is False
    assert json.loads(lines[3]) == []


def test_static_tables_cli_serves_the_character_calc_ops(monkeypatch) -> None:
    out = io.StringIO()
    requests = [
        {
            "op": "profession_level",
            "skill_levels": {"Rifle": 1000, "Agility": 50},
            "profession": {
                "skills": [
                    {"skill": {"name": "Rifle"}, "weight": 5},
                    {"skill": {"name": "Agility"}, "weight": 2},
                ]
            },
        },
        {"op": "skill_rank", "level": 5, "ranks": []},
        {"op": "codex_next_reward", "skill_name": "No Such Skill", "current_level": 9},
    ]
    stdin = "\n".join(json.dumps(r) for r in requests) + "\n"
    monkeypatch.setattr(sys, "stdin", io.StringIO(stdin))
    monkeypatch.setattr(sys, "stdout", out)
    static_tables_cli.main()
    lines = out.getvalue().splitlines()
    assert json.loads(lines[0]) == 0.7
    assert json.loads(lines[1]) == "Unknown"
    assert json.loads(lines[2]) is None
