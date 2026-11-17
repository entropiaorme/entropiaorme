/**
 * Wire-shaped minute by minute fixtures: every list present, every figure
 * carrying its precision, and only shapes the backend can produce (a
 * stretch names steps the series has; an arrival in a settled step).
 */

import type {
	AccountSessionMinutes,
	ArrivalDisposition,
	MinuteArrival,
	MinuteFigure,
	MinuteMissingLoot,
	MinuteStretch,
	SessionMinute,
} from '$lib/api/commands.gen';
import { exact, wholePec } from './measure';

/** The span of the step at `index`: a minute each, from 14:40 UTC. */
export function stepSpan(index: number): { from: string; to: string } {
	return {
		from: new Date(Date.UTC(2026, 10, 15, 14, 40 + index)).toISOString(),
		to: new Date(Date.UTC(2026, 10, 15, 14, 41 + index)).toISOString(),
	};
}

export function measuredFigure(
	ped: number,
	precision: 'exact' | 'wholePec' = 'exact',
): MinuteFigure {
	return { kind: 'measured', amount: precision === 'exact' ? exact(ped) : wholePec(ped) };
}

export function step(index: number, overrides: Partial<SessionMinute> = {}): SessionMinute {
	return {
		readingId: 100 + index,
		previousReadingId: 99 + index,
		between: stepSpan(index),
		gap: false,
		settled: true,
		looted: 0,
		arrived: exact(0),
		arrivedNotLooted: [],
		lootedNeverArrived: [],
		weapon: { measured: measuredFigure(0), tracked: 0 },
		healing: { measured: measuredFigure(0), tracked: 0 },
		armour: measuredFigure(0),
		...overrides,
	};
}

export function open(proposalId = 7, possiblyRecorded = false): ArrivalDisposition {
	return { state: 'open', proposalId, possiblyRecorded };
}

export function arrival(itemName: string, overrides: Partial<MinuteArrival> = {}): MinuteArrival {
	return {
		itemName,
		arrivedIn: { kind: 'carried' },
		units: 1,
		tt: wholePec(5),
		boughtAtTt: false,
		disposition: open(),
		...overrides,
	};
}

/** Loot flagged as never arrived in the step at `index`. */
export function missing(
	itemName: string,
	index: number,
	overrides: Partial<MinuteMissingLoot> = {},
): MinuteMissingLoot {
	return {
		itemName,
		units: 3,
		tt: 0.3,
		// Looted in the step before the one that flags it: the site may run a
		// step late, so loot is missing only once the next step has passed.
		lootedIn: stepSpan(index - 1),
		lines: [{ source: 'kill', id: 31 }],
		correction: { kind: 'offered', sessionId: 's1', itemName },
		...overrides,
	};
}

export function stretch(
	firstStep: number,
	lastStep: number,
	measured: number,
	tracked: number,
	drift: number,
): MinuteStretch {
	return {
		firstStep,
		lastStep,
		between: { from: stepSpan(firstStep).from, to: stepSpan(lastStep).to },
		measured: exact(measured),
		tracked,
		drift,
	};
}

export function minutes(overrides: Partial<AccountSessionMinutes> = {}): AccountSessionMinutes {
	const steps = overrides.steps ?? [];
	return {
		sessionId: 's1',
		running: false,
		coverage: {
			complete: true,
			opening: 'read',
			closing: 'read',
			readingCount: steps.length + 1,
			gapCount: 0,
		},
		steps,
		weaponStretches: [],
		healingStretches: [],
		lootUnconfirmed: 0,
		...overrides,
	};
}
