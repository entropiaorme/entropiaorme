// @vitest-environment happy-dom

import { render, screen, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import type { Equipment } from '$lib/types';
import type { WeaponBand } from './carriedWeapons';
import DamageRanges from './DamageRanges.svelte';

const band = (
	id: string,
	name: string,
	min: number,
	max: number,
	slot: string | null,
): WeaponBand => ({
	id,
	name,
	min,
	max,
	critMax: max * 3,
	slot,
	tick: null,
});

describe('damage ranges', () => {
	it('lists each weapon with its slot, in a table the chart stands in for', () => {
		render(DamageRanges, {
			props: {
				banded: [band('1', 'Pistol', 5, 10, '1'), band('3', 'Rifle', 8, 16, null)],
				bandless: [],
			},
		});
		const table = screen.getByRole('table');
		const rows = within(table).getAllByRole('row');
		const cells = (row: HTMLElement, index: number) =>
			within(row)
				.getAllByRole(index === 0 ? 'columnheader' : 'cell')
				.map((cell) => cell.textContent?.trim());
		expect(rows.map(cells)).toEqual([
			['Weapon', 'Hotbar slot', 'Hit', 'Critical reach', 'Effect tick'],
			['Pistol', '1', '5.0–10.0', 'up to 30.0', '\u2014'],
			['Rifle', 'None', '8.0–16.0', 'up to 48.0', '\u2014'],
		]);
		expect(screen.getByTitle('Carried without a hotkey')).toBeTruthy();
	});

	it('says where two weapons can only be told apart by the hotbar', () => {
		render(DamageRanges, {
			props: {
				banded: [band('1', 'Pistol', 5, 10, '1'), band('3', 'Rifle', 8, 16, null)],
				bandless: [],
			},
		});
		expect(screen.getByTestId('shared-ranges').textContent?.replace(/\s+/g, ' ')).toContain(
			'Pistol and Rifle share 8.0\u201310.0',
		);
	});

	it("bands an effect's ticks and says where they meet another weapon's hits", () => {
		render(DamageRanges, {
			props: {
				banded: [
					band('1', 'Cannon', 20, 40, '1'),
					{ ...band('2', 'Zapper', 100, 160, '2'), tick: { min: 35, max: 75 } },
				],
				bandless: [],
			},
		});
		const zapper = within(screen.getByRole('table'))
			.getAllByRole('row')
			.find((row) => row.textContent?.includes('Zapper'));
		expect(zapper?.textContent).toContain('35.0–75.0');
		expect(screen.getByText('Effect tick', { selector: 'span' })).toBeTruthy();
		expect(screen.getByTestId('effect-overlaps').textContent?.replace(/\s+/g, ' ')).toContain(
			"Zapper's ticks and Cannon's hits share 35.0\u201340.0: while the effect runs, a hit there with Cannon on your hotbar is left unpriced.",
		);
	});

	it('explains a weapon with no damage figure, and an empty set', () => {
		const { unmount } = render(DamageRanges, {
			props: { banded: [], bandless: [{ id: '5', name: 'Mystery' } as Equipment] },
		});
		expect(screen.getByText(/Weapons on the hotbar or carried without a hotkey/)).toBeTruthy();
		expect(screen.getByText(/No damage figure for Mystery/)).toBeTruthy();
		unmount();
		render(DamageRanges, { props: { banded: [band('1', 'Pistol', 5, 10, '1')], bandless: [] } });
		expect(screen.queryByTestId('shared-ranges')).toBeNull();
	});
});
