"""Canonical DB snapshot for golden-file regression diffing.

A snapshot runs a fixed catalogue of SELECT queries against the
tracking schema and renders the rows into a single JSON document under
the same normalisation rules as the event-stream fingerprint. The
catalogue is the contract: a change that introduces a new
externally-observable table appends a ``TableSpec`` here, and existing
goldens grow to cover the new surface on the next ``--update-fingerprints``
pass.

The shared ``Normalizer`` instance keeps UUIDs and timestamps mapped
to the same symbols across the fingerprint and the snapshot, which is
what makes the diff renderer's "session ``<UUID_1>`` lost a kill"
output meaningful when read against the event stream.
"""

from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from backend.testing.fingerprint import Normalizer


@dataclass(frozen=True)
class TableSpec:
    """One entry in the snapshot catalogue.

    ``query`` selects the columns the harness cares about (explicit
    column lists, not ``SELECT *``, so a schema-added column does not
    silently mutate goldens). ``order_by`` enforces stable row order;
    fields chosen are those that combine to a natural per-row identity
    so the snapshot reads cleanly when diffed.
    """

    name: str
    query: str
    order_by: tuple[str, ...] = ()


# Catalogue of tables the initial scenario touches. Extend as new
# scenarios bring fresh tables into scope.
#
# Each ``order_by`` is chosen for cross-run determinism: rows must
# arrive in the same order regardless of the wall-clock time the test
# ran and regardless of which random UUIDs the tracker generated for
# session and kill ids. Two anchors satisfy this: stable source-side
# timestamps (chatlog-parsed instants, which match across runs of the
# same scenario) and SQLite's per-row ``rowid`` (assigned by insertion
# order, which is itself deterministic because events stream in chat
# order). The tiebreaker pattern is ``(natural-time-key, rowid)`` so
# the snapshot reads chronologically while keeping equal-time rows in
# a stable order. Never order by a raw UUID column: those values vary
# per run and only get normalised AFTER the SQL sort runs, which
# silently flips row order across runs.
CATALOGUE: tuple[TableSpec, ...] = (
    TableSpec(
        name="tracking_sessions",
        query=(
            "SELECT id, started_at, ended_at, is_active, "
            "COALESCE(heal_cost, 0.0) AS heal_cost, "
            "COALESCE(dangling_cost, 0.0) AS dangling_cost "
            "FROM tracking_sessions"
        ),
        order_by=("rowid",),
    ),
    TableSpec(
        name="kills",
        query=(
            "SELECT id, session_id, mob_name, mob_species, mob_maturity, "
            "timestamp, shots_fired, damage_dealt, damage_taken, "
            "critical_hits, cost_ped, enhancer_cost, loot_total_ped, "
            "is_global, is_hof "
            "FROM kills"
        ),
        order_by=("timestamp", "rowid"),
    ),
    TableSpec(
        name="kill_loot_items",
        query=(
            "SELECT kli.kill_id, kli.item_name, kli.quantity, "
            "kli.value_ped, kli.is_enhancer_shrapnel "
            "FROM kill_loot_items kli "
            "JOIN kills k ON kli.kill_id = k.id"
        ),
        order_by=("k.timestamp", "kli.rowid"),
    ),
    TableSpec(
        name="kill_tool_stats",
        query=(
            "SELECT kts.kill_id, kts.tool_name, kts.shots_fired, "
            "kts.damage_dealt, kts.critical_hits, kts.cost_per_shot "
            "FROM kill_tool_stats kts "
            "JOIN kills k ON kts.kill_id = k.id"
        ),
        order_by=("k.timestamp", "kts.rowid"),
    ),
    TableSpec(
        name="ledger_entries",
        query=("SELECT id, date, type, description, amount, tag FROM ledger_entries"),
        order_by=("rowid",),
    ),
    TableSpec(
        name="notable_events",
        query=(
            "SELECT session_id, kill_id, event_type, mob_or_item, "
            "value_ped, timestamp "
            "FROM notable_events"
        ),
        order_by=("timestamp", "rowid"),
    ),
)


def capture(
    db: sqlite3.Connection,
    normalizer: Normalizer | None = None,
    catalogue: tuple[TableSpec, ...] | None = None,
) -> dict[str, Any]:
    """Run every catalogue query, normalise each row, return one dict.

    Tables that do not exist (e.g. a fresh in-memory DB whose schema
    has not initialised every table the catalogue knows about) appear
    as empty lists rather than raising, so the snapshot stays
    structurally consistent across scenarios that exercise different
    subsets of the schema.
    """
    norm = normalizer or Normalizer()
    cat = catalogue or CATALOGUE
    out: dict[str, Any] = {}
    for spec in cat:
        if not _table_exists(db, spec.name):
            out[spec.name] = []
            continue
        rows = _fetch_rows(db, spec)
        out[spec.name] = [norm.normalize(row) for row in rows]
    return out


def serialize(snapshot: dict[str, Any]) -> str:
    """Render the snapshot as canonical JSON (sorted keys, 2-space
    indent, trailing newline)."""
    return json.dumps(snapshot, sort_keys=True, indent=2, ensure_ascii=False) + "\n"


def write(snapshot: dict[str, Any], path: Path) -> None:
    """Persist the snapshot to ``path`` (parents created on demand)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    # newline="\n" so a Windows regen writes LF directly, matching the repo's
    # `*.json eol=lf` policy rather than emitting CRLF in text mode.
    path.write_text(serialize(snapshot), encoding="utf-8", newline="\n")


def _fetch_rows(db: sqlite3.Connection, spec: TableSpec) -> list[dict[str, Any]]:
    """Execute ``spec.query`` with its ``order_by`` appended and return
    each row as a ``{column: value}`` dict so the caller can hand the
    rows straight to the ``Normalizer`` without re-mapping positional
    columns."""
    query = spec.query
    if spec.order_by:
        query += " ORDER BY " + ", ".join(spec.order_by)
    cursor = db.execute(query)
    columns = [c[0] for c in cursor.description]
    return [dict(zip(columns, row, strict=True)) for row in cursor.fetchall()]


def _table_exists(db: sqlite3.Connection, name: str) -> bool:
    """Return whether ``name`` exists as a table in the connection's
    schema. Lets the snapshot stay structurally consistent when a
    scenario's in-memory DB has not initialised every table the
    catalogue knows about."""
    row = db.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        (name,),
    ).fetchone()
    return row is not None
