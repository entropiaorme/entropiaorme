"""Quest endpoints — CRUD, cooldowns, playlists, completion."""

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from backend.dependencies import get_services

router = APIRouter(prefix="/quests", tags=["quests"])


# ── Request models ──────────────────────────────────────────────────────────


class QuestCreate(BaseModel):
    name: str
    planet: str = "Calypso"
    category: str | None = None
    waypoint: str | None = None
    cooldown_hours: float | None = None
    reward_ped: float | None = None
    reward_is_skill: bool = False
    expected_reward_markup_percent: float | None = None
    reward_description: str | None = None
    notes: str | None = None
    chain_name: str | None = None
    chain_position: int | None = None
    chain_total: int | None = None
    mobs: list[str] = []


class QuestUpdate(BaseModel):
    name: str | None = None
    planet: str | None = None
    category: str | None = None
    waypoint: str | None = None
    cooldown_hours: float | None = None
    reward_ped: float | None = None
    reward_is_skill: bool | None = None
    expected_reward_markup_percent: float | None = None
    reward_description: str | None = None
    notes: str | None = None
    chain_name: str | None = None
    chain_position: int | None = None
    chain_total: int | None = None
    mobs: list[str] | None = None


class PlaylistItemInput(BaseModel):
    quest_id: int
    description: str | None = None
    group_type: str = "immediate"


class PlaylistCreate(BaseModel):
    name: str
    planet: str = "Calypso"
    estimated_minutes: int = 30
    quest_ids: list[int] = []
    items: list[PlaylistItemInput] | None = None


class PlaylistUpdate(BaseModel):
    name: str | None = None
    planet: str | None = None
    estimated_minutes: int | None = None
    quest_ids: list[int] | None = None
    items: list[PlaylistItemInput] | None = None


class QuestCancel(BaseModel):
    undo_reward: bool = False


# ── Helpers ─────────────────────────────────────────────────────────────────


def _format_quest(q: dict) -> dict:
    """Format a quest dict for the API response, matching frontend Quest type."""
    return {
        "id": str(q["id"]),
        "name": q["name"],
        "category": q.get("category"),
        "targetMobs": q.get("mobs", []),
        "planet": q["planet"],
        "waypoint": q.get("waypoint"),
        "cooldownDurationHours": q.get("cooldown_hours"),
        "cooldownExpiresAt": q.get("cooldown_expires_at"),
        "reward": q.get("reward_ped"),
        "rewardIsSkill": bool(q.get("reward_is_skill", 0)),
        "expectedRewardMarkupPercent": q.get("expected_reward_markup_percent"),
        "rewardDescription": q.get("reward_description") or "",
        "notes": q.get("notes") or "",
        "chainName": q.get("chain_name"),
        "chainPosition": q.get("chain_position"),
        "chainTotal": q.get("chain_total"),
        "playlistIds": [str(pid) for pid in q.get("playlist_ids", [])],
        "startedAt": q.get("started_at"),
    }


def _format_playlist(pl: dict) -> dict:
    """Format a playlist dict for the API response, matching frontend QuestPlaylist type."""
    items = pl.get("items", [])
    return {
        "id": str(pl["id"]),
        "name": pl["name"],
        "planet": pl["planet"],
        "estimatedMinutes": pl["estimated_minutes"],
        "questIds": [str(qid) for qid in pl.get("quest_ids", [])],
        "immediateQuestIds": [str(qid) for qid in pl.get("immediate_quest_ids", [])],
        "longHorizonQuestIds": [
            str(qid) for qid in pl.get("long_horizon_quest_ids", [])
        ],
        "items": [
            {
                "questId": str(i["quest_id"]),
                "description": i.get("description"),
                "groupType": i.get("group_type", "immediate"),
            }
            for i in items
        ],
    }


# ── Static quest endpoints (must be before /{quest_id} parameter routes) ───


@router.get("")
def list_quests():
    """List all active quests."""
    svc = get_services()
    quests = svc.quest_service.get_quests()
    return [_format_quest(q) for q in quests]


@router.post("")
def create_quest(req: QuestCreate):
    """Create a new quest."""
    svc = get_services()
    q = svc.quest_service.create_quest(req.model_dump())
    return _format_quest(q)


@router.get("/mobs")
def list_mob_names():
    """All distinct mob names for autocomplete."""
    svc = get_services()
    return svc.quest_service.get_all_mob_names()


@router.get("/analytics")
def quest_analytics():
    """Per-quest sustainability metrics from curated linked sessions."""
    svc = get_services()
    rows = svc.quest_service.get_quest_analytics()
    return [_format_quest_analytics(r) for r in rows]


def _format_quest_analytics(row: dict) -> dict:
    """Format quest analytics row for the API response."""
    return {
        "questId": str(row["quest_id"]),
        "questName": row["quest_name"],
        "planet": row["planet"],
        "category": row["category"],
        "rewardPed": round(row["reward_ped"], 2),
        "rewardIsSkill": row["reward_is_skill"],
        "expectedRewardMarkupPercent": row["expected_reward_markup_percent"],
        "totalExpectedRewardPed": round(row["total_expected_reward_ped"], 2),
        "linkedSessions": row["linked_sessions"],
        "totalDurationSec": round(row["total_duration"], 1),
        "totalWeaponCost": round(row["weapon_cost"], 4),
        "totalHealCost": round(row["heal_cost"], 4),
        "totalEnhancerCost": round(row["enhancer_cost"], 4),
        "totalArmourCost": round(row["armour_cost"], 4),
        "totalLootTt": round(row["loot_tt"], 4),
        "totalPes": round(row["skill_tt"], 4),
    }


