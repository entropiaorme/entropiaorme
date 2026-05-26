"""Pydantic response models for the API's read surface.

These describe the JSON shapes the handlers already return; they do not change
behaviour. Two deliberate conventions make that guarantee hold:

- Every model sets ``extra="allow"``. A handler may return keys a model does not
  enumerate; those pass through untouched rather than being dropped, so adding a
  model can never silently truncate a response. The enumerated fields are the
  ones the contract pins a type to.
- The polymorphic status/live models mark all non-discriminator fields optional,
  and their routes serialise with ``response_model_exclude_unset=True`` so only
  the keys the handler actually set appear. Without that, the lean ``unavailable``
  and ``idle`` shapes would gain a wall of explicit nulls.

The schema these produce is what the contract suite (``test_api_contract.py``)
validates real responses against.
"""

from __future__ import annotations

from typing import Any

from pydantic import BaseModel, ConfigDict


class _Loose(BaseModel):
    """Base that lets undeclared keys pass through serialisation untouched."""

    model_config = ConfigDict(extra="allow")


# ---------------------------------------------------------------------------
# Notable events (shared by /tracking/recent-events and the live feed)
# ---------------------------------------------------------------------------


class NotableEvent(_Loose):
    """A dashboard activity-feed entry.

    Covers both the notable-event payload (global/HoF/quest) and the tracker
    warning entry, which share ``type``/``description``/``value`` and differ in
    the optional ``eventType``/``timestamp``/``id`` keys.
    """

    type: str
    description: str
    value: float
    eventType: str | None = None
    timestamp: str | None = None
    id: str | None = None


# ---------------------------------------------------------------------------
# Tracking status (/tracking/status): unavailable | idle | active
# ---------------------------------------------------------------------------


class TrackingStatus(_Loose):
    status: str
    # Shared by idle + active.
    hotbarListenerActive: bool | None = None
    weaponAttribution: str | None = None
    repairOcrEnabled: bool | None = None
    endOfSessionArmourReminderEnabled: bool | None = None
    mobEntryMode: str | None = None
    currentMob: str | None = None
    mobSource: str | None = None
    # Active only.
    session_id: str | None = None
    started_at: str | None = None
    kill_count: int | None = None
    cost: float | None = None
    returns: float | None = None
    pes: float | None = None
    returnRate: float | None = None
    damageDealtTotal: float | None = None
    weaponDamageDealt: float | None = None
    weaponCost: float | None = None
    shotsFiredTotal: int | None = None
    criticalHitsTotal: int | None = None
    maxDamage: float | None = None
    globalsCount: int | None = None
    hofsCount: int | None = None
    latestKillLoot: float | None = None
    multiplierLast: float | None = None
    multiplierAvg: float | None = None
    multiplierMax: float | None = None
    multiplierHistory: list[float] | None = None
    cumulativeNetHistory: list[float] | None = None


# ---------------------------------------------------------------------------
# Tracking live (/tracking/live): unavailable | idle | active (overlay feed)
# ---------------------------------------------------------------------------


class TrackingLive(_Loose):
    status: str
    # Shared by idle + active.
    weaponAttribution: str | None = None
    repairOcrEnabled: bool | None = None
    endOfSessionArmourReminderEnabled: bool | None = None
    mobEntryMode: str | None = None
    currentMob: str | None = None
    mobSource: str | None = None
    currentTool: str | None = None
    trifectaAttribution: dict[str, Any] | None = None
    # Active only.
    sessionId: str | None = None
    elapsed: int | None = None
    killCount: int | None = None
    kills: int | None = None
    cost: float | None = None
    returns: float | None = None
    pes: float | None = None
    net: float | None = None
    returnRate: float | None = None
    recentEvents: list[NotableEvent] | None = None


# ---------------------------------------------------------------------------
# Analytics overview (/analytics/overview): stable shape
# ---------------------------------------------------------------------------


class ReturnsBreakdown(_Loose):
    lootTt: float
    pes: float
    codexPes: float
    questPes: float
    ledger: dict[str, float]


class LossesBreakdown(_Loose):
    trackingCost: float
    cycledBreakdown: Any
    ledger: dict[str, float]


class TimelinePoint(_Loose):
    date: str
    lootTt: float
    pes: float
    codexPes: float
    questPes: float
    ledgerGains: dict[str, Any]
    trackingCost: float
    ledgerLosses: dict[str, Any]


class MonthlyPoint(_Loose):
    month: str
    lootTt: float
    pes: float
    codexPes: float
    questPes: float
    ledgerGains: dict[str, Any]
    trackingCost: float
    ledgerLosses: dict[str, Any]


class AnalyticsOverview(_Loose):
    totalReturnRate: float
    trend: str
    returnsBreakdown: ReturnsBreakdown
    lossesBreakdown: LossesBreakdown
    totalGains: float
    totalLosses: float
    timeline: list[TimelinePoint]
    monthlyBreakdown: list[MonthlyPoint]


# ---------------------------------------------------------------------------
# Character prospect (/character/prospect): result | error union
# ---------------------------------------------------------------------------


class CharacterProspect(_Loose):
    """Forecast result or a not-found error shape.

    The error branch sets ``error``/``rows``/``warnings``; the result branch
    omits ``error`` and carries its forecast keys (passed through via
    ``extra="allow"``). Served with ``response_model_exclude_unset=True`` so the
    two branches keep their distinct shapes.
    """

    error: str | None = None
    rows: list[Any] | None = None
    warnings: list[Any] | None = None
