"""DSL build script for the ``empty_session`` scenario.

Regenerate the scenario's ``chat_replay.log`` via::

    python -m backend.tests.e2e.corpus.scripted.empty_session.build

The script emits zero lines (the scenario's whole point), so the
file is created empty. Goldens regenerate via the standard
``pytest --update-fingerprints`` workflow.
"""

from __future__ import annotations

from pathlib import Path

from backend.testing.dsl import Scenario


def build() -> Path:
    """Build the scenario's chat_replay.log into its own directory."""

    scenario = Scenario(name="empty_session")
    # No events: the scenario's job is to pin the empty-session path.
    return scenario.write(Path(__file__).parent)


if __name__ == "__main__":
    build()
