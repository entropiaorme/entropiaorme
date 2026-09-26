import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MonthlyEntry, OverviewStats, TimelineDay } from '$lib/types/analytics';
import { createOverviewModel, labelFor, PIE_C } from './overviewModel.svelte';

vi.mock('$lib/api', () => ({
	getAnalyticsOverview: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function day(overrides: Partial<TimelineDay> = {}): TimelineDay {
	return {
		date: '2026-07-01',
		lootTt: 0,
		questItemTt: 0,
		pes: 0,
		codexPes: 0,
		questPes: 0,
		ledgerGains: {},
		trackingCost: 0,
		ledgerLosses: {},
		...overrides,
	};
}

function month(overrides: Partial<MonthlyEntry> = {}): MonthlyEntry {
	return {
		month: '2026-07',
		lootTt: 0,
		questItemTt: 0,
		pes: 0,
		codexPes: 0,
		questPes: 0,
		ledgerGains: {},
		trackingCost: 0,
		ledgerLosses: {},
		...overrides,
	};
}

function overview(overrides: Partial<OverviewStats> = {}): OverviewStats {
	return {
		totalReturnRate: 0.85,
		trend: 'stable',
		returnsBreakdown: {
			lootTt: 850,
			questItemTt: 0,
			pes: 12.5,
			codexPes: 3,
			questPes: 1.5,
			ledger: { item_sale: 120, quest_reward: 30, codex: 5 },
		},
		lossesBreakdown: {
			trackingCost: 1000,
			cycledBreakdown: {
				weapon: 700,
				healing: 150,
				enhancer: 100,
				armour: 50,
				dangling: 0,
				consumables: 0,
			},
			ledger: { equipment: 40, repair: 20 },
		},
		totalGains: 1000,
		totalLosses: 1060,
		timeline: [],
		monthlyBreakdown: [],
		...overrides,
	};
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('loadData', () => {
	it('loads the overview and seeds the config from the ledger tags', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(overview());
		const model = createOverviewModel();
		await model.loadData('all');

		expect(model.data).not.toBeNull();
		expect(model.loading).toBe(false);
		// Progression tags (codex) stay out of the gain config; losses take all tags.
		expect(model.config.gainTags).toEqual({ item_sale: true, quest_reward: true });
		expect(model.config.lossTags).toEqual({ equipment: true, repair: true });
	});

	it('maps the active range to its wire period', () => {
		const model = createOverviewModel();
		expect(model.period).toBe('all');
		model.activeRange = '30d';
		expect(model.period).toBe('30d');
	});

	it('ignores a slow response for a superseded period', async () => {
		let resolveFirst!: (v: OverviewStats) => void;
		mocked.getAnalyticsOverview
			.mockImplementationOnce(
				() =>
					new Promise<OverviewStats>((res) => {
						resolveFirst = res;
					}),
			)
			.mockResolvedValueOnce(overview({ totalReturnRate: 0.3 }));
		const model = createOverviewModel();
		const first = model.loadData('all');
		await model.loadData('30d');
		expect(model.data?.totalReturnRate).toBe(0.3);

		resolveFirst(overview({ totalReturnRate: 0.99 }));
		await first;
		expect(model.data?.totalReturnRate).toBe(0.3);
		expect(model.loading).toBe(false);
	});

	it('surfaces a load failure and clears a stale error on entry', async () => {
		mocked.getAnalyticsOverview.mockRejectedValueOnce(new Error('backend unreachable'));
		const model = createOverviewModel();
		await model.loadData('all');
		expect(model.error).toBe('backend unreachable');

		mocked.getAnalyticsOverview.mockResolvedValue(overview());
		await model.loadData('all');
		expect(model.error).toBeNull();
	});
});

describe('pieView', () => {
	it('builds config-gated arcs whose lengths tile the gain total', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(overview());
		const model = createOverviewModel();
		await model.loadData('all');

		const pie = model.pieView;
		expect(pie).not.toBeNull();
		if (!pie) return;
		expect(pie.gains).toBe(1000); // 850 loot + 120 + 30 (codex excluded from config)
		expect(pie.losses).toBe(1060); // 1000 cycled + 40 + 20
		expect(pie.rate).toBeCloseTo(1000 / 1060, 10);
		expect(pie.arcs.map((a) => a.label)).toEqual(['TT Loot', 'Auction Sales', 'Quest Rewards']);
		const total = pie.arcs.reduce((s, a) => s + a.length, 0);
		expect(total).toBeCloseTo(PIE_C, 8);
		expect(pie.arcs[1].offset).toBeCloseTo(pie.arcs[0].length, 8);
	});

	it('drops a toggled-off gain source from the arcs and totals', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(overview());
		const model = createOverviewModel();
		await model.loadData('all');

		model.config.gainTags.item_sale = false;
		const pie = model.pieView;
		expect(pie?.gains).toBe(880);
		expect(pie?.arcs.map((a) => a.label)).toEqual(['TT Loot', 'Quest Rewards']);
	});

	it('returns null when either side is empty', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({
				returnsBreakdown: {
					lootTt: 0,
					questItemTt: 0,
					pes: 0,
					codexPes: 0,
					questPes: 0,
					ledger: {},
				},
			}),
		);
		const model = createOverviewModel();
		await model.loadData('all');
		expect(model.pieView).toBeNull();
	});
});

