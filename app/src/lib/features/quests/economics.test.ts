import { describe, expect, it } from 'vitest';
import type { QuestAnalyticsRow } from '$lib/types';
import { computeQuestAnalytics } from './economics';

function questRow(overrides: Partial<QuestAnalyticsRow> = {}): QuestAnalyticsRow {
	return {
		questId: 'q1',
		questName: 'Daily Kill',
		planet: 'Calypso',
		category: null,
		recordedCompletions: 2,
		confirmedCompletions: 2,
		unresolvedCompletions: 0,
		totalRecordedRewardTt: 4,
		totalRecordedRewardPes: 0,
		totalRecordedItemTt: 0,
		totalRealisedRewardMarkup: 0,
		recordedRewardItems: [],
		linkedSessions: 2,
		...overrides,
	};
}

describe('computeQuestAnalytics: confirmed liquid-TT outcomes', () => {
	// Fixture: 4 PED observed TT over 2 completions (2 per run).
	it('shows the face-value reward per run in TT mode', () => {
		const [row] = computeQuestAnalytics([questRow()], 'tt');
		expect(row.displayLiquidReward).toBeCloseTo(2, 12);
	});

	it('does not revalue liquid TT without observed stock items', () => {
		const [row] = computeQuestAnalytics([questRow()], 'markup');
		expect(row.displayLiquidReward).toBeCloseTo(2, 12);
		expect(row.rewardMarkupPercent).toBe(100);
	});

	it('counts Universal Ammo once as liquid face value and never as market stock', () => {
		const [row] = computeQuestAnalytics(
			[
				questRow({
					totalRecordedRewardTt: 4,
					totalRecordedItemTt: 0,
					recordedRewardItems: [{ itemName: 'Universal Ammo', quantity: 40000, valuePed: 4 }],
				}),
			],
			'markup',
		);
		expect(row.totalRecordedRewardMu).toBe(4);
		expect(row.displayLiquidReward).toBe(2);
	});

	it('projects only the stock component of a mixed ammo and item reward', () => {
		const [row] = computeQuestAnalytics(
			[
				questRow({
					totalRecordedRewardTt: 5,
					totalRecordedItemTt: 1,
					recordedRewardItems: [
						{ itemName: 'Universal Ammo', quantity: 40000, valuePed: 4 },
						{ itemName: 'Mission Token', quantity: 1, valuePed: 1 },
					],
				}),
			],
			'markup',
			{
				nanocubeMarkupPct: null,
				items: [
					{
						itemName: 'Mission Token',
						markupPct: 200,
						unitPricePed: null,
						horizon: 'month',
						salesPed: 100,
						recommendedPacketTt: null,
						readings: [],
					},
				],
			},
		);
		expect(row.totalRecordedRewardMu).toBe(6);
		expect(row.displayLiquidReward).toBe(3);
	});

	it('keeps the PES column at zero for a liquid-TT outcome in both modes', () => {
		for (const mode of ['tt', 'markup'] as const) {
			const [row] = computeQuestAnalytics([questRow()], mode);
			expect(row.avgRewardPes).toBe(0);
		}
	});

	it('projects twenty zero-TT vouchers to forty PED from a two-PED unit quote', () => {
		const [row] = computeQuestAnalytics(
			[
				questRow({
					recordedCompletions: 20,
					confirmedCompletions: 20,
					totalRecordedRewardTt: 0,
					totalRecordedItemTt: 0,
					recordedRewardItems: [{ itemName: 'Hyperion Daily Voucher', quantity: 20, valuePed: 0 }],
				}),
			],
			'markup',
			{
				nanocubeMarkupPct: null,
				items: [
					{
						itemName: 'Hyperion Daily Voucher',
						markupPct: null,
						unitPricePed: 2,
						horizon: null,
						salesPed: null,
						recommendedPacketTt: null,
						readings: [],
					},
				],
			},
		);
		expect(row.totalRecordedRewardTt).toBe(0);
		expect(row.totalRecordedRewardMu).toBe(40);
		expect(row.displayLiquidReward).toBe(2);
	});
});

describe('computeQuestAnalytics: skill quests never blend into liquid', () => {
	const skillQuest = questRow({
		recordedCompletions: 4,
		confirmedCompletions: 4,
		totalRecordedRewardTt: 0,
		totalRecordedItemTt: 0,
		recordedRewardItems: [],
		totalRecordedRewardPes: 20,
		linkedSessions: 4,
	});

	it('contributes zero liquid reward in BOTH display modes', () => {
		for (const mode of ['tt', 'markup'] as const) {
			const [row] = computeQuestAnalytics([skillQuest], mode);
			expect(row.displayLiquidReward).toBe(0);
		}
	});

	it('rides the PES side at face value per run, invariant to the toggle', () => {
		for (const mode of ['tt', 'markup'] as const) {
			const [row] = computeQuestAnalytics([skillQuest], mode);
			expect(row.avgRewardPes).toBe(5);
		}
	});
});

describe('computeQuestAnalytics: division guards', () => {
	it('falls back to one run when nothing was recorded', () => {
		const [row] = computeQuestAnalytics(
			[
				questRow({
					recordedCompletions: 0,
					confirmedCompletions: 0,
					totalRecordedRewardTt: 0,
					totalRecordedRewardPes: 0,
					totalRecordedItemTt: 0,
					recordedRewardItems: [],
				}),
			],
			'tt',
		);
		expect(row.displayLiquidReward).toBe(0);
		expect(row.avgRewardPes).toBe(0);
		expect(row.rewardMarkupPercent).toBeNull();
	});
});
