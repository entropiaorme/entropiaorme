import { describe, expect, it } from 'vitest';
import type { Equipment } from '$lib/types';
import {
	enabledCarried,
	entryLabel,
	hotbarReadingNote,
	isHoldable,
	lookalikeNotes,
	slotIdleWhileReading,
} from './toolReading';

function entry(id: string, name: string, overrides: Partial<Equipment> = {}): Equipment {
	return {
		id,
		name,
		type: 'weapon',
		enabled: true,
		amplifierName: null,
		costPerUse: 1,
		damageMin: 1,
		damageMax: 2,
		reloadSeconds: 1,
		isLimited: false,
		enrichmentLevel: 1,
		healingProfile: null,
		lifestealPercent: null,
		effectProfile: null,
		consumable: null,
		...overrides,
	};
}

describe('isHoldable', () => {
	it('is every kind the bar can name, and never a consumable', () => {
		expect(isHoldable('weapon')).toBe(true);
		expect(isHoldable('healing')).toBe(true);
		expect(isHoldable('tool')).toBe(true);
		expect(isHoldable('consumable')).toBe(false);
	});
});

describe('lookalikeNotes', () => {
	const library = [
		entry('1', 'Pistol', { amplifierName: 'A101' }),
		entry('2', 'Pistol'),
		entry('3', 'Rifle'),
		entry('4', 'Long Rifle Mk1', { enabled: false }),
		entry('5', 'Long Rifle Mk2', { enabled: false }),
	];
	// As the library read serves them: one label per member of a group.
	const labels = { '1': 'Pistol + A101', '2': 'Pistol' };

	it('names the others on each member as the player tells them apart', () => {
		const notes = lookalikeNotes(library, [['1', '2']], labels);
		expect(notes.get('1')?.text).toBe("Looks the same as Pistol on the game's bar.");
		expect(notes.get('2')?.text).toBe("Looks the same as Pistol + A101 on the game's bar.");
		expect(notes.has('3')).toBe(false);
	});

	it('asks to leave one on only while more than one member is on', () => {
		expect(lookalikeNotes(library, [['1', '2']], labels).get('1')?.leaveOne).toBe(true);
		expect(lookalikeNotes(library, [['4', '5']], {}).get('4')?.leaveOne).toBe(false);
		const oneOn = [entry('1', 'Pistol'), entry('2', 'Pistol', { enabled: false })];
		expect(lookalikeNotes(oneOn, [['1', '2']], labels).get('2')?.leaveOne).toBe(false);
	});

	it('lists three or more in plain English, by name where no label is served', () => {
		const notes = lookalikeNotes(library, [['3', '4', '5']], {});
		expect(notes.get('3')?.text).toBe(
			"Looks the same as Long Rifle Mk1 and Long Rifle Mk2 on the game's bar.",
		);
	});

	it('skips ids the library does not hold, and a group left with one member', () => {
		expect(lookalikeNotes(library, [['3', '99']], {}).size).toBe(0);
		expect(lookalikeNotes([], [['1', '2']], labels).size).toBe(0);
	});
});

describe('entryLabel', () => {
	it('is the label served for the entry, else its name', () => {
		expect(entryLabel({ '1': 'Pistol + A101' }, entry('1', 'Pistol'))).toBe('Pistol + A101');
		expect(entryLabel({}, entry('2', 'Pistol'))).toBe('Pistol');
	});
});

describe('enabledCarried', () => {
	it('carries every weapon that is on, on no key', () => {
		const carried = enabledCarried([
			entry('1', 'Pistol'),
			entry('2', 'Rifle', { enabled: false }),
			entry('3', 'FAP', { type: 'healing' }),
		]);
		expect(carried.map((c) => c.weapon.id)).toEqual(['1']);
		expect(carried[0].slot).toBeNull();
		expect(carried[0].parked).toBeNull();
	});
});

describe('slotIdleWhileReading', () => {
	it('is idle for a weapon or harvesting tool, and live for a consumable or healing tool', () => {
		expect(slotIdleWhileReading('weapon')).toBe(true);
		expect(slotIdleWhileReading('tool')).toBe(true);
		expect(slotIdleWhileReading('consumable')).toBe(false);
		expect(slotIdleWhileReading('healing')).toBe(false);
		expect(slotIdleWhileReading(null)).toBe(false);
	});
});

describe('hotbarReadingNote', () => {
	it('says what the keys still do, as the machine reads them', () => {
		expect(hotbarReadingNote({ keyless: false, hotbarHooksEnabled: true })).toBe(
			'The held tool is read from the screen. Hotbar keys only start consumables and count heals.',
		);
		expect(hotbarReadingNote({ keyless: false, hotbarHooksEnabled: false })).toMatch(
			/once the hotbar key listener is on in Settings\.$/,
		);
		expect(hotbarReadingNote({ keyless: true, hotbarHooksEnabled: true })).toMatch(
			/start consumables from the overlay\.$/,
		);
	});
});
