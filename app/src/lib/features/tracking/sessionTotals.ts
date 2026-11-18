/**
 * A session's totals after a loot toggle, worked out here only when the
 * session cannot be read again: the returns from the toggle's answer, the
 * cost shown (Cycled) and the session's overhead as they stand. The Net is
 * rounded once, from the parts as they are, so it can differ from the
 * backend's by no more than its own rounding.
 */

import type { SessionSummary } from '$lib/api/commands.gen';

export function totalsAfterToggle(
	summary: SessionSummary,
	sessionTotalReturns: number,
): Pick<SessionSummary, 'returns' | 'net' | 'returnRate'> {
	const cost = summary.cost;
	const overhead = summary.costBreakdown?.overheadCost ?? 0;
	return {
		returns: sessionTotalReturns,
		net: Math.round((sessionTotalReturns - cost - overhead) * 100) / 100,
		returnRate: cost > 0 ? sessionTotalReturns / cost : 0,
	};
}
