/**
 * Wire-shaped account fixtures for the account feature's tests: every list
 * field present, every amount carrying its precision, and only shapes the
 * backend can produce (a gained total comes with the gained lines that
 * make it, an empty sum is exact, a sale has the items it sold). A test
 * that sets a measure's lines sets its totals to match.
 */

import type {
	AccountHolding,
	AccountMeasuredSummary,
	AccountSessionMeasure,
	AccountStatus,
	MeasuredAmount,
	MeasuredGain,
	MeasuredGainMark,
	MeasuredSessionRow,
	MeasuredTotals,
	TrackedSessionFigures,
} from '$lib/api/commands.gen';

export function exact(ped: number): MeasuredAmount {
	return { ped, precision: 'exact' };
}

export function wholePec(ped: number): MeasuredAmount {
	return { ped, precision: 'wholePec' };
}

export function carried(name: string): AccountHolding {
	return { name, place: { kind: 'carried' } };
}

export const SPAN = { from: '2026-11-15T14:31:00Z', to: '2026-11-15T14:32:00Z' };

/** A gained line: what arrived of one holding. */
export function gain(
	name: string,
	amount: MeasuredAmount,
	mark: MeasuredGainMark = 'arrived',
): MeasuredGain {
	return { item: carried(name), role: 'material', stacking: 'stack', mark, units: 1, amount };
}

export function measure(overrides: Partial<AccountSessionMeasure> = {}): AccountSessionMeasure {
	return {
		sessionId: 's1',
		sealed: true,
		coverage: {
			level: 'complete',
			opening: 'read',
			closing: 'read',
			readingCount: 60,
			gapCount: 0,
		},
		ammo: [],
		decay: [],
		repairs: [],
		otherConsumption: [],
		unexplainedDepartures: [],
		gained: [],
		gainedTotal: exact(0),
		gainedPossiblyStrongbox: exact(0),
		events: [],
		totals: totals(),
		...overrides,
	};
}

export function totals(overrides: Partial<MeasuredTotals> = {}): MeasuredTotals {
	return {
		ammo: { kind: 'settled', burned: exact(0) },
		weaponDecay: exact(0),
		healingDecay: exact(0),
		armourDecay: exact(0),
		otherEquipmentDecay: exact(0),
		otherConsumption: exact(0),
		unexplainedDepartures: exact(0),
		repairCost: exact(0),
		repairCostDerived: exact(0),
		repairsUncosted: 0,
		gained: exact(0),
		gainedPossiblyStrongbox: exact(0),
		...overrides,
	};
}

export function tracked(overrides: Partial<TrackedSessionFigures> = {}): TrackedSessionFigures {
	return {
		weaponCost: 0,
		healingCost: 0,
		armourCost: 0,
		armourUnrecorded: false,
		lootTt: 0,
		totalCycled: 0,
		...overrides,
	};
}

export function row(overrides: Partial<MeasuredSessionRow> = {}): MeasuredSessionRow {
	return {
		id: 's1',
		name: 'Atrox hunt',
		startTime: '2026-11-15T13:00:00Z',
		endTime: '2026-11-15T14:30:00Z',
		coverage: 'complete',
		readingCount: 90,
		gapCount: 0,
		sealed: true,
		exclusion: null,
		measured: totals(),
		tracked: tracked(),
		...overrides,
	};
}

export function status(overrides: Partial<AccountStatus> = {}): AccountStatus {
	return {
		state: 'disconnected',
		signal: 'off',
		connectedAt: null,
		lastReadingAt: null,
		readingSession: false,
		hasConnected: false,
		confirming: null,
		signingIn: false,
		hasReadings: false,
		failingSince: null,
		...overrides,
	};
}

export function summary(overrides: Partial<AccountMeasuredSummary> = {}): AccountMeasuredSummary {
	const none = { sessions: 0, tracked: 0, measured: exact(0) };
	return {
		weapon: none,
		healing: none,
		armour: none,
		other: { sessions: 0, measured: exact(0) },
		loot: none,
		included: 0,
		leftOut: [
			{ reason: 'partialCoverage', sessions: 0 },
			{ reason: 'excluded', sessions: 0 },
			{ reason: 'notSealed', sessions: 0 },
		],
		...overrides,
	};
}
