// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { WeaponAttributionSummary, WeaponShot } from '$lib/api/weapons';
import type { SessionDetail } from '$lib/types/tracking';
import WeaponEvidence from './WeaponEvidence.svelte';
import { createWeaponReviewModel } from './weaponReviewModel.svelte';

vi.mock('$lib/api', () => ({
	assignWeaponShot: vi.fn(),
	getWeaponCorrectionWeapons: vi.fn(),
	getWeaponShots: vi.fn(),
	undoWeaponAssignment: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function detail(overrides: Partial<WeaponAttributionSummary> = {}): SessionDetail {
	return {
		sessionId: 's1',
		weaponAttribution: {
			correctable: true,
			agreed: 40,
			evidenced: 3,
			evidenceShots: 2,
			unresolved: 2,
			unpriced: 1,
			assigned: 1,
			effectTicks: 0,
			reviews: [
				{
					id: 'r1',
					decision: 'confirmed',
					hotbarTool: 'Pistol',
					evidenceTool: 'Cannon',
					since: 900,
					decidedAt: 1000,
					repricedShots: 2,
					costDelta: 0.3,
				},
			],
			...overrides,
		},
	} as SessionDetail;
}

function shot(overrides: Partial<WeaponShot> = {}): WeaponShot {
	return {
		id: 'e1',
		observedAt: 1100,
		amount: 35,
		critical: true,
		reason: 'fits several carried weapons',
		hotbarTool: 'Pistol',
		toolName: null,
		costPerShot: 0,
		candidates: [
			{ equipmentId: 2, name: 'Cannon', fits: true },
			{ equipmentId: 3, name: 'Carbine', fits: true },
		],
		correctionId: null,
		reviewDecision: null,
		correctable: true,
		...overrides,
	};
}

function mount(initial: SessionDetail) {
	let current = initial;
	const model = createWeaponReviewModel({
		detail: () => current,
		apply: (fresh) => {
			current = fresh;
		},
	});
	return render(WeaponEvidence, { props: { model } });
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('weapon evidence', () => {
	it('tallies the shots, discloses the unpriced ones, and lists the decisions', () => {
		mount(detail());
		expect(
			screen.getByText('40 matched the hotbar · 3 by damage range · 2 unresolved'),
		).toBeTruthy();
		expect(screen.getByTestId('weapon-unpriced').textContent).toContain(
			'1 shot could not be priced to one weapon.',
		);
		const review = screen.getByTestId('weapon-review');
		expect(
			within(review).getByText('Switched to Cannon from Pistol; 2 shots repriced'),
		).toBeTruthy();
		expect(within(review).getByText('+0.30 PED')).toBeTruthy();
	});

	it('assigns an unpriced shot to a chosen weapon, fitting weapons first', async () => {
		mocked.getWeaponShots.mockResolvedValue({ shots: [shot()], total: 2 });
		mocked.getWeaponCorrectionWeapons.mockResolvedValue([
			{ equipmentId: 2, name: 'Cannon', costPerShotPed: 0.2, fits: true },
			{ equipmentId: 1, name: 'Pistol', costPerShotPed: 0.05, fits: false },
		]);
		mocked.assignWeaponShot.mockResolvedValue(detail({ unpriced: 0 }));
		mount(detail());

		await fireEvent.click(screen.getByRole('button', { name: 'Review shots (4)' }));
		const row = await screen.findByTestId('weapon-shot');
		expect(within(row).getByText('Fits Cannon and Carbine (hotbar: Pistol)')).toBeTruthy();
		expect(within(row).getByText('Unpriced')).toBeTruthy();
		expect(within(row).getByText('crit')).toBeTruthy();
		await fireEvent.click(within(row).getByRole('button', { name: /to a weapon$/ }));
		const items = await screen.findAllByRole('menuitem');
		expect(items.map((item) => item.getAttribute('aria-label'))).toEqual([
			'Cannon, fits · 0.20 PED',
			'Pistol · 0.05 PED',
		]);
		await fireEvent.click(items[0]);
		expect(mocked.assignWeaponShot).toHaveBeenCalledWith('e1', 2);
	});

	it('offers an assigned shot back with Undo, and no action on a running session', async () => {
		mocked.getWeaponShots.mockResolvedValue({
			shots: [
				shot({ toolName: 'Cannon', costPerShot: 0.2, correctionId: 'c1', correctable: false }),
				shot({ id: 'e2', correctable: false }),
			],
			total: 2,
		});
		mocked.undoWeaponAssignment.mockResolvedValue(detail());
		mount(detail());
		await fireEvent.click(screen.getByRole('button', { name: 'Review shots (4)' }));
		const rows = await screen.findAllByTestId('weapon-shot');
		expect(within(rows[0]).getByText('Cannon')).toBeTruthy();
		await fireEvent.click(within(rows[0]).getByRole('button', { name: /^Undo the assignment/ }));
		expect(mocked.undoWeaponAssignment).toHaveBeenCalledWith('c1');
		expect(within(rows[1]).queryByRole('button')).toBeNull();
	});

	it('switches to the shots that overrode the hotbar', async () => {
		mocked.getWeaponShots.mockResolvedValue({ shots: [shot()], total: 2 });
		mount(detail());
		await fireEvent.click(screen.getByRole('button', { name: 'Review shots (4)' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Overrode the hotbar: 2' }));
		expect(mocked.getWeaponShots).toHaveBeenLastCalledWith('s1', 'evidence', 0, 50);
	});
});
