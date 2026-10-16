/**
 * The sessions table's rows: each named session carried to the goal as
 * recorded, with the TT lost above the cheapest one that answers.
 */

import type { SkillingForecastSource } from '$lib/api/commands.gen';

export interface SessionRow {
	source: SkillingForecastSource;
	/** TT lost above the cheapest answering session; null when this one
	 * does not answer or nothing else does. */
	premium: number | null;
	/** That premium per hour of this session's play; null without both. */
	premiumPerHour: number | null;
}

/** Every named session in the forecast's order, with premiums against the
 * cheapest that answers. */
export function sessionRows(sources: SkillingForecastSource[]): SessionRow[] {
	const rows: SessionRow[] = sources.map((source) => ({
		source,
		premium: null,
		premiumPerHour: null,
	}));
	const ready = rows.filter((row) => row.source.status === 'ready');
	if (ready.length > 1) {
		const cheapest = Math.min(...ready.map((row) => row.source.ttCost));
		for (const row of ready) {
			row.premium = row.source.ttCost - cheapest;
			row.premiumPerHour = row.source.hours > 0 ? row.premium / row.source.hours : null;
		}
	}
	return rows;
}
