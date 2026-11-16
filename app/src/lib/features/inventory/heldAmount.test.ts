import { describe, expect, it } from 'vitest';
import {
	amountEquivalent,
	canEnterByPed,
	heldInMode,
	pedForUnits,
	unitsForPed,
} from './heldAmount';

describe('held amounts', () => {
	it('enters by PED any item with TT, however little, and none without', () => {
		expect(canEnterByPed(0.0001)).toBe(true);
		expect(canEnterByPed(2.5)).toBe(true);
		expect(canEnterByPed(0)).toBe(false);
		expect(canEnterByPed(Number.NaN)).toBe(false);
	});

	it('turns PED of Shrapnel into whole units, and units back into PED', () => {
		expect(unitsForPed(1, 0.0001)).toBe(10_000);
		expect(unitsForPed(0.00026, 0.0001)).toBe(3);
		expect(pedForUnits(10_000, 0.0001)).toBe(1);
		expect(pedForUnits(30, 0.0001)).toBe(0.003);
	});

	it('takes PED naming the whole holding as exactly all of it', () => {
		// An average unit TT and a fractional held count both survive the trip.
		expect(unitsForPed(10.03, 10.03 / 10_000, 10_000)).toBe(10_000);
		expect(unitsForPed(12.5, 1, 12.5)).toBe(12.5);
		expect(unitsForPed(5, 10.03 / 10_000, 10_000)).toBe(4985);
	});

	it('comes to nothing for an empty, negative, or unenterable amount', () => {
		expect(unitsForPed(0, 0.0001)).toBe(0);
		expect(unitsForPed(-1, 0.0001)).toBe(0);
		expect(unitsForPed(5, 0)).toBe(0);
	});

	it('reads the amount in the other form, and the holding in this one', () => {
		expect(amountEquivalent(10_000, 0.0001, 'units')).toBe('1.00 PED');
		expect(amountEquivalent(30, 0.0001, 'units')).toBe('0.0030 PED');
		expect(amountEquivalent(1, 0.0001, 'ped')).toBe('1 unit');
		expect(amountEquivalent(4985, 0.0001, 'ped')).toBe('4985 units');
		expect(amountEquivalent(12, 0, 'units')).toBeNull();
		expect(amountEquivalent(null, 0.0001, 'units')).toBeNull();
		expect(heldInMode(5000, 0.0001, 'ped')).toBe('0.50 PED');
		expect(heldInMode(5000, 0.0001, 'units')).toBe('5000');
		expect(heldInMode(12, 0, 'ped')).toBe('12');
	});
});
