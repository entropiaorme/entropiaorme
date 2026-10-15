import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MarketHistoryPoint, MarketOverviewRow } from '$lib/api';
import {
	ageDays,
	createOverviewModel,
	formatAge,
	formatSalesPed,
	resolveReading,
} from './overviewModel.svelte';

vi.mock('$lib/api', () => ({
	getMarketOverview: vi.fn(),
	getMarketItemHistory: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

const reading = (markupPct: number | null, salesPed: number) => ({ markupPct, salesPed });

function row(overrides: Partial<MarketOverviewRow> = {}): MarketOverviewRow {
	return {
		itemName: 'Carabok Hide',
		tier: 0,
		observedAt: 1_752_000_000,
		day: reading(106.88, 451.9),
		week: reading(107.16, 531.9),
		month: reading(106.02, 979.04),
		year: reading(108.28, 13_500),
		decade: reading(158.92, 35_300),
		...overrides,
	};
}

function point(observedAt: number, markupPct: number | null): MarketHistoryPoint {
	return { observedAt, markupPct, salesPed: 100 };
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('resolveReading', () => {
	it('prefers the week', () => {
		expect(resolveReading(row())).toEqual({ horizon: 'week', markupPct: 107.16, salesPed: 531.9 });
	});

	it('falls back through month then year, never to day or decade', () => {
		const noWeek = row({ week: reading(null, 0) });
		expect(resolveReading(noWeek).horizon).toBe('month');
		const yearOnly = row({ week: reading(null, 0), month: reading(null, 0) });
		expect(resolveReading(yearOnly)).toEqual({
			horizon: 'year',
			markupPct: 108.28,
			salesPed: 13_500,
		});
	});

	it('keeps a no-sales item null rather than zero', () => {
		const none = row({ week: reading(null, 0), month: reading(null, 0), year: reading(null, 0) });
		expect(resolveReading(none)).toEqual({ horizon: 'week', markupPct: null, salesPed: 0 });
	});
});

describe('createOverviewModel', () => {
	it('loads rows with their resolved readings and filters by search', async () => {
		mocked.getMarketOverview.mockResolvedValue([
			row({ itemName: 'Animal Oil Residue' }),
			row({ itemName: 'Carabok Hide' }),
			row({ itemName: 'Carabok Leg Fur', week: reading(null, 0) }),
		]);
		const model = createOverviewModel();
		await model.loadData();

		expect(model.loading).toBe(false);
		expect(model.items.map((item) => item.resolved.horizon)).toEqual(['week', 'week', 'month']);
		model.search = 'carabok';
		expect(model.visibleItems.map((item) => item.row.itemName)).toEqual([
			'Carabok Hide',
			'Carabok Leg Fur',
		]);
	});

	it('expanding an item loads its history on the resolved horizon, newest first', async () => {
		mocked.getMarketOverview.mockResolvedValue([row({ week: reading(null, 0) })]);
		mocked.getMarketItemHistory.mockResolvedValue([point(1, 100), point(2, 105)]);
		const model = createOverviewModel();
		await model.loadData();

		await model.toggle('Carabok Hide');
		expect(mocked.getMarketItemHistory).toHaveBeenCalledWith('Carabok Hide', 'month');
		expect(model.expandedItem).toBe('Carabok Hide');
		expect(model.history.map((entry) => entry.observedAt)).toEqual([2, 1]);

		await model.toggle('Carabok Hide');
		expect(model.expandedItem).toBeNull();
		expect(model.history).toEqual([]);
	});

	it('a slow history for a superseded item never lands on the newer one', async () => {
		mocked.getMarketOverview.mockResolvedValue([row(), row({ itemName: 'Carabok Leg Fur' })]);
		let resolveFirst: (value: MarketHistoryPoint[]) => void = () => {};
		mocked.getMarketItemHistory
			.mockImplementationOnce(() => new Promise((resolve) => (resolveFirst = resolve)))
			.mockResolvedValueOnce([point(9, 120)]);
		const model = createOverviewModel();
		await model.loadData();

		const first = model.toggle('Carabok Hide');
		await model.toggle('Carabok Leg Fur');
		resolveFirst([point(1, 100)]);
		await first;
		expect(model.expandedItem).toBe('Carabok Leg Fur');
		expect(model.history.map((entry) => entry.observedAt)).toEqual([9]);
		expect(model.historyLoading).toBe(false);
	});

	it('surfaces a load failure', async () => {
		mocked.getMarketOverview.mockRejectedValue(new Error('boom'));
		const model = createOverviewModel();
		await model.loadData();
		expect(model.error).not.toBeNull();
		expect(model.items).toHaveLength(0);
	});
});

describe('formatting helpers', () => {
	it('compacts sales volume', () => {
		expect(formatSalesPed(451.9)).toBe('451.90 PED');
		expect(formatSalesPed(13_500)).toBe('13.5K PED');
		expect(formatSalesPed(45_300_000)).toBe('45.3M PED');
	});

	it('labels observation age', () => {
		const now = 1_752_000_000;
		expect(ageDays(now, now)).toBe(0);
		expect(formatAge(now, now)).toBe('today');
		expect(formatAge(now - 3 * 86_400, now)).toBe('3d ago');
		expect(formatAge(now - 21 * 86_400, now)).toBe('3w ago');
	});
});
