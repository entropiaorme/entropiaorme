import { describe, expect, it } from 'vitest';
import type { ConsumableDose } from '$lib/api';
import {
	describeDoseCost,
	describeEffect,
	describeEffects,
	describeRun,
	describeSource,
	doseState,
	formatCountdown,
	formatDuration,
	liveRows,
	nextBoundary,
	remainingSeconds,
} from './doses';

const reload = (percent: number) => ({
	name: 'Reload Speed Increased',
	strength: percent,
	unit: '%',
	reloadSpeedPercent: percent,
});

function dose(overrides: Partial<ConsumableDose> = {}): ConsumableDose {
	return {
		id: 'd1',
		equipmentId: 40,
		itemName: 'Nanobots - Adrenaline Boost',
		source: 'manual',
		sessionId: 's1',
		startedAt: 1000,
		endsAt: 4600,
		untimed: false,
		replaced: false,
		costPed: 4.5,
		costTracked: true,
		effects: [reload(10)],
		removedAt: null,
		removedBy: null,
		...overrides,
	};
}

describe('dose countdowns', () => {
	it('run until the stored end and never go negative', () => {
		expect(doseState(dose(), 4599)).toBe('running');
		expect(doseState(dose(), 4600)).toBe('ended');
		expect(doseState(dose({ removedAt: 2000 }), 1500)).toBe('removed');
		expect(remainingSeconds(dose(), 4599.2)).toBe(1);
		expect(remainingSeconds(dose(), 5000)).toBe(0);
	});

	it('never end on their own for an untimed dose, which has nothing to count', () => {
		const untimed = dose({ untimed: true, endsAt: null, costPed: 0 });
		expect(doseState(untimed, 1e12)).toBe('running');
		expect(remainingSeconds(untimed, 2000)).toBeNull();
		expect(doseState({ ...untimed, endsAt: 3000 }, 3000)).toBe('ended');
	});

	it('print hours only past an hour', () => {
		expect(formatCountdown(3599)).toBe('59:59');
		expect(formatCountdown(3600)).toBe('1:00:00');
		expect(formatCountdown(65)).toBe('1:05');
		expect(formatCountdown(-3)).toBe('0:00');
	});

	it('name a dose length in words, zero as immediate', () => {
		expect(formatDuration(0)).toBe('Instant');
		expect(formatDuration(8)).toBe('8 s');
		expect(formatDuration(600)).toBe('10 min');
		expect(formatDuration(3600)).toBe('1 h');
		expect(formatDuration(5400)).toBe('1 h 30 min');
	});
});

describe('the live readout', () => {
	const now = 2000;
	const running = dose({ id: 'a', endsAt: 3000 });
	const sooner = dose({ id: 'b', equipmentId: 41, endsAt: 2500 });
	const justEnded = dose({ id: 'c', equipmentId: 42, endsAt: 1990 });
	const longEnded = dose({ id: 'd', equipmentId: 43, endsAt: 1900 });
	const removed = dose({ id: 'e', removedAt: 1500 });
	const buff = dose({ id: 'f', equipmentId: 9, source: 'on_use', endsAt: 2005 });

	it('lists running doses soonest first, then the just ended', () => {
		const rows = liveRows([running, justEnded, longEnded, sooner, removed], now);
		expect(rows.map((row) => row.id)).toEqual(['b', 'a', 'c']);
	});

	it('lists an untimed dose after the ones counting down', () => {
		const untimed = dose({ id: 'u', equipmentId: 44, untimed: true, endsAt: null });
		const rows = liveRows([untimed, running, sooner], now);
		expect(rows.map((row) => row.id)).toEqual(['b', 'a', 'u']);
	});

	it('leaves out a heal buff, which the player does not take', () => {
		expect(liveRows([buff], now)).toEqual([]);
	});

	it('knows when the next row changes', () => {
		expect(nextBoundary([running, sooner], now)).toBe(2500);
		expect(nextBoundary([justEnded], now)).toBe(2050);
		expect(nextBoundary([longEnded], 3000)).toBeNull();
		expect(nextBoundary([dose({ untimed: true, endsAt: null })], now)).toBeNull();
	});
});

describe('wording', () => {
	it('puts reload speed first and prints the rest as the item does', () => {
		expect(describeEffect(reload(-4))).toBe('Reload speed -4%');
		expect(
			describeEffects([
				{ name: 'Critical Chance Added', strength: 1, unit: '%', reloadSpeedPercent: null },
				reload(10),
				{ name: 'Health Added', strength: 50, unit: 'HP', reloadSpeedPercent: null },
			]),
		).toBe('Reload speed +10%, Critical Chance Added 1%, Health Added 50 HP');
	});

	it('says why a dose booked nothing', () => {
		expect(describeDoseCost(dose())).toBe('4.50 PED');
		expect(describeDoseCost(dose({ costPed: 0, costTracked: false }))).toBe('cost not tracked');
		expect(describeDoseCost(dose({ costPed: 0, sessionId: null }))).toBe('outside a session');
		expect(describeDoseCost(dose({ costPed: 0, source: 'on_use' }))).toBe('paid with the heal');
		expect(describeDoseCost(dose({ costPed: 0, untimed: true }))).toBe('taken before the session');
	});

	it('says how long an effect ran, and that an untimed one runs until ended', () => {
		expect(describeRun(dose())).toBe('1 h');
		expect(describeRun(dose({ endsAt: 1600, replaced: true }))).toBe(
			'10 min (replaced by a re-dose)',
		);
		expect(describeRun(dose({ untimed: true, endsAt: null }))).toBe('Until ended');
		expect(describeRun(dose({ untimed: true, endsAt: 2800 }))).toBe('30 min');
		expect(describeSource(dose({ untimed: true }))).toBe('Already in effect');
		expect(describeSource(dose())).toBe('Started by hand');
	});
});
