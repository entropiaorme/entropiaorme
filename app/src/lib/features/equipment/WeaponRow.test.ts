// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { equipmentDemoDetails, equipmentDemoLibrary } from '$lib/guide/fixtures/equipment';
import type { EquipmentDetail } from '$lib/types';
import type { LibraryModel } from './libraryModel.svelte';
import WeaponRow from './WeaponRow.svelte';

const item = equipmentDemoLibrary[0];

/** The row reads only the expansion and the detail cache from its model. */
function expandedWith(detail: EquipmentDetail): LibraryModel {
	return {
		expandedId: item.id,
		detailCache: { [item.id]: detail },
		toggleExpand: () => {},
	} as unknown as LibraryModel;
}

describe('the weapon detail attack rate', () => {
	it('states the catalogue rate within the server limit', () => {
		render(WeaponRow, { props: { model: expandedWith(equipmentDemoDetails['1']), item } });
		expect(screen.getByTestId('weapon-attack-rate').textContent).toContain('46 a minute');
		expect(screen.queryByText(/damage per PEC is unchanged/)).toBeNull();
	});

	it('explains the per-attack factor when the limit holds the weapon back', () => {
		const detail: EquipmentDetail = {
			...equipmentDemoDetails['1'],
			attackRate: {
				basePerMinute: 90,
				reloadSpeedPercent: 15,
				buffedPerMinute: 103.5,
				effectivePerMinute: 100,
				factor: 1.035,
			},
		};
		render(WeaponRow, { props: { model: expandedWith(detail), item } });
		expect(screen.getByTestId('weapon-attack-rate').textContent).toContain(
			'100 a minute, server limit',
		);
		expect(
			screen.getByText(
				'The 103.5 a minute this weapon would reach become ×1.035 damage and cost per attack; damage per PEC is unchanged.',
			),
		).toBeTruthy();
	});

	it('shows no attack-rate line for a weapon without a catalogue rate', () => {
		render(WeaponRow, {
			props: { model: expandedWith({ ...equipmentDemoDetails['1'], attackRate: null }), item },
		});
		expect(screen.queryByTestId('weapon-attack-rate')).toBeNull();
	});
});
