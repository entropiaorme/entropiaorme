import { describe, expect, it } from 'vitest';
import type { QuestAnalyticsRow } from '$lib/types';
import { computeQuestAnalytics, type GlobalRates, globalRates } from './economics';

// 0.9 liquid PED and 0.05 PES back per cycled PED: round numbers so every
// expected value below is exact by hand.
const RATES: GlobalRates = { liquidReturnRate: 0.9, skillProgressionRate: 0.05 };

const emptyCycled = { weapon: 0, healing: 0, enhancer: 0, armour: 0, dangling: 0, consumables: 0 };

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
		totalDurationSec: 3600,
		totalWeaponCost: 60,
		totalHealCost: 20,
		totalConsumableCost: 0,
		totalEnhancerCost: 15,
		totalArmourCost: 5,
		totalLootTt: 90,
		totalPes: 4,
		...overrides,
	};
}

describe('globalRates', () => {
	it('derives the liquid rate from loot TT, quest-item TT, and ledger gains', () => {
		const rates = globalRates({
			returnsBreakdown: {
				lootTt: 75,
				questItemTt: 5,
				pes: 3,
				codexPes: 1,
				questPes: 2,
				ledger: { convert: 10 },
			},
			lossesBreakdown: { trackingCost: 100, cycledBreakdown: emptyCycled, ledger: {} },
		});
		// (80 + 10) / 100: PES channels are progression and must not leak in.
		expect(rates.liquidReturnRate).toBeCloseTo(0.9, 12);
		// (3 + 1 + 2) / 100: all three progression channels, none of the liquid.
		expect(rates.skillProgressionRate).toBeCloseTo(0.06, 12);
	});

	it('treats a missing ledger convert bucket as zero', () => {
		const rates = globalRates({
			returnsBreakdown: {
				lootTt: 50,
				questItemTt: 0,
				pes: 0,
				codexPes: 0,
				questPes: 0,
				ledger: {},
			},
			lossesBreakdown: { trackingCost: 100, cycledBreakdown: emptyCycled, ledger: {} },
		});
		expect(rates.liquidReturnRate).toBeCloseTo(0.5, 12);
	});

	it('returns zero rates when nothing has been cycled', () => {
		const rates = globalRates({
			returnsBreakdown: {
				lootTt: 80,
				questItemTt: 0,
				pes: 3,
				codexPes: 1,
				questPes: 2,
				ledger: { convert: 10 },
			},
			lossesBreakdown: { trackingCost: 0, cycledBreakdown: emptyCycled, ledger: {} },
		});
		expect(rates.liquidReturnRate).toBe(0);
		expect(rates.skillProgressionRate).toBe(0);
	});
});

describe('computeQuestAnalytics: confirmed liquid-TT outcomes', () => {
	// Fixture: 2 PED observed TT per completion, 2 sessions, 100 PED cycled
	// in total (so 50 per session).
	it('shows the face-value reward in TT mode', () => {
		const [row] = computeQuestAnalytics([questRow()], RATES, 'tt');
		expect(row.avgCycled).toBeCloseTo(50, 12);
		expect(row.displayLiquidReward).toBeCloseTo(2, 12);
		// 50 * 0.9 cycle returns + 2 reward - 50 cycled.
		expect(row.avgRawReturns).toBeCloseTo(45, 12);
		expect(row.avgNet).toBeCloseTo(-3, 12);
		expect(row.returnRate).toBeCloseTo(0.94, 12);
	});

	it('does not revalue liquid TT without observed stock items', () => {
		const [row] = computeQuestAnalytics([questRow()], RATES, 'markup');
		expect(row.displayLiquidReward).toBeCloseTo(2, 12);
		expect(row.avgNet).toBeCloseTo(-3, 12);
		expect(row.returnRate).toBeCloseTo(0.94, 12);
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
			RATES,
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
			RATES,
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
			const [row] = computeQuestAnalytics([questRow()], RATES, mode);
			expect(row.avgRewardPes).toBe(0);
			// PES net is purely the cycle baseline: 50 * 0.05.
			expect(row.avgPesNet).toBeCloseTo(2.5, 12);
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
			RATES,
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
		totalWeaponCost: 200,
		totalHealCost: 0,
		totalEnhancerCost: 0,
		totalArmourCost: 0,
	});

	it('contributes zero liquid reward in BOTH display modes', () => {
		for (const mode of ['tt', 'markup'] as const) {
			const [row] = computeQuestAnalytics([skillQuest], RATES, mode);
			expect(row.displayLiquidReward).toBe(0);
			// Liquid net sees only the cycle: 50 * 0.9 - 50.
			expect(row.avgNet).toBeCloseTo(-5, 12);
			expect(row.returnRate).toBeCloseTo(0.9, 12);
		}
	});

	it('rides the PES side at face value, invariant to the toggle', () => {
		for (const mode of ['tt', 'markup'] as const) {
			const [row] = computeQuestAnalytics([skillQuest], RATES, mode);
			expect(row.avgRewardPes).toBe(5);
			// 50 * 0.05 cycle PES + 5 reward PES.
			expect(row.avgPesNet).toBeCloseTo(7.5, 12);
		}
	});
});

describe('computeQuestAnalytics: division guards', () => {
	it('yields zero rates and zero returnRate when nothing was cycled', () => {
		const [row] = computeQuestAnalytics(
			[
				questRow({
					totalWeaponCost: 0,
					totalHealCost: 0,
					totalEnhancerCost: 0,
					totalArmourCost: 0,
				}),
			],
			RATES,
			'tt',
		);
		expect(row.avgCycled).toBe(0);
		expect(row.avgRawReturns).toBe(0);
		expect(row.returnRate).toBe(0);
		// Net degenerates to the bare reward.
		expect(row.avgNet).toBeCloseTo(2, 12);
	});

	it('falls back to one session when linkedSessions is zero', () => {
		const [row] = computeQuestAnalytics(
			[
				questRow({
					linkedSessions: 0,
					recordedCompletions: 0,
					confirmedCompletions: 0,
					totalRecordedRewardTt: 0,
					totalRecordedItemTt: 0,
					recordedRewardItems: [],
				}),
			],
			RATES,
			'tt',
		);
		// totalCycled / 1, not division by zero.
		expect(row.avgCycled).toBeCloseTo(100, 12);
		// No recorded completions means no historical reward.
		expect(row.displayLiquidReward).toBe(0);
		expect(Number.isFinite(row.avgNet)).toBe(true);
	});
});
