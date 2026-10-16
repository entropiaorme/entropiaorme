/**
 * The sessions table's rows: each named session carried to the goal as
 * recorded, with the expected TT lost above the cheapest one that answers.
 */

import type { SkillingForecastSource } from '$lib/api/commands.gen';

export interface SessionRow {
	source: SkillingForecastSource;
	/** Expected TT lost above the cheapest answering session the model
	 * prices; null when this one does not answer, the model prices none of
	 * its weapons, or nothing else compares. */
	premium: number | null;
	/** That premium per hour of this session's play; null without both. */
	premiumPerHour: number | null;
}

/** Every named session in the forecast's order, with premiums against the
 * cheapest that answers. The premium compares expected losses: a recorded
 * loss leads or trails on whether a run has met its rare large loot yet. */
export function sessionRows(sources: SkillingForecastSource[]): SessionRow[] {
	const rows: SessionRow[] = sources.map((source) => ({
		source,
		premium: null,
		premiumPerHour: null,
	}));
	const priced = rows.flatMap((row) =>
		row.source.status === 'ready' && row.source.expected !== null
			? [{ row, cost: row.source.expected.ttCost }]
			: [],
	);
	if (priced.length > 1) {
		const cheapest = Math.min(...priced.map(({ cost }) => cost));
		for (const { row, cost } of priced) {
			row.premium = cost - cheapest;
			row.premiumPerHour = row.source.hours > 0 ? row.premium / row.source.hours : null;
		}
	}
	return rows;
}
