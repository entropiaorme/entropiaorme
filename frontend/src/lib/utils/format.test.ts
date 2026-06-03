import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	formatDate,
	formatDateFull,
	formatDuration,
	formatElapsed,
	formatLedgerDate,
	formatMultiplier,
	formatPed,
	formatPercent,
} from '$lib/utils/format';

describe('formatPed', () => {
	it('formats positive values to 2 decimals', () => {
		expect(formatPed(1.5)).toBe('1.50');
		expect(formatPed(12.3456)).toBe('12.35');
	});

	it('formats negative values', () => {
		expect(formatPed(-2.5)).toBe('-2.50');
	});

	it('formats zero', () => {
		expect(formatPed(0)).toBe('0.00');
	});

	it('passes through NaN and infinities (toFixed quirk)', () => {
		expect(formatPed(NaN)).toBe('NaN');
		expect(formatPed(Infinity)).toBe('Infinity');
		expect(formatPed(-Infinity)).toBe('-Infinity');
	});

	it('exhibits the float rounding quirk at .675', () => {
		// 2.675 is not exactly representable; toFixed(2) yields '2.67', not '2.68'.
		expect(formatPed(2.675)).toBe('2.67');
	});
});

describe('formatPercent', () => {
	it('formats a sub-unity ratio', () => {
		expect(formatPercent(0.917)).toBe('91.7%');
	});

	it('formats ratios above 1', () => {
		expect(formatPercent(1.5)).toBe('150.0%');
	});

	it('formats negative ratios', () => {
		expect(formatPercent(-0.1)).toBe('-10.0%');
	});

	it('formats zero', () => {
		expect(formatPercent(0)).toBe('0.0%');
	});
});

describe('formatMultiplier', () => {
	it('uses 2 decimals below 10', () => {
		expect(formatMultiplier(1.5)).toBe('1.50x');
		expect(formatMultiplier(9.99)).toBe('9.99x');
	});

	it('uses 1 decimal at or above 10', () => {
		expect(formatMultiplier(10)).toBe('10.0x');
		expect(formatMultiplier(10.0)).toBe('10.0x');
		expect(formatMultiplier(25)).toBe('25.0x');
	});

	it('branches on the RAW value before rounding (9.999 -> 10.00x)', () => {
		// 9.999 < 10, so the 2dp branch is taken; toFixed(2) then rounds up to
		// '10.00', producing the visually-inconsistent '10.00x'.
		expect(formatMultiplier(9.999)).toBe('10.00x');
	});
});

describe('formatDuration', () => {
	it('formats zero as minutes only', () => {
		expect(formatDuration(0)).toBe('0m');
	});

	it('formats sub-hour durations as minutes only', () => {
		expect(formatDuration(1800)).toBe('30m');
	});

	it('zero-pads minutes when an hour component is present', () => {
		expect(formatDuration(3660)).toBe('1h 01m');
	});

	it('formats whole hours with 00m', () => {
		expect(formatDuration(7200)).toBe('2h 00m');
	});
});

describe('formatElapsed', () => {
	it('formats zero', () => {
		expect(formatElapsed(0)).toBe('0:00');
	});

	it('zero-pads single-digit seconds', () => {
		expect(formatElapsed(5)).toBe('0:05');
	});

	it('formats just under a minute', () => {
		expect(formatElapsed(59)).toBe('0:59');
	});

	it('rolls over to a minute', () => {
		expect(formatElapsed(60)).toBe('1:00');
	});

	it('formats minutes and seconds', () => {
		expect(formatElapsed(332)).toBe('5:32');
	});

	it('formats a whole hour', () => {
		expect(formatElapsed(3600)).toBe('1:00:00');
	});

	it('formats hours, minutes, and seconds', () => {
		expect(formatElapsed(3932)).toBe('1:05:32');
	});

	it('does not floor fractional seconds (un-floored-seconds quirk)', () => {
		// Seconds use `seconds % 60` without flooring, so a fractional input
		// leaks the decimal; padStart is a no-op on the already-3-char string.
		expect(formatElapsed(5.9)).toBe('0:5.9');
	});
});

describe('formatDate / formatDateFull', () => {
	it('formats a short date (UTC-pinned)', () => {
		expect(formatDate('2026-03-24T00:00:00Z')).toBe('Mar 24');
	});

	it('formats a full date with year', () => {
		expect(formatDateFull('2026-03-24T00:00:00Z')).toBe('Mar 24, 2026');
	});

	it('returns "Invalid Date" for unparseable input (both forms)', () => {
		expect(formatDate('not-a-date')).toBe('Invalid Date');
		expect(formatDateFull('not-a-date')).toBe('Invalid Date');
	});
});

describe('formatLedgerDate', () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it('renders a same-day timestamp as a time (UTC)', () => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-06-03T12:00:00Z'));
		expect(formatLedgerDate('2026-06-03T08:30:00Z')).toBe('8:30 AM');
	});

	it('renders a different day as a short date', () => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-06-03T12:00:00Z'));
		expect(formatLedgerDate('2026-03-24T00:00:00Z')).toBe('Mar 24');
	});

	it('treats a cross-year same-M/D as NOT today (compares full year)', () => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-06-03T12:00:00Z'));
		// Same month/day as "today" but a year earlier: must be a date, not a time.
		expect(formatLedgerDate('2025-06-03T08:30:00Z')).toBe('Jun 3');
	});
});
