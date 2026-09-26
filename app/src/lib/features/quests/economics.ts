/**
 * Quest economics: the pure derivations behind the quests
 * analytics view. No runes; every function is a plain input-to-output
 * mapping so the accounting invariants (liquid TT and non-liquid PES
 * never blend) stay pinned by the colocated tests.
 */

import type {
	AnalyticsOverview,
	MarketHarvestData,
	MarketHarvestItem,
} from '$lib/api/commands.gen';
import { projectRewardItems } from '$lib/features/analytics/huntingModel.svelte';
import type { QuestAnalyticsRow } from '$lib/types';

export type RewardMode = 'tt' | 'markup';

export interface GlobalRates {
	/**
	 * Liquid PED returns per cycled PED: drives Net/Rate forecasts and the
	 * "PES burn-saved" translation. Skill TT, codex PES, and quest PES are
	 * progression and stay out of this rate.
	 */
	liquidReturnRate: number;
	/**
	 * Combined PES throughput per cycled PED (skill TT + codex PES + quest PES).
	 * Used to translate a PES reward into the equivalent cycle it would replace.
	 */
	skillProgressionRate: number;
}

/** Derive the global baseline rates from the all-time analytics overview. */
export function globalRates(
	overview: Pick<AnalyticsOverview, 'returnsBreakdown' | 'lossesBreakdown'>,
): GlobalRates {
	// Liquid PED returns: ordinary loot, confirmed quest-item TT, and every
	// realised ledger gain (including Universal Ammo and realised sale MU).
	const ledgerGains = Object.values(overview.returnsBreakdown.ledger).reduce(
		(sum, value) => sum + value,
		0,
	);
	const liquidReturns =
		overview.returnsBreakdown.lootTt + overview.returnsBreakdown.questItemTt + ledgerGains;
	// PES throughput across all progression channels.
	const skillProgressionReturns =
		overview.returnsBreakdown.pes +
		(overview.returnsBreakdown.codexPes ?? 0) +
		(overview.returnsBreakdown.questPes ?? 0);
	const rawCycled = overview.lossesBreakdown.trackingCost;
	return {
		liquidReturnRate: rawCycled > 0 ? liquidReturns / rawCycled : 0,
		skillProgressionRate: rawCycled > 0 ? skillProgressionReturns / rawCycled : 0,
	};
}

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
	totalCycled: number;
	avgRawReturns: number;
	avgCycled: number;
	// Confirmed liquid-TT outcome shown in the Reward column. TT mode uses
	// observed face value; Markup mode projects only stock reward items.
	displayLiquidReward: number;
	// PES face value of the reward, invariant to toggle. 0 for liquid-TT outcomes.
	avgRewardPes: number;
	rewardMarkupPercent: number | null;
	// Liquid Net for the run: liquid cycle returns + liquid reward − cycled.
	avgNet: number;
	// Cycle PES baseline + explicit PES reward. Always at face value.
	avgPesNet: number;
	returnRate: number;
}

export function computeQuestAnalytics(
	rows: QuestAnalyticsRow[],
	rates: GlobalRates,
	rewardMode: RewardMode,
	market: MarketHarvestData | null = null,
): QuestAnalyticsComputed[] {
	return rows.map((row) => {
		const totalCycled =
			row.totalWeaponCost +
			row.totalHealCost +
			row.totalConsumableCost +
			row.totalEnhancerCost +
			row.totalArmourCost;
		const sessions = row.linkedSessions || 1;
		const recordedRuns = row.recordedCompletions || 1;
		const avgCycled = totalCycled / sessions;
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
		// Liquid cycle projection (PES sources excluded: denomination-pure).
		const avgRawReturns = avgCycled * rates.liquidReturnRate;
		const avgNet = avgRawReturns + displayLiquidReward - avgCycled;
		const returnRate = avgCycled > 0 ? (avgRawReturns + displayLiquidReward) / avgCycled : 0;
		// PES cycle baseline + explicit PES reward (face value).
		const avgPesNet = avgCycled * rates.skillProgressionRate + avgRewardPes;
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
			totalCycled,
			displayLiquidReward,
			avgRewardPes,
			rewardMarkupPercent:
				row.totalRecordedRewardTt > 0
					? (totalRecordedRewardMu / row.totalRecordedRewardTt) * 100
					: null,
			avgRawReturns,
			avgCycled,
			avgNet,
			avgPesNet,
			returnRate,
		};
	});
}
