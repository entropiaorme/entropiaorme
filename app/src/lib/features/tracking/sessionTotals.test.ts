import { describe, expect, it } from 'vitest';
import type { SessionSummary } from '$lib/api/commands.gen';
import { totalsAfterToggle } from './sessionTotals';

function summary(cost: number, overheadCost: number): SessionSummary {
	return {
		cost,
		returns: 0,
		pes: 0,
		net: 0,
		returnRate: 0,
		kills: 0,
		duration: 0,
		costBasis: 'measured',
		costBreakdown: { overheadCost } as SessionSummary['costBreakdown'],
	};
}

describe("a session's totals after a loot toggle, when it cannot be read again", () => {
	it('rounds the Net once, from the parts as they are, never from rounded parts', () => {
		// 12.005 less 10 less 0.004 is 2.001: once rounded, 2.00. Rounding the
		// parts first (12.01 less 10 less 0.00) would read 2.01.
		const totals = totalsAfterToggle(summary(10, 0.004), 12.005);
		expect(totals.net).toBe(2);
		expect(totals.returns).toBe(12.005);
	});

	it('takes the overhead off the Net, and leaves it out of the return rate', () => {
		const totals = totalsAfterToggle(summary(10, 0.5), 12);
		expect(totals.net).toBe(1.5);
		expect(totals.returnRate).toBeCloseTo(1.2, 9);
		expect(totalsAfterToggle(summary(0, 0), 3).returnRate).toBe(0);
	});
});
