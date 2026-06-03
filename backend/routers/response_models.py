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
# Notable events (shared by the snapshot's recentEvents feed and warnings)
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
# Tracking snapshot (/tracking/snapshot): unavailable | idle | active
# ---------------------------------------------------------------------------


class TrackingSnapshot(_Loose):
    """The consolidated dashboard hydration shape.

    The union of the status, live, and recent-events readouts in one response:
    a newly mounted dashboard hydrates from this once, then reacts to pushed
    events rather than re-polling. Casing is preserved from the readouts it
    unions: ``session_id`` / ``started_at`` / ``kill_count`` stay snake-case,
    the headline numbers stay camelCase. Polymorphic across the three states;
    served with ``response_model_exclude_unset=True`` so each state keeps its
    own shape.
    """

    status: str
    # Shared by idle + active.
    hotbarListenerActive: bool | None = None
    weaponAttribution: str | None = None
    repairOcrEnabled: bool | None = None
    endOfSessionArmourReminderEnabled: bool | None = None
    mobEntryMode: str | None = None
    currentMob: str | None = None
    mobSource: str | None = None
    currentTool: str | None = None
    trifectaAttribution: dict[str, Any] | None = None
    recentEvents: list[NotableEvent] | None = None
    # Active only.
    session_id: str | None = None
    started_at: str | None = None
    kill_count: int | None = None
    elapsed: int | None = None
    cost: float | None = None
    returns: float | None = None
    pes: float | None = None
    net: float | None = None
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
    warnings: list[NotableEvent] | None = None


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


# ---------------------------------------------------------------------------
# Shared acknowledgement shapes
# ---------------------------------------------------------------------------


class OkResponse(_Loose):
    """A bare ``{"ok": true}`` mutation acknowledgement."""

    ok: bool


class DeletedStatus(_Loose):
    """A ``{"status": "deleted"}`` soft-delete acknowledgement."""

    status: str


# ---------------------------------------------------------------------------
# Quests + playlists (/quests, /quests/playlists)
# ---------------------------------------------------------------------------


class Quest(_Loose):
    """A quest as returned by the quest read/write endpoints.

    Mirrors the frontend ``Quest`` type field-for-field, camelCase verbatim.
    Every key is always emitted by ``_format_quest`` (nullables carry explicit
    ``null``), so the shape is stable and served without an exclude flag.
    """

    id: str
    name: str
    category: str | None = None
    targetMobs: list[str]
    planet: str
    waypoint: str | None = None
    cooldownDurationHours: float | None = None
    cooldownExpiresAt: str | None = None
    reward: float | None = None
    rewardIsSkill: bool
    expectedRewardMarkupPercent: float | None = None
    rewardDescription: str
    notes: str
    chainName: str | None = None
    chainPosition: int | None = None
    chainTotal: int | None = None
    playlistIds: list[str]
    startedAt: float | None = None  # unix timestamp (time.time()), fractional


class QuestAnalyticsRow(_Loose):
    """Per-quest sustainability metrics from curated linked sessions."""

    questId: str
    questName: str
    planet: str
    category: str | None = None
    rewardPed: float
    rewardIsSkill: bool
    expectedRewardMarkupPercent: float | None = None
    totalExpectedRewardPed: float
    linkedSessions: int
    totalDurationSec: float
    totalWeaponCost: float
    totalHealCost: float
    totalEnhancerCost: float
    totalArmourCost: float
    totalLootTt: float
    totalPes: float


class PlaylistItem(_Loose):
    """A quest slot within a playlist."""

    questId: str
    description: str | None = None
    groupType: str


class QuestPlaylist(_Loose):
    """A playlist as returned by the playlist read/write endpoints."""

    id: str
    name: str
    planet: str
    estimatedMinutes: int
    questIds: list[str]
    immediateQuestIds: list[str]
    longHorizonQuestIds: list[str]
    items: list[PlaylistItem]


class PlaylistAnalyticsRow(_Loose):
    """Per-playlist sustainability metrics from exact-match sessions."""

    playlistId: str
    playlistName: str
    questCount: int
    longHorizonQuestCount: int
    matchedSessions: int
    totalRewardPed: float
    totalImmediateRewardPed: float
    totalBonusRewardPed: float
    totalPesReward: float
    totalImmediatePesReward: float
    totalBonusPesReward: float
    totalExpectedRewardPed: float
    totalExpectedImmediateRewardPed: float
    totalExpectedBonusRewardPed: float
    totalDurationSec: float
    totalWeaponCost: float
    totalHealCost: float
    totalEnhancerCost: float
    totalArmourCost: float
    totalLootTt: float
    totalPes: float


# ---------------------------------------------------------------------------
# Codex (/codex)
# ---------------------------------------------------------------------------


class CodexSpecies(_Loose):
    """A mob species row with codex base cost and player rank progress."""

    name: str
    baseCost: float
    codexType: str | None = None
    currentRank: int
    nextRank: int | None = None
    nextCategory: str | None = None
    nextCost: float | None = None


class CodexRankItem(_Loose):
    """One of a species' 25 codex ranks, cross-referenced with claims."""

    rank: int
    category: str
    cost: float
    rewardPed: float
    cat4Bonus: bool
    cat4RewardPed: float | None = None
    skills: list[str]
    cat4Skills: list[str]
    claimed: bool
    claimedSkill: str | None = None
    claimedPed: float | None = None
    isNext: bool


