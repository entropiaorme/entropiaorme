import { describe, expect, it } from 'vitest';
import { enrichmentColor, enrichmentLabel } from './display';

describe('enrichment ladder', () => {
	it('maps the four levels to their labels and colours', () => {
		expect(enrichmentLabel(0)).toBe('Unresolved');
		expect(enrichmentLabel(1)).toBe('Base');
		expect(enrichmentLabel(2)).toBe('Base + Amp');
		expect(enrichmentLabel(3)).toBe('Full Setup');
		expect(enrichmentColor(0)).toBe('negative');
		expect(enrichmentColor(1)).toBe('warning');
		expect(enrichmentColor(2)).toBe('accent');
		expect(enrichmentColor(3)).toBe('positive');
	});

	it('reads anything outside the ladder as unresolved', () => {
		expect(enrichmentLabel(7)).toBe('Unresolved');
		expect(enrichmentLabel(-1)).toBe('Unresolved');
		expect(enrichmentColor(7)).toBe('negative');
	});
});
