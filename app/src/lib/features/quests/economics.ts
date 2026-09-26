/**
 * Quest economics: the pure derivations behind the quests analytics view.
 * No runes; every function is a plain input-to-output mapping so the
 * accounting invariants (liquid TT and non-liquid PES never blend) stay
 * pinned by the colocated tests.
 *
 * The view reports rewards per recorded completion only. Per-quest cost is
 * not derived here: a session's cost cannot be charged to each quest that
 * ran in it without counting it once per co-active quest.
 */

import type { MarketHarvestData, MarketHarvestItem } from '$lib/api/commands.gen';
import { projectRewardItems } from '$lib/features/analytics/huntingModel.svelte';
import type { QuestAnalyticsRow } from '$lib/types';

export type RewardMode = 'tt' | 'markup';

export interface QuestAnalyticsComputed {
	questId: string;
	questName: string;
	planet: string;
	category: string | null;
	linkedSessions: number;
	recordedCompletions: number;
	confirmedCompletions: number;
	unresolvedCompletions: number;
	totalRecordedRewardTt: number;
	totalRecordedRewardMu: number;
	totalRealisedRewardMarkup: number;
	totalRecordedRewardPes: number;
	// Confirmed liquid-TT outcome per completion. TT mode uses observed face
	// value; Markup mode projects only stock reward items.
	displayLiquidReward: number;
	// PES face value of the reward per completion, invariant to the toggle.
	// 0 for liquid-TT outcomes.
	avgRewardPes: number;
	rewardMarkupPercent: number | null;
}

export function computeQuestAnalytics(
	rows: QuestAnalyticsRow[],
	rewardMode: RewardMode,
	market: MarketHarvestData | null = null,
): QuestAnalyticsComputed[] {
	return rows.map((row) => {
		const recordedRuns = row.recordedCompletions || 1;
		// Liquid TT: face value or with stock-item markup projection, depending
		// on the toggle. PES outcomes never contribute to the liquid side.
		const avgRewardLiquidFace = row.totalRecordedRewardTt / recordedRuns;
		const marketByItem = new Map<string, MarketHarvestItem>(
			market?.items.map((item) => [item.itemName, item]) ?? [],
		);
		const projectedItems =
			projectRewardItems(row.recordedRewardItems, market, marketByItem, 'liquidMiddling') ?? 0;
		const totalRecordedRewardMu =
			row.totalRecordedRewardTt - row.totalRecordedItemTt + projectedItems;
		const avgRewardLiquidMarkup = totalRecordedRewardMu / recordedRuns;
		const displayLiquidReward =
			rewardMode === 'markup' ? avgRewardLiquidMarkup : avgRewardLiquidFace;
		// PES reward stays at face value across both modes.
		const avgRewardPes = row.totalRecordedRewardPes / recordedRuns;
		return {
			questId: row.questId,
			questName: row.questName,
			planet: row.planet,
			category: row.category,
			linkedSessions: row.linkedSessions,
			recordedCompletions: row.recordedCompletions,
			confirmedCompletions: row.confirmedCompletions,
			unresolvedCompletions: row.unresolvedCompletions,
			totalRecordedRewardTt: row.totalRecordedRewardTt,
			totalRecordedRewardMu,
			totalRealisedRewardMarkup: row.totalRealisedRewardMarkup,
			totalRecordedRewardPes: row.totalRecordedRewardPes,
			displayLiquidReward,
			avgRewardPes,
			rewardMarkupPercent:
				row.totalRecordedRewardTt > 0
					? (totalRecordedRewardMu / row.totalRecordedRewardTt) * 100
					: null,
		};
	});
}
