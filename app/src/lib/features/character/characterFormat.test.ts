import { describe, expect, it } from 'vitest';
import { formatGain, formatProfLevel, gainColorClass } from './characterFormat';

describe('formatGain', () => {
	it('renders a dash for null (no anchor on record)', () => {
		expect(formatGain(null)).toBe('\u2014');
	});

	it('collapses near-zero gains to an unsigned 0.00', () => {
		expect(formatGain(0)).toBe('0.00');
		expect(formatGain(0.0049)).toBe('0.00');
		expect(formatGain(-0.0049)).toBe('0.00');
	});

	it('prefixes positive gains with a plus and keeps the negative sign', () => {
		expect(formatGain(1.234)).toBe('+1.23');
		expect(formatGain(-2.5)).toBe('-2.50');
	});
});

describe('gainColorClass', () => {
	it('mutes null and near-zero gains', () => {
		expect(gainColorClass(null)).toBe('text-text-tertiary');
		expect(gainColorClass(0.0049)).toBe('text-text-tertiary');
	});

	it('colours positive gains as success and negative as warning', () => {
		expect(gainColorClass(0.01)).toBe('text-success');
		expect(gainColorClass(-0.01)).toBe('text-warning');
	});
});

describe('formatProfLevel', () => {
	it('renders a dash for null', () => {
		expect(formatProfLevel(null)).toBe('\u2014');
	});

	it('renders the floored level with the fractional part as a percentage', () => {
		expect(formatProfLevel(12.345)).toBe('12 (34.5%)');
		expect(formatProfLevel(50)).toBe('50 (0.0%)');
		expect(formatProfLevel(0.5)).toBe('0 (50.0%)');
	});
});
