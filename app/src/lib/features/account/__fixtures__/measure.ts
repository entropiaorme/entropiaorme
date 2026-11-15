/**
 * Wire-shaped account fixtures for the account feature's tests: every list
 * field present, every amount carrying its precision, as the commands
 * answer.
 */

import type {
	AccountHolding,
	AccountSessionMeasure,
	AccountStatus,
	MeasuredAmount,
	MeasuredSessionRow,
	MeasuredTotals,
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
		repairCost: wholePec(0),
		repairCostDerived: wholePec(0),
		repairsUncosted: 0,
		gained: exact(0),
		gainedPossiblyStrongbox: exact(0),
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
		tracked: { weaponCost: 0, healingCost: 0, armourCost: 0, lootTt: 0, totalCycled: 0 },
		...overrides,
	};
}

export function status(overrides: Partial<AccountStatus> = {}): AccountStatus {
	return {
		state: 'disconnected',
		connectedAt: null,
		lastReadingAt: null,
		readingSession: false,
		hasConnected: false,
		...overrides,
	};
}
