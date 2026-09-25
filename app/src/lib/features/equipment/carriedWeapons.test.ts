import { describe, expect, it } from 'vitest';
import type { Equipment } from '$lib/types';
import {
	addableWeapons,
	axisTicks,
	CRITICAL_REACH,
	carriedWeapons,
	formatRange,
	sharedRanges,
	weaponBands,
} from './carriedWeapons';

function item(
	id: string,
	name: string,
	type: Equipment['type'],
	min: number | null,
	max: number | null,
): Equipment {
	return {
		id,
		name,
		type,
		amplifierName: null,
		costPerUse: 1,
		damageMin: min,
		damageMax: max,
		reloadSeconds: null,
		isLimited: false,
		enrichmentLevel: 0,
		healingProfile: null,
		lifestealPercent: null,
	} as Equipment;
}

const library = [
	item('1', 'Pistol', 'weapon', 5, 10),
	item('2', 'Cannon', 'weapon', 20, 40),
	item('3', 'Rifle', 'weapon', 8, 16),
	item('4', 'FAP', 'healing', null, null),
	item('5', 'Mystery', 'weapon', null, null),
];

describe('the carried weapons', () => {
	it('lists the hotbar weapons in slot order, then the carried ones, each once', () => {
		const carried = carriedWeapons(library, { '0': 1, '2': 4, '3': 2, '9': null }, [3, 2, 1, 99]);
		expect(carried.map((c) => [c.weapon.name, c.slot])).toEqual([
			['Cannon', '3'],
			['Pistol', '0'],
			['Rifle', null],
		]);
	});

	it('offers every other weapon for carrying, by name', () => {
		expect(addableWeapons(library, { '1': 1 }, [3]).map((w) => w.name)).toEqual([
			'Cannon',
			'Mystery',
		]);
		expect(addableWeapons([], {}, [])).toEqual([]);
	});

	it('splits weapons with a damage figure from those without', () => {
		const { banded, bandless } = weaponBands(carriedWeapons(library, { '1': 1 }, [5]));
		expect(banded).toEqual([
			{ id: '1', name: 'Pistol', min: 5, max: 10, critMax: 10 * CRITICAL_REACH, slot: '1' },
		]);
		expect(bandless.map((w) => w.name)).toEqual(['Mystery']);
	});

	it('names every pair of overlapping regular bands and where they meet', () => {
		const { banded } = weaponBands(carriedWeapons(library, { '1': 1, '2': 2, '3': 3 }, []));
		expect(sharedRanges(banded)).toEqual([{ first: 'Pistol', second: 'Rifle', min: 8, max: 10 }]);
		// Touching bands share their edge.
		const touching = weaponBands(
			carriedWeapons(
				[item('a', 'A', 'weapon', 5, 10), item('b', 'B', 'weapon', 10, 20)],
				{ '1': 'a' as never, '2': 'b' as never },
				[],
			),
		).banded;
		expect(sharedRanges(touching)).toEqual([{ first: 'A', second: 'B', min: 10, max: 10 }]);
	});

	it('formats ranges and axis ticks for reading', () => {
		expect(formatRange(5, 10.25)).toBe('5.0–10.3');
		expect(axisTicks(120)).toEqual([0, 50, 100, 150]);
		expect(axisTicks(30)).toEqual([0, 10, 20, 30]);
		expect(axisTicks(0)).toEqual([0]);
	});
});
