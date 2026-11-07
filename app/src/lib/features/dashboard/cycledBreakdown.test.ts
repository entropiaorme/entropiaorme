import { describe, expect, it } from 'vitest';
import type { CycledLine } from '$lib/api';
import { groupCycledBreakdown, hasArmourLine } from './cycledBreakdown';

const lines: CycledLine[] = [
	{ bucket: 'healing', label: 'Vivo T10', costPed: 3 },
	{ bucket: 'weapon', label: 'Opalo', costPed: 20 },
	{ bucket: 'armour', label: 'Armour repairs', costPed: 2 },
	{ bucket: 'weapon', label: 'Sollomate Rubio', costPed: 70 },
	{ bucket: 'consumable', label: 'Reload Speed Pill', costPed: 5 },
];

describe('groupCycledBreakdown', () => {
	it('orders buckets the way spend is read, dearest item first', () => {
		const groups = groupCycledBreakdown(lines, 100);
		expect(groups.map((group) => group.bucket)).toEqual([
			'weapon',
			'healing',
			'armour',
			'consumable',
		]);
		expect(groups[0].items.map((item) => item.label)).toEqual(['Sollomate Rubio', 'Opalo']);
		expect(groups[0].subtotalPed).toBe(90);
	});

	it('states each item as a share of the whole Cycled', () => {
		const [weapons] = groupCycledBreakdown(lines, 100);
		expect(weapons.items[0].sharePct).toBe(70);
		expect(weapons.items[1].sharePct).toBe(20);
	});

	it('leaves the groups summing to the Cycled they break down', () => {
		const groups = groupCycledBreakdown(lines, 100);
		expect(groups.reduce((sum, group) => sum + group.subtotalPed, 0)).toBe(100);
	});

	it('claims no share of an empty Cycled', () => {
		const [weapons] = groupCycledBreakdown([{ bucket: 'weapon', label: 'Opalo', costPed: 0 }], 0);
		expect(weapons.items[0].sharePct).toBe(0);
	});

	it('knows whether armour has been recorded yet', () => {
		expect(hasArmourLine(lines)).toBe(true);
		expect(hasArmourLine(lines.filter((line) => line.bucket !== 'armour'))).toBe(false);
	});
});
