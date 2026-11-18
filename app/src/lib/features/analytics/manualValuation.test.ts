import { describe, expect, it } from 'vitest';
import type { MarketHarvestItem } from '$lib/api';
import {
	effectiveItemMarkup,
	manualValuation,
	marketOpportunity,
	projectLoot,
	SHRAPNEL_CONVERSION_MARKUP,
} from './treeCuttingModel.svelte';

const NANOCUBE = 100.6;

function market(overrides: Partial<MarketHarvestItem> = {}): MarketHarvestItem {
	return {
		itemName: 'Animal Oil Residue',
		markupPct: 104,
		marketObservedAt: 1_700_000_000,
		manual: null,
		preferred: 'market',
		nanocubeMarked: false,
		unitPricePed: null,
		horizon: 'week',
		salesPed: 50_000,
		recommendedPacketTt: null,
		readings: [{ horizon: 'week', markupPct: 104, salesPed: 50_000 }],
		...overrides,
	};
}

function manual(kind: 'percent' | 'ped_per_unit', value: number) {
	return {
		kind,
		value,
		unitTtPed: null,
		markupPct: kind === 'percent' ? value : null,
		recordedAt: 1_700_000_100,
	} as const;
}

describe('manualValuation', () => {
	it('applies only when the backend names the manual entry the more recent', () => {
		const entry = manual('percent', 130);
		expect(manualValuation(market({ manual: entry, preferred: 'market' }), 10, 5)).toBeNull();
		expect(manualValuation(market({ manual: entry, preferred: 'manual' }), 10, 5)).toEqual({
			valuePed: 6.5,
			markupPct: 130,
			entry,
		});
	});

	it('values a per-unit entry exactly, whatever the unit TT', () => {
		const valued = manualValuation(
			market({ manual: manual('ped_per_unit', 0.5), preferred: 'manual' }),
			4,
			2,
		);
		// 2 PED TT plus 4 units at +0.5 PED.
		expect(valued?.valuePed).toBe(4);
		expect(valued?.markupPct).toBe(200);
	});

	it('gives a zero-TT item its absolute value', () => {
		const valued = manualValuation(
			market({ manual: manual('ped_per_unit', 1.5), preferred: 'manual' }),
			2,
			0,
		);
		expect(valued?.valuePed).toBe(3);
		expect(valued?.markupPct).toBe(100);
	});
});

describe('effectiveItemMarkup precedence', () => {
	// An unsupported market: the confidence rule alone floors it.
	const thin = marketOpportunity(market({ salesPed: 1, markupPct: 104 }), NANOCUBE);

	it('takes Shrapnel at its conversion over every override', () => {
		expect(
			effectiveItemMarkup('Shrapnel', thin, NANOCUBE, 'liquid', {
				nanocubeMarked: true,
				manual: { valuePed: 1, markupPct: 150, entry: manual('percent', 150) },
			}),
		).toEqual({ markupPct: SHRAPNEL_CONVERSION_MARKUP, basis: 'shrapnel_conversion' });
	});

	it('puts the live Nanocube mark above the player entry and the market', () => {
		expect(
			effectiveItemMarkup('Animal Oil Residue', thin, NANOCUBE, 'all', {
				nanocubeMarked: true,
				manual: { valuePed: 1, markupPct: 150, entry: manual('percent', 150) },
			}),
		).toEqual({ markupPct: NANOCUBE, basis: 'nanocube_mark' });
	});

	it('trusts the player entry past the confidence filter and the floor', () => {
		expect(
			effectiveItemMarkup('Animal Oil Residue', thin, NANOCUBE, 'liquid', {
				manual: { valuePed: 1, markupPct: 95, entry: manual('percent', 95) },
			}),
		).toEqual({ markupPct: 95, basis: 'manual' });
	});

	it('falls back to the market under the confidence rule', () => {
		expect(effectiveItemMarkup('Animal Oil Residue', thin, NANOCUBE, 'liquid')).toEqual({
			markupPct: NANOCUBE,
			basis: 'nanocube',
		});
	});
});

describe('projectLoot with the player entries', () => {
	const loot = [
		{ itemName: 'Animal Oil Residue', quantity: 100, valuePed: 10 },
		{ itemName: 'Animal Hide', quantity: 10, valuePed: 5 },
	];
	const feed = {
		nanocubeMarkupPct: NANOCUBE,
		items: [
			market({
				itemName: 'Animal Oil Residue',
				manual: manual('percent', 130),
				preferred: 'manual',
			}),
			market({ itemName: 'Animal Hide', nanocubeMarked: true }),
		],
	};
	const byItem = new Map(feed.items.map((item) => [item.itemName, item]));

	it('honours the Nanocube mark only in the live projection', () => {
		const live = projectLoot(loot, 0, feed, byItem, 'all', { live: true });
		const hide = live.items.find((item) => item.name === 'Animal Hide');
		expect(hide?.markupBasis).toBe('nanocube_mark');
		expect(hide?.floored).toBe(true);

		const recorded = projectLoot(loot, 0, feed, byItem, 'all');
		const recordedHide = recorded.items.find((item) => item.name === 'Animal Hide');
		expect(recordedHide?.markupBasis).toBe('market');
		expect(recordedHide?.nanocubeMarked).toBe(true);
	});

	it('values the player entry in the projected total', () => {
		const live = projectLoot(loot, 0, feed, byItem, 'all', { live: true });
		const oil = live.items.find((item) => item.name === 'Animal Oil Residue');
		expect(oil?.markupBasis).toBe('manual');
		expect(oil?.marketValuePed).toBe(13);
		// 13 PED of oil at 130% plus the hide at the Nanocube floor.
		expect(live.muProjectedReturns).toBeCloseTo(13 + 5 * 1.006, 9);
	});
});
