import { describe, expect, it } from 'vitest';
import type { MarketHarvestItem } from '$lib/api';
import { entryMarkupPct, initialKind, initialText, parseEntry, unitTt } from './manualMarkupEntry';

function market(overrides: Partial<MarketHarvestItem> = {}): MarketHarvestItem {
	return {
		itemName: 'Animal Oil Residue',
		markupPct: 104.25,
		marketObservedAt: 1_700_000_000,
		manual: null,
		preferred: 'market',
		nanocubeMarked: false,
		unitPricePed: null,
		horizon: 'week',
		salesPed: 50,
		recommendedPacketTt: null,
		readings: [],
		...overrides,
	};
}

describe('parseEntry', () => {
	it('reads the forms the game quotes markup in', () => {
		expect(parseEntry('percent', '130')).toEqual({ ok: true, value: 130 });
		expect(parseEntry('percent', ' 104,5 % ')).toEqual({ ok: true, value: 104.5 });
		expect(parseEntry('ped_per_unit', '+2.50')).toEqual({ ok: true, value: 2.5 });
		expect(parseEntry('ped_per_unit', '0.3 PED')).toEqual({ ok: true, value: 0.3 });
		expect(parseEntry('ped_per_unit', '0')).toEqual({ ok: true, value: 0 });
	});

	it('refuses what is not a markup, saying why', () => {
		expect(parseEntry('percent', '')).toEqual({ ok: false, error: 'Enter a markup' });
		expect(parseEntry('percent', 'lots')).toEqual({ ok: false, error: 'Enter a number' });
		expect(parseEntry('percent', '-5')).toEqual({ ok: false, error: 'Enter a number' });
		expect(parseEntry('percent', '0')).toEqual({ ok: false, error: 'A markup is above 0%' });
	});
});

describe('entryMarkupPct', () => {
	it('reads a per-unit amount as a markup over the unit TT', () => {
		expect(entryMarkupPct('percent', 130, null)).toBe(130);
		expect(entryMarkupPct('ped_per_unit', 2.5, 5)).toBe(150);
		expect(entryMarkupPct('ped_per_unit', 2.5, 0)).toBeNull();
		expect(unitTt(4, 2)).toBe(0.5);
		expect(unitTt(0, 2)).toBeNull();
	});
});

describe('the field starts from what stands', () => {
	it('starts from the player entry in its own form', () => {
		const item = market({
			manual: {
				kind: 'ped_per_unit',
				value: 2.5,
				unitTtPed: 5,
				markupPct: 150,
				recordedAt: 1_700_000_100,
			},
			preferred: 'manual',
		});
		expect(initialKind(item)).toBe('ped_per_unit');
		expect(initialText(item, 'ped_per_unit')).toBe('2.5');
		// Switching form starts the percentage from the market reading.
		expect(initialText(item, 'percent')).toBe('104.25');
	});

	it('starts a fresh entry from the market percentage, or empty', () => {
		expect(initialKind(market())).toBe('percent');
		expect(initialText(market(), 'percent')).toBe('104.25');
		expect(initialText(market({ markupPct: null }), 'percent')).toBe('');
		expect(initialText(undefined, 'percent')).toBe('');
	});
});