describe('timeline', () => {
	it('produces cumulative points spanning x=40..760 with the zero line placed by range', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({
				timeline: [
					day({ date: '2026-07-01', lootTt: 100, trackingCost: 50 }),
					day({ date: '2026-07-02', lootTt: 0, trackingCost: 100 }),
					day({ date: '2026-07-03', lootTt: 30, trackingCost: 20 }),
				],
			}),
		);
		const model = createOverviewModel();
		await model.loadData('all');

		const points = model.chartPoints;
		expect(points.map((p) => p.value)).toEqual([50, -50, -40]);
		expect(points[0].x).toBe(40);
		expect(points[2].x).toBe(760);
		// Range is -50..50; zero sits mid-band: y = 28 + (50/100) * 112.
		expect(model.zeroY).toBeCloseTo(84, 10);
		expect(points[0].y).toBeCloseTo(28, 10);
		expect(points[1].y).toBeCloseTo(140, 10);
		expect(model.chartPath).toBe(points.map((p) => `${p.x},${p.y}`).join(' '));
		expect(model.chartFillPath.endsWith(` 760,${model.zeroY} 40,${model.zeroY}`)).toBe(true);
	});

	it('honours the gain/loss config in the cumulative net', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({
				timeline: [
					day({ date: '2026-07-01', lootTt: 10, ledgerGains: { item_sale: 5 }, trackingCost: 8 }),
					day({ date: '2026-07-02', ledgerLosses: { equipment: 4 } }),
				],
			}),
		);
		const model = createOverviewModel();
		await model.loadData('all');
		expect(model.chartPoints.map((p) => p.value)).toEqual([7, 3]);

		model.config.gainTags.item_sale = false;
		expect(model.chartPoints.map((p) => p.value)).toEqual([2, -2]);
	});

	it('returns no points for a sub-two-day timeline', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(overview({ timeline: [day()] }));
		const model = createOverviewModel();
		await model.loadData('all');
		expect(model.chartPoints).toEqual([]);
		expect(model.zeroY).toBe(84);
		expect(model.chartFillPath).toBe('');
	});
});

describe('monthlyRows', () => {
	it('derives costs, returns, rates, and the combined PES per month', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({
				monthlyBreakdown: [
					month({
						month: '2026-06',
						lootTt: 90,
						ledgerGains: { item_sale: 10 },
						trackingCost: 100,
						ledgerLosses: { equipment: 25 },
						pes: 1,
						codexPes: 2,
						questPes: 3,
					}),
					month({ month: '2026-07' }),
				],
			}),
		);
		const model = createOverviewModel();
		await model.loadData('all');

		const rows = model.monthlyRows;
		expect(rows[0]).toEqual({
			month: '2026-06',
			cost: 125,
			returns: 100,
			net: -25,
			lootRate: 0.9,
			globalRate: 0.8,
			pes: 6,
		});
		// A zero month yields null rates rather than dividing by zero.
		expect(rows[1].lootRate).toBeNull();
		expect(rows[1].globalRate).toBeNull();
	});
});

describe('labelFor', () => {
	it('maps known tags and title-cases unknown snake_case tags', () => {
		expect(labelFor('item_sale')).toBe('Auction Sales');
		expect(labelFor('space_travel')).toBe('Space travel');
	});
});
