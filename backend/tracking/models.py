"""In-memory data models for tracking sessions, kills, and combat."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class LootItem:
    """A single item received from a loot drop."""

    item_name: str
    quantity: int
    value_ped: float
    is_enhancer_shrapnel: bool = False


@dataclass
class ToolStats:
    """Per-tool damage statistics within a kill."""

    tool_name: str
    shots_fired: int = 0
    damage_dealt: float = 0.0
    critical_hits: int = 0
    cost_per_shot: float = 0.0  # From equipment library


@dataclass
class Kill:
    """A single kill — one loot group with its accumulated combat stats.

    Created when a loot group arrives. The accumulated shots/cost since the
    previous kill (or session start) are snapshotted into this record.
    mob_name is stamped from the current manual mob or free-text tag.
    """

    id: str
    session_id: str
    mob_name: str  # snapshot from manual/tag state; "Unknown" if unset
    mob_species: str = ""
    mob_maturity: str = ""
    timestamp: float = 0.0  # epoch — when loot arrived

    # Accumulated combat stats since last kill
    shots_fired: int = 0
    damage_dealt: float = 0.0
    damage_taken: float = 0.0
    critical_hits: int = 0
    cost_ped: float = 0.0  # total weapon cost (sum of cost_per_shot × shots per tool)

    # Enhancer cost accumulated during this kill's shots
    enhancer_cost: float = 0.0

    # Loot
    loot_total_ped: float = 0.0
    loot_items: list[LootItem] = field(default_factory=list)

    # Per-tool tracking
    tool_stats: dict[str, ToolStats] = field(default_factory=dict)

    # Notable event flags
    is_global: bool = False
    is_hof: bool = False


@dataclass
class TrackingSession:
    """A tracking session — started/stopped by the user."""

    id: str
    start_time: datetime = field(default_factory=datetime.now)
    end_time: datetime | None = None
    kills: list[Kill] = field(default_factory=list)
    dangling_cost: float = 0.0  # unresolved shots at session end


@dataclass(frozen=True)
class ActiveSessionView:
    """Immutable view of the active-session readout.

    Computed by ``HuntTracker.snapshot`` under the tracker's own ownership and
    returned as a detached value, so a caller on the web thread never iterates
    the live ``kills`` list or the in-progress accumulator while the chat-log
    thread mutates them. Maps onto a Rust ``struct`` returned from the tracking
    actor's snapshot query: owned data out, no shared reference in.

    The notable-event feed is carried as raw rows rather than formatted
    entries: the presentation mapping (category/label/description) lives in the
    HTTP layer, so the owner stays free of wire-format concerns.
    """

    session_id: str
    started_at: str
    kill_count: int
    elapsed: int
    cost: float
    returns: float
    pes: float
    net: float
    return_rate: float
    damage_dealt_total: float
    weapon_damage_dealt: float
    weapon_cost: float
    shots_fired_total: int
    critical_hits_total: int
    max_damage: float
    globals_count: int
    hofs_count: int
    latest_kill_loot: float | None
    multiplier_last: float | None
    multiplier_avg: float | None
    multiplier_max: float | None
    multiplier_history: tuple[float, ...]
    cumulative_net_history: tuple[float, ...]
    current_mob: str | None
    mob_source: str | None
    mob_entry_mode: str
    notable_event_rows: tuple[tuple[str, str, float, float | None], ...]
    warnings: tuple[str, ...]


@dataclass(frozen=True)
class TrackingReadout:
    """Immutable view of the whole tracking readout the owner can supply.

    ``active`` is the session discriminator: ``None`` when no session is
    running, an ``ActiveSessionView`` otherwise (maps onto a Rust
    ``Option<ActiveSessionView>``). ``current_tool`` (the detected active
    weapon) is meaningful in both states. The HTTP layer merges configuration-
    and runtime-derived fields (attribution mode, the repair-OCR flag, whether
    the hotbar listener is running) around this owned value to build the wire
    response, since those are not the tracker's to own.
    """

    current_tool: str | None
    active: ActiveSessionView | None
