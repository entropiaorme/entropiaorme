import { describe, expect, it } from 'vitest';
import type { MarketManualMarkup } from '$lib/api';
import { appliedMarkupText, manualMarkupLabel } from './marketConfidence';

const perUnit: MarketManualMarkup = {
	kind: 'ped_per_unit',
	value: 0.7,
	unitTtPed: 0.5,
	markupPct: 240,
	recordedAt: 1_700_000_000,
};
const percent: MarketManualMarkup = { ...perUnit, kind: 'percent', value: 130, markupPct: 130 };

describe('appliedMarkupText', () => {
	it("reads the player's own value in the form they entered it, whatever the TT", () => {
		expect(appliedMarkupText(240, 'manual', perUnit)).toBe('+0.70 ea');
		expect(appliedMarkupText(100, 'manual', perUnit)).toBe('+0.70 ea');
		expect(appliedMarkupText(130, 'manual', percent)).toBe('130.0%');
	});

	it('reads every other route as a percentage of TT', () => {
		expect(appliedMarkupText(112.5, 'market', perUnit)).toBe('112.5%');
		expect(appliedMarkupText(100.6, 'nanocube', null)).toBe('100.6%');
		expect(appliedMarkupText(101, 'shrapnel_conversion', undefined)).toBe('101.0%');
	});
});

describe('manualMarkupLabel', () => {
	it('names a per-unit entry with its percentage where the item has TT', () => {
		expect(manualMarkupLabel(perUnit, 240, 5)).toBe('Your value +0.70 PED each (240.0% of TT)');
		expect(manualMarkupLabel(perUnit, 100, 0)).toBe(
			'Your value +0.70 PED each; the item has no TT',
		);
		expect(manualMarkupLabel(percent, 130, 5)).toBe('Your markup 130.0%');
	});
});