# ── Playlist endpoints (must be before /{quest_id} to avoid route clash) ───


@router.get("/playlists")
def list_playlists():
    """List all active playlists."""
    svc = get_services()
    playlists = svc.quest_service.get_playlists()
    return [_format_playlist(pl) for pl in playlists]


@router.get("/playlists/analytics")
def playlist_analytics():
    """Per-playlist sustainability metrics from curated linked sessions."""
    svc = get_services()
    rows = svc.quest_service.get_all_playlist_analytics()
    return [_format_playlist_analytics(r) for r in rows]


def _format_playlist_analytics(row: dict) -> dict:
    """Format playlist analytics row for the API response."""
    return {
        "playlistId": str(row["playlist_id"]),
        "playlistName": row["playlist_name"],
        "questCount": row["quest_count"],
        "longHorizonQuestCount": row["long_horizon_quest_count"],
        "matchedSessions": row["matched_sessions"],
        "totalRewardPed": round(row["total_reward_ped"], 2),
        "totalImmediateRewardPed": round(row["total_immediate_reward_ped"], 2),
        "totalBonusRewardPed": round(row["total_bonus_reward_ped"], 2),
        "totalPesReward": round(row["total_skill_reward_ped"], 2),
        "totalImmediatePesReward": round(row["total_immediate_skill_reward_ped"], 2),
        "totalBonusPesReward": round(row["total_bonus_skill_reward_ped"], 2),
        "totalExpectedRewardPed": round(row["total_expected_reward_ped"], 2),
        "totalExpectedImmediateRewardPed": round(
            row["total_expected_immediate_reward_ped"], 2
        ),
        "totalExpectedBonusRewardPed": round(row["total_expected_bonus_reward_ped"], 2),
        "totalDurationSec": round(row["total_duration"], 1),
        "totalWeaponCost": round(row["weapon_cost"], 4),
        "totalHealCost": round(row["heal_cost"], 4),
        "totalEnhancerCost": round(row["enhancer_cost"], 4),
        "totalArmourCost": round(row["armour_cost"], 4),
        "totalLootTt": round(row["loot_tt"], 4),
        "totalPes": round(row["skill_tt"], 4),
    }


@router.post("/playlists")
def create_playlist(req: PlaylistCreate):
    """Create a new playlist."""
    svc = get_services()
    pl = svc.quest_service.create_playlist(req.model_dump())
    return _format_playlist(pl)


@router.put("/playlists/{playlist_id}")
def update_playlist(playlist_id: int, req: PlaylistUpdate):
    """Update a playlist."""
    svc = get_services()
    data = req.model_dump(exclude_unset=True)
    pl = svc.quest_service.update_playlist(playlist_id, data)
    if not pl:
        raise HTTPException(status_code=404, detail="Playlist not found")
    return _format_playlist(pl)


@router.delete("/playlists/{playlist_id}")
def delete_playlist(playlist_id: int):
    """Soft-delete a playlist."""
    svc = get_services()
    if not svc.quest_service.delete_playlist(playlist_id):
        raise HTTPException(status_code=404, detail="Playlist not found")
    return {"ok": True}


# ── Parameterised quest endpoints ──────────────────────────────────────────


@router.get("/{quest_id}")
def get_quest(quest_id: int):
    """Get a single quest."""
    svc = get_services()
    q = svc.quest_service.get_quest(quest_id)
    if not q:
        raise HTTPException(status_code=404, detail="Quest not found")
    return _format_quest(q)


@router.put("/{quest_id}")
def update_quest(quest_id: int, req: QuestUpdate):
    """Update a quest."""
    svc = get_services()
    data = req.model_dump(exclude_unset=True)
    q = svc.quest_service.update_quest(quest_id, data)
    if not q:
        raise HTTPException(status_code=404, detail="Quest not found")
    return _format_quest(q)


@router.delete("/{quest_id}")
def delete_quest(quest_id: int):
    """Soft-delete a quest."""
    svc = get_services()
    if not svc.quest_service.delete_quest(quest_id):
        raise HTTPException(status_code=404, detail="Quest not found")
    return {"ok": True}


@router.post("/{quest_id}/start")
def start_quest(quest_id: int):
    """Mark a quest as in-progress."""
    svc = get_services()
    q = svc.quest_service.start_quest(quest_id)
    if not q:
        raise HTTPException(status_code=404, detail="Quest not found")
    return _format_quest(q)


@router.post("/{quest_id}/complete")
def complete_quest(quest_id: int):
    """Complete a quest — resets cooldown, increments counter, auto-creates ledger entry."""
    svc = get_services()
    q = svc.quest_service.complete_quest(quest_id)
    if not q:
        raise HTTPException(status_code=404, detail="Quest not found")
    return _format_quest(q)


@router.post("/{quest_id}/cancel")
def cancel_quest(quest_id: int, req: QuestCancel | None = None):
    """Cancel a started quest or undo an active cooldown reset."""
    svc = get_services()
    q = svc.quest_service.cancel_quest(
        quest_id,
        undo_reward=bool(req.undo_reward) if req else False,
    )
    if not q:
        raise HTTPException(status_code=404, detail="Quest not found")
    return _format_quest(q)