class CodexRankBreakdown(_Loose):
    """A species' full 25-rank breakdown, cross-referenced with claims."""

    speciesName: str
    baseCost: float
    codexType: str | None = None
    currentRank: int
    ranks: list[CodexRankItem]


class CodexClaimResult(_Loose):
    """Acknowledgement of a claimed codex rank reward."""

    speciesName: str
    rank: int
    skillName: str
    pedValue: float


class CodexCalibrateResult(_Loose):
    """Acknowledgement of a direct codex-rank calibration."""

    speciesName: str
    rank: int


class CodexSkillOption(_Loose):
    """A skill choice for a codex rank, ranked by profession or HP gain."""

    skillName: str
    category: str
    rewardPed: float
    currentLevel: float | None = None
    levelsGained: float
    professionWeight: int
    profContribution: float
    hpIncrease: float | None = None
    hpGain: float
    recommendRank: int | None = None


class CodexMetaAttribute(_Loose):
    """A meta-codex attribute with its current calibrated level."""

    name: str
    currentLevel: float | None = None


class CodexMetaClaimResult(_Loose):
    """Acknowledgement of a claimed meta-codex attribute reward."""

    attributeName: str
    pedValue: float


# ---------------------------------------------------------------------------
# Equipment (/equipment)
# ---------------------------------------------------------------------------


class EquipmentSearchResult(_Loose):
    """A catalogue search hit, with ammo burn already converted to PEC."""

    catalogId: str
    name: str
    decay: float
    ammoBurn: float
    isLimited: bool


class Equipment(_Loose):
    """A library entry with computed per-use cost (the list shape)."""

    id: str
    name: str
    type: str
    amplifierName: str | None = None
    costPerUse: float
    damageMin: float | None = None
    damageMax: float | None = None
    reloadSeconds: float | None = None
    isLimited: bool
    enrichmentLevel: int


class CostBreakdownLine(_Loose):
    """One line of a per-use cost breakdown."""

    component: str
    costPec: float
    markupMultiplier: float
    effectiveCostPec: float


class WeaponComponent(_Loose):
    """A weapon, amplifier, or scope sub-object of an equipment detail.

    The amplifier and scope sub-objects are emitted by the same builder as the
    weapon, so they carry ``damageEnhancers`` too (the frontend type omits it on
    the amplifier; the handler emits it, so it is modelled here and kept).
    """

    catalogId: str | None = None
    name: str
    decay: float
    ammoBurn: float
    markupPercent: int
    isLimited: bool
    damageEnhancers: int


class AbsorberComponent(_Loose):
    """The absorber sub-object of an equipment detail (carries absorption)."""

    catalogId: str | None = None
    name: str
    decay: float
    ammoBurn: float
    absorptionPercent: float
    markupPercent: int
    isLimited: bool


class EquipmentDetail(_Loose):
    """The expanded detail for a library item, with cost breakdown."""

    id: str
    weapon: WeaponComponent
    amplifier: WeaponComponent | None = None
    scope: WeaponComponent | None = None
    absorber: AbsorberComponent | None = None
    costBreakdown: list[CostBreakdownLine]
    totalCostPerUse: float


class CostResult(_Loose):
    """A standalone cost calculation (breakdown plus total)."""

    costBreakdown: list[CostBreakdownLine]
    totalCostPerUse: float


# ---------------------------------------------------------------------------
# Settings (/settings)
# ---------------------------------------------------------------------------


class GameConnection(_Loose):
    """The chat.log connection block of the settings response."""

    chatLogPath: str
    chatLogValid: bool
    playerName: str


class TrifectaPreset(_Loose):
    """One trifecta loadout preset with its readiness state."""

    id: str
    name: str
    smallWeaponId: int | None = None
    bigWeaponId: int | None = None
    healId: int | None = None
    ready: bool
    message: str | None = None


class TrifectaSettings(_Loose):
    """The trifecta block of the settings response."""

    activePresetId: str | None = None
    activePresetName: str | None = None
    presets: list[TrifectaPreset]
    ready: bool
    message: str | None = None


class AppSettings(_Loose):
    """The full application settings response."""

    gameConnection: GameConnection
    hotbarHooksEnabled: bool
    repairOcrEnabled: bool
    endOfSessionArmourReminderEnabled: bool
    developerModeEnabled: bool
    mobTrackingMode: str
    mobTrackingTag: str
    hotbar: dict[str, int | None]
    trifecta: TrifectaSettings
    lootFilterBlacklist: list[str]
    dbPath: str
    appVersion: str


class OverlayPosition(_Loose):
    """The persisted overlay window position (null until first placed)."""

    x: int | None = None
    y: int | None = None


# ---------------------------------------------------------------------------
# Recording (developer-only) (/recording)
# ---------------------------------------------------------------------------


class RecordingStatus(_Loose):
    """Recording lifecycle state plus live capture counters."""

    state: str
    started_at: str | None = None
    lines: int
    captures: int
    keystrokes: int


class RecordingStopResult(_Loose):
    """The outcome of finalising a recording.

    Polymorphic: the success branch carries ``finalized_path`` + ``determinism``
    (and ``diff`` only on a determinism leak); the failure branch carries
    ``error`` + ``recovery_path``. Served with ``response_model_exclude_unset=True``
    so each branch keeps its own keys rather than gaining a wall of nulls.
    """

    finalized_path: str | None = None
    determinism: str | None = None
    diff: str | None = None
    error: str | None = None
    recovery_path: str | None = None


class RecordingAbortResult(_Loose):
    """Acknowledgement that the in-flight recording was discarded."""

    state: str
