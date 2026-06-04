import type { ISODate, Ped } from './common';

export type PlaylistItemGroup = 'immediate' | 'long_horizon';

export interface Quest {
	id: string;
	name: string;
	category: string | null;
	targetMobs: string[];
	planet: string;
	waypoint: string | null;
	cooldownDurationHours: number | null;
	cooldownExpiresAt: ISODate | null;
	reward: Ped | null;
	rewardIsSkill: boolean;
	expectedRewardMarkupPercent: number | null;
	rewardDescription: string;
	notes: string;
	chainName: string | null;
	chainPosition: number | null;
	chainTotal: number | null;
	playlistIds: string[];
	startedAt: number | null; // Unix timestamp
}

/** A quest slot in a playlist, with optional routing note */
export interface PlaylistItem {
	questId: string;
	description: string | null;
	groupType: PlaylistItemGroup;
}

export interface QuestPlaylist {
	id: string;
	name: string;
	planet: string;
	estimatedMinutes: number;
	questIds: string[];
	immediateQuestIds: string[];
	longHorizonQuestIds: string[];
	items: PlaylistItem[];
}

/** Data for creating a quest */
export interface QuestCreateData {
	name: string;
	planet?: string;
	category?: string | null;
	waypoint?: string | null;
	cooldown_hours?: number | null;
	reward_ped?: number | null;
	reward_is_skill?: boolean;
	expected_reward_markup_percent?: number | null;
	reward_description?: string | null;
	notes?: string | null;
	chain_name?: string | null;
	chain_position?: number | null;
	chain_total?: number | null;
	mobs?: string[];
}

/** Data for updating a quest */
export interface QuestUpdateData {
	name?: string;
	planet?: string;
	category?: string | null;
	waypoint?: string | null;
	cooldown_hours?: number | null;
	reward_ped?: number | null;
	reward_is_skill?: boolean;
	expected_reward_markup_percent?: number | null;
	reward_description?: string | null;
	notes?: string | null;
	chain_name?: string | null;
	chain_position?: number | null;
	chain_total?: number | null;
	mobs?: string[];
}

/** Data for creating a playlist */
export interface PlaylistCreateData {
	name: string;
	planet?: string;
	estimated_minutes?: number;
	quest_ids?: number[];
	items?: { quest_id: number; description?: string | null; group_type?: PlaylistItemGroup }[];
}

/** Data for updating a playlist */
export interface PlaylistUpdateData {
	name?: string;
	planet?: string;
	estimated_minutes?: number;
	quest_ids?: number[];
	items?: { quest_id: number; description?: string | null; group_type?: PlaylistItemGroup }[];
}

/** Raw per-quest analytics returned from the backend */
export interface QuestAnalyticsRow {
	questId: string;
	questName: string;
	planet: string;
	category: string | null;
	rewardPed: number;
	rewardIsSkill: boolean;
	expectedRewardMarkupPercent: number | null;
	totalExpectedRewardPed: number;
	linkedSessions: number;
	totalDurationSec: number;
	totalWeaponCost: number;
	totalHealCost: number;
	totalEnhancerCost: number;
	totalArmourCost: number;
	totalLootTt: number;
	totalPes: number;
}

/** Raw per-playlist analytics from exact-match sessions */
export interface PlaylistAnalyticsRow {
	playlistId: string;
	playlistName: string;
	questCount: number;
	longHorizonQuestCount: number;
	matchedSessions: number;
	totalRewardPed: number;
	totalImmediateRewardPed: number;
	totalBonusRewardPed: number;
	totalPesReward: number;
	totalImmediatePesReward: number;
	totalBonusPesReward: number;
	totalExpectedRewardPed: number;
	totalExpectedImmediateRewardPed: number;
	totalExpectedBonusRewardPed: number;
	totalDurationSec: number;
	totalWeaponCost: number;
	totalHealCost: number;
	totalEnhancerCost: number;
	totalArmourCost: number;
	totalLootTt: number;
	totalPes: number;
}
