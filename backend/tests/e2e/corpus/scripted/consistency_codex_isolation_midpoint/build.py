"""DSL build script for the consistency_codex_isolation_midpoint scenario.

Regenerate the scenario's two segment files via::

    python -m backend.tests.e2e.corpus.scripted.consistency_codex_isolation_midpoint.build

The post-segment contains combat plus a loot tick so the consistency
property is exercised against a meaningfully-populated chat stream;
the codex surface's projection must stay unchanged across the interval.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def _write_segment(lines: list[str], target: Path) -> None:
    """Write a segment's chat lines into the scenario directory."""
    target.write_text("".join(lines), encoding="utf-8")


def build() -> Path:
    """Build both segment files into the scenario's own directory."""

    target_dir = Path(__file__).parent

    pre = Scenario(name="consistency_codex_isolation_midpoint_pre")
    pre.at("2026-05-19 13:00:00")
    pre.combat.damage_dealt(18.0)

    _write_segment(pre.lines(), target_dir / "chat_replay.log")

    post = Scenario(name="consistency_codex_isolation_midpoint_post")
    post.at("2026-05-19 13:00:10")
    post.combat.damage_dealt(25.0)
    post.tick()
    post.loot.received("Bone Frame", value_ped=2.10)

    _write_segment(post.lines(), target_dir / "chat_replay_after.log")

    return target_dir


if __name__ == "__main__":
    build()
