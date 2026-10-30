import { describe, expect, it } from 'vitest';
import type { AppSettings, EquipmentSummary } from '$lib/api/commands.gen';
import { declarableKindLabel, declarableOptions } from './declareOptions';

function item(id: number, name: string, type: EquipmentSummary['type']): EquipmentSummary {
	return { id: String(id), name, type } as unknown as EquipmentSummary;
}

function settings(
	hotbar: Record<string, number | null>,
	carriedWeaponIds: number[] = [],
	parked: Record<string, number | null> = {},
): AppSettings {
	return {
		hotbar,
		activeHotbarPreset: 1,
		hotbarPresets: [hotbar, parked, {}],
		carriedWeaponIds,
	} as unknown as AppSettings;
}

const library = [
	item(1, 'Opalo', 'weapon'),
	item(2, 'Sollomate Rubio', 'weapon'),
	item(3, 'Unused Rifle', 'weapon'),
	item(4, 'FAP', 'healing'),
	item(5, 'Adjusted FAP', 'healing'),
	item(6, 'Snack', 'consumable'),
	item(7, 'Ziplex', 'tool'),
];

describe('declarableOptions', () => {
	it('offers carried weapons, every healer and tool, never consumables, in kind then key order', () => {
		const options = declarableOptions(
			library,
			settings({ '1': 2, '2': 5, '3': 6, '0': 1 }, [], { '4': 1 }),
		);
		expect(options.map((option) => [option.name, option.kind, option.hotbarSlot])).toEqual([
			['Sollomate Rubio', 'weapon', '1'],
			['Opalo', 'weapon', '0'],
			['Adjusted FAP', 'healing', '2'],
			['FAP', 'healing', null],
			['Ziplex', 'tool', null],
		]);
	});

	it('counts weapons carried without a key and on parked presets as carried', () => {
		const options = declarableOptions(library, settings({}, [3], { '5': 2 }));
		expect(
			options.filter((option) => option.kind === 'weapon').map((option) => option.name),
		).toEqual(['Sollomate Rubio', 'Unused Rifle']);
	});

	it('offers every weapon when none is carried yet', () => {
		const options = declarableOptions(library, settings({}));
		expect(options.filter((option) => option.kind === 'weapon')).toHaveLength(3);
	});

	it('labels each kind for the menu', () => {
		expect(declarableKindLabel('weapon')).toBe('Weapon');
		expect(declarableKindLabel('healing')).toBe('Healer');
		expect(declarableKindLabel('tool')).toBe('Tool');
	});
});
