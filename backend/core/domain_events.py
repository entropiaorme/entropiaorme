"""Typed, frontend-facing domain events bridged across the IPC seam.

The in-process :class:`~backend.core.event_bus.EventBus` (see ``event_bus.py``)
carries low-level, string-typed, dict-payload events between backend services
(the ``EVENT_*`` constants in ``events.py``: a parsed combat line, a loot group,
a skill gain). Those topics are intra-backend wiring at the wrong granularity to
push to a webview: a frontend window does not want "a damage_dealt combat line",
it wants "the live session aggregates changed".

This module defines the coarse, *frontend-facing* domain events that are
forwarded over the SSE bridge (``GET /api/events``) and re-emitted onto the
Tauri event bus. Each is a Pydantic v2 model carrying a ``type`` discriminator,
so the wire format is a serde-compatible tagged JSON object. It maps line-for-
line onto a Rust ``#[serde(tag = "type")] enum DomainEvent { ... }`` at port
time, and the frontend listener contract is unchanged across that port.

Conventions, held deliberately so the Rust port is mechanical rather than a
redesign:

- ``type`` is a ``Literal`` discriminator carrying the domain-topic string
  verbatim (``"tracking.session.updated"``), so the bus-topic to envelope
  mapping is identity.
- ``event_version`` is a per-event-type schema version (additive-only field
  evolution bumps it), independent of the app's FastAPI version, so the frontend
  and a future Rust emitter can reason about shape evolution without coupling to
  the app version.
- ``occurred_at`` is an ISO-8601 UTC string (never the bus's raw float), ported
  to Rust ``chrono::DateTime<Utc>`` -> RFC3339. The conversion is re-implemented
  here (``to_iso_utc``) rather than imported from the routers layer so ``core``
  does not depend upward on ``routers``.
- payload field names are spelled camelCase literally (house style: no alias
  generators anywhere; Rust uses ``#[serde(rename_all = "camelCase")]``). The
  envelope never spreads a raw bus dict, so snake_case keys and float timestamps
  cannot leak onto the wire.
- envelope and payload models set ``extra="forbid"``. We are the *emitter* and
  construct these explicitly, so the wire contract is closed: an undeclared key
  is a bug the schema-drift golden must catch. This is the deliberate opposite
  of the read-surface ``_Loose`` base (``routers/response_models.py``), whose
  job is handler passthrough.

The first cut is **push-to-pull**: a payload is a minimal invalidation signal
(which session, active vs idle, why) and the window re-hydrates the full shape
via the snapshot GET. That keeps the ETag/304 snapshot as the single source of
shape and minimises the Rust-port serialisation surface. Push-with-data is
reserved for a latency-sensitive topic (e.g. scan-status capture progress) if
it is ever warranted.

One layer does **not** port one-to-one, called out so the "maps line-for-line"
claim above stays honest: these typed envelopes travel on the shared
:class:`~backend.core.event_bus.EventBus`, whose ``publish`` is ``Any``-typed and
also carries the low-level dict ``EVENT_*`` topics, so "a typed instance on a
domain topic" is a producer-side convention the type checker cannot enforce (the
SSE hub re-validates at runtime for exactly this reason). A monomorphic Rust
``broadcast::Sender<DomainEvent>`` makes that convention a compile-time guarantee
and turns the hub's runtime-defensive branches into dead code; bringing that
enforcement forward into Python (a producer-side ``publish_domain`` narrowing) is
a noted follow-up, not done here.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter

# Domain-topic strings. These double as the ``type`` discriminator values and as
# the Tauri event topic the relay emits, so the mapping is identity. They are
# distinct from the intra-backend ``EVENT_*`` constants in ``events.py``.
TOPIC_TRACKING_SESSION_UPDATED: Literal["tracking.session.updated"] = (
    "tracking.session.updated"
)
TOPIC_SCAN_STATUS_CHANGED: Literal["scan.status.changed"] = "scan.status.changed"


def to_iso_utc(ts: float | None) -> str | None:
    """Render a Unix timestamp (SQLite REAL / bus float) as ISO-8601 UTC.

    Mirrors the routers-layer ``_ts_to_iso`` boundary, re-implemented here so
    ``core`` carries no upward dependency on ``routers``.
    """
    if ts is None:
        return None
    return datetime.fromtimestamp(ts, tz=UTC).isoformat()


class _EventModel(BaseModel):
    """Base for the envelope and payload models: a closed wire contract."""

    model_config = ConfigDict(extra="forbid")


class TrackingSessionUpdatedPayload(_EventModel):
    """Push-to-pull invalidation signal for the live tracking session.

    Carries only what the frontend needs to route the change: which session, the
    coarse active/idle state (so the relay can drive the existing
    ``tracking-state-changed`` listener verbatim), and why it fired. The window
    re-hydrates the full readout via the snapshot GET.
    """

    sessionId: str | None = None
    status: Literal["active", "idle"]
    reason: Literal["started", "updated", "stopped"]


class TrackingSessionUpdated(_EventModel):
    """The session aggregates changed (started, advanced a tick, or stopped)."""

    type: Literal["tracking.session.updated"] = TOPIC_TRACKING_SESSION_UPDATED
    event_version: int = 1
    occurred_at: str | None = None
    payload: TrackingSessionUpdatedPayload


class ScanStatusChangedPayload(_EventModel):
    """Push-to-pull invalidation signal for the manual skill-scan flow.

    Carries only the coarse phase so a listener can route (and a human can read
    the wire). The window re-hydrates the full status via the scan-status GET on
    every frame, so per-page capture/OCR progress liveness comes from the
    hydration rather than from widening this payload: the emitter still fires on
    every discrete progress change (see ``SkillScanManual``), but the wire stays
    a minimal invalidation signal.
    """

    phase: Literal["idle", "capturing", "processing", "awaiting_review"]


class ScanStatusChanged(_EventModel):
    """The manual skill-scan status changed (phase transition or capture/OCR progress)."""

    type: Literal["scan.status.changed"] = TOPIC_SCAN_STATUS_CHANGED
    event_version: int = 1
    occurred_at: str | None = None
    payload: ScanStatusChangedPayload


# The discriminated union of every frontend-facing domain event.
# ``Field(discriminator="type")`` selects the member by its ``type`` tag, so a
# new member changes neither the existing members nor the wire format, and the
# call sites (the bus publish, the SSE serialiser, the schema golden) route
# through the union unchanged. The second member (scan) is the proof the
# serde-tagged-union spine generalises beyond tracking, which is what maps
# mechanically onto a Rust ``#[serde(tag = "type")] enum DomainEvent { ... }``.
DomainEvent = Annotated[
    TrackingSessionUpdated | ScanStatusChanged,
    Field(discriminator="type"),
]

DomainEventAdapter: TypeAdapter[TrackingSessionUpdated | ScanStatusChanged] = (
    TypeAdapter(DomainEvent)
)
