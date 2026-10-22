import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	formatDate,
	formatDateFull,
	formatDuration,
	formatElapsed,
	formatLedgerDate,
	formatMultiplier,
	formatOrdinal,
	formatPed,
	formatPedHalfEven,
	formatPercent,
	todayDate,
} from '$lib/utils/format';

describe('todayDate', () => {
	// The suite pins TZ to UTC, so the divergence this function exists to
	// avoid has to be created deliberately.
	const suiteTz = process.env.TZ;
	afterEach(() => {
		vi.useRealTimers();
		// Assigning an absent value would leave the string "undefined" behind
		// and contaminate every later date test.
		if (suiteTz === undefined) {
			delete process.env.TZ;
		} else {
			process.env.TZ = suiteTz;
		}
	});

	it('reports the local calendar day, zero-padded', () => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date(2026, 6, 5, 14, 30));
		expect(todayDate()).toBe('2026-07-05');
	});

	it('stays on the local day when UTC has already rolled over', () => {
		// 05:00 UTC on the 27th is 22:00 on the 26th in Los Angeles. The
		// player is having the 26th, and a listing they do not touch the date
		// on must be dated the day they made it.
		process.env.TZ = 'America/Los_Angeles';
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-07-27T05:00:00Z'));
		expect(todayDate()).toBe('2026-07-26');
	});
});

describe('formatPedHalfEven', () => {
	it('rounds exact halves to the even cent, matching the game display', () => {
		expect(formatPedHalfEven(15.625)).toBe('15.62');
		expect(formatPedHalfEven(15.635)).toBe('15.64');
		expect(formatPedHalfEven(7.8125)).toBe('7.81');
		expect(formatPedHalfEven(25)).toBe('25.00');
	});

	it('rounds non-halves to the nearest cent', () => {
		expect(formatPedHalfEven(12.3456)).toBe('12.35');
		expect(formatPedHalfEven(1.504)).toBe('1.50');
	});
});

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

describe('formatOrdinal', () => {
	it('suffixes a place by English ordinal rules', () => {
		expect([1, 2, 3, 4, 10].map(formatOrdinal)).toEqual(['1st', '2nd', '3rd', '4th', '10th']);
		expect([11, 12, 13].map(formatOrdinal)).toEqual(['11th', '12th', '13th']);
		expect([21, 22, 23, 101, 111].map(formatOrdinal)).toEqual([
			'21st',
			'22nd',
			'23rd',
			'101st',
			'111th',
		]);
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

	it('renders a date-only entry as its day, never as a time', () => {
		// A bare date knows no time, so inventing one is wrong in both
		// directions: read as UTC midnight it renders as "1:00 AM" today east
		// of UTC, and as the day before west of it.
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-07-26T09:00:00Z'));
		expect(formatLedgerDate('2026-07-26')).toBe('Jul 26');
		expect(formatLedgerDate('2026-03-24')).toBe('Mar 24');
	});

	it('treats a cross-year same-M/D as NOT today (compares full year)', () => {
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-06-03T12:00:00Z'));
		// Same month/day as "today" but a year earlier: must be a date, not a time.
		expect(formatLedgerDate('2025-06-03T08:30:00Z')).toBe('Jun 3');
	});
});
