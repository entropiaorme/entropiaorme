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
	markWeaponShotEffectTick: vi.fn(),
	undoWeaponAssignment: vi.fn(),
	getWeaponBulkOptions: vi.fn(),
	assignFittingWeaponShots: vi.fn(),
	undoWeaponBatch: vi.fn(),
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
			markedTicks: 0,
			effectTicks: 0,
			pricedTicks: 0,
			unclaimedTicks: 0,
			undeclaredShots: 0,
			ranUndeclared: false,
			batches: [],
			unpricedCostBound: 0.2,
			unpricedNoFitShots: 0,
			effects: [],
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
		group: 'unresolved',
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
		effectCandidates: [],
		effectWindowId: null,
		correctionId: null,
		correctionKind: null,
		correctionWindowId: null,
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
			'1 shot unpriced, up to +0.20 PED.',
		);
		expect(screen.queryByTestId('weapon-undeclared')).toBeNull();
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
		await fireEvent.click(within(row).getByRole('button', { name: /^Assign the shot at/ }));
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
		await fireEvent.click(within(rows[0]).getByRole('button', { name: /^Undo the correction/ }));
		expect(mocked.undoWeaponAssignment).toHaveBeenCalledWith('c1');
		expect(within(rows[1]).queryByRole('button')).toBeNull();
	});

	it('lists the effects over time with their ticks and where each was paid', () => {
		mount(
			detail({
				effectTicks: 22,
				unclaimedTicks: 1,
				effects: [
					{
						id: 'w1',
						toolName: 'Electrocution',
						activatedAt: 1000,
						expiresAt: 1025,
						hitAmount: 129.2,
						costPerShot: 4.8732,
						paidHere: true,
						withdrawn: false,
						ticks: 21,
						tickDamage: 1183.8,
					},
					{
						id: 'w0',
						toolName: 'Electrocution',
						activatedAt: 900,
						expiresAt: 925,
						hitAmount: 131,
						costPerShot: 4.8732,
						paidHere: false,
						withdrawn: false,
						ticks: 1,
						tickDamage: 50,
					},
					{
						id: 'w2',
						toolName: 'Electrocution',
						activatedAt: 1100,
						expiresAt: 1125,
						hitAmount: 130,
						costPerShot: 4.8732,
						paidHere: true,
						withdrawn: true,
						ticks: 0,
						tickDamage: 0,
					},
				],
			}),
		);
		const rows = screen.getAllByTestId('weapon-effect');
		expect(rows).toHaveLength(3);
		expect(within(rows[0]).getByText('4.87 PED')).toBeTruthy();
		expect(within(rows[0]).getByText(/^21 ticks · /)).toBeTruthy();
		expect(within(rows[1]).getByText('Paid earlier')).toBeTruthy();
		expect(within(rows[2]).getByText('Taken back')).toBeTruthy();
		expect(screen.getByTestId('weapon-unclaimed-ticks').textContent).toContain(
			'1 tick could not be tied to one cast.',
		);
		// Ticks join the reviewable shots.
		expect(screen.getByRole('button', { name: 'Review shots (26)' })).toBeTruthy();
	});

	it('still accounts for ticks whose cast is no longer listed', () => {
		mount(detail({ effectTicks: 2, unclaimedTicks: 2 }));
		expect(screen.queryAllByTestId('weapon-effect')).toHaveLength(0);
		expect(screen.getByTestId('weapon-unclaimed-ticks').textContent).toContain(
			'2 ticks could not be tied to one cast.',
		);
	});

	it('marks an unresolved hit as a tick of an effect open when it landed', async () => {
		const cast = { windowId: 'w1', toolName: 'Electrocution', activatedAt: 1000, standing: true };
		const takenBack = {
			windowId: 'w0',
			toolName: 'Electrocution',
			activatedAt: 990,
			standing: false,
		};
		mocked.getWeaponShots.mockResolvedValue({
			shots: [shot({ effectCandidates: [takenBack, cast] })],
			total: 2,
		});
		mocked.getWeaponCorrectionWeapons.mockResolvedValue([
			{ equipmentId: 2, name: 'Cannon', costPerShotPed: 0.2, fits: true },
		]);
		mocked.markWeaponShotEffectTick.mockResolvedValue(detail({ unpriced: 0, markedTicks: 1 }));
		mount(detail());
		await fireEvent.click(screen.getByRole('button', { name: 'Review shots (4)' }));
		const row = await screen.findByTestId('weapon-shot');
		await fireEvent.click(within(row).getByRole('button', { name: /^Assign the shot at/ }));
		const items = await screen.findAllByRole('menuitem');
		expect(items[0].getAttribute('aria-label')).toMatch(
			/^A tick of Electrocution, cast .*, no cost$/,
		);
		expect(items[1].getAttribute('aria-label')).toBe('Cannon, fits · 0.20 PED');
		await fireEvent.click(items[0]);
		expect(mocked.markWeaponShotEffectTick).toHaveBeenCalledWith('e1', 'w1');
	});

	it('shows a tick as costing nothing, and offers to price it as a shot', async () => {
		const cast = { windowId: 'w1', toolName: 'Electrocution', activatedAt: 1000, standing: true };
		mocked.getWeaponShots.mockResolvedValue({
			shots: [
				shot({
					group: 'effect_tick',
					effectCandidates: [cast],
					effectWindowId: 'w1',
				}),
				shot({
					id: 'e2',
					effectCandidates: [cast],
					correctionId: 'c9',
					correctionKind: 'effect_tick',
					correctionWindowId: 'w1',
					correctable: false,
				}),
			],
			total: 2,
		});
		mocked.getWeaponCorrectionWeapons.mockResolvedValue([
			{ equipmentId: 2, name: 'Cannon', costPerShotPed: 0.2, fits: false },
		]);
		mocked.undoWeaponAssignment.mockResolvedValue(detail());
		mount(detail({ effectTicks: 3 }));
		await fireEvent.click(screen.getByRole('button', { name: 'Review shots (7)' }));
		const rows = await screen.findAllByTestId('weapon-shot');
		expect(within(rows[0]).getByText('Tick · no cost')).toBeTruthy();
		await fireEvent.click(
			within(rows[0]).getByRole('button', { name: /^Price the tick at .* as a paid shot$/ }),
		);
		const items = await screen.findAllByRole('menuitem');
		expect(items.map((item) => item.getAttribute('aria-label'))).toEqual(['Cannon · 0.20 PED']);
		// A hit marked as a tick names it, and can be undone.
		expect(within(rows[1]).getByText('Tick of Electrocution')).toBeTruthy();
		await fireEvent.click(within(rows[1]).getByRole('button', { name: /^Undo the correction/ }));
		expect(mocked.undoWeaponAssignment).toHaveBeenCalledWith('c9');
	});

	it('switches to the shots that overrode the hotbar', async () => {
		mocked.getWeaponShots.mockResolvedValue({ shots: [shot()], total: 2 });
		mount(detail());
		await fireEvent.click(screen.getByRole('button', { name: 'Review shots (4)' }));
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Differed from the hotbar: 2' }),
		);
		expect(mocked.getWeaponShots).toHaveBeenLastCalledWith('s1', 'evidence', 0, 50);
	});
});

describe("assigning a session's unpriced shots at once", () => {
	it('states what it will do, then assigns every shot the weapon fits', async () => {
		mocked.getWeaponBulkOptions.mockResolvedValue([
			{ equipmentId: 2, name: 'Cannon', fittingShots: 3, skippedShots: 1, addedCostPed: 0.6 },
		]);
		mocked.assignFittingWeaponShots.mockResolvedValue({
			assigned: 3,
			skipped: 1,
			addedCostPed: 0.6,
			detail: detail({ unpriced: 1 }),
		});
		mount(detail({ unpriced: 4, unpricedCostBound: 0.8, unpricedNoFitShots: 1 }));
		expect(screen.getByTestId('weapon-unpriced').textContent).toContain(
			'4 shots unpriced, up to +0.80 PED. 1 fit no carried weapon.',
		);

		await fireEvent.click(screen.getByTestId('weapon-bulk-trigger'));
		await fireEvent.click(await screen.findByLabelText('Cannon: fits 3, adds 0.60 PED'));
		const confirm = screen.getByTestId('weapon-bulk-confirm');
		expect(confirm.textContent).toContain(
			'Assign 3 shots to Cannon, skip 1 shot it did not fit: +0.60 PED.',
		);
		expect(confirm.textContent).toContain('Priced as Cannon is configured now.');
		expect(mocked.assignFittingWeaponShots).not.toHaveBeenCalled();

		await fireEvent.click(within(confirm).getByText('Assign'));
		expect(mocked.assignFittingWeaponShots).toHaveBeenCalledWith('s1', 2);
		expect((await screen.findByTestId('weapon-bulk-notice')).textContent).toBe(
			'Assigned 3 shots to Cannon; 1 shot left unpriced.',
		);
	});

	it('changes nothing when cancelled', async () => {
		mocked.getWeaponBulkOptions.mockResolvedValue([
			{ equipmentId: 2, name: 'Cannon', fittingShots: 1, skippedShots: 0, addedCostPed: 0.2 },
		]);
		mount(detail());
		await fireEvent.click(screen.getByTestId('weapon-bulk-trigger'));
		await fireEvent.click(await screen.findByLabelText('Cannon: fits 1, adds 0.20 PED'));
		await fireEvent.click(within(screen.getByTestId('weapon-bulk-confirm')).getByText('Cancel'));
		expect(screen.queryByTestId('weapon-bulk-confirm')).toBeNull();
		expect(mocked.assignFittingWeaponShots).not.toHaveBeenCalled();
	});

	it('records that the session ran undeclared and undoes a batch as one', async () => {
		mocked.undoWeaponBatch.mockResolvedValue(detail({ batches: [] }));
		mount(
			detail({
				ranUndeclared: true,
				undeclaredShots: 30,
				batches: [{ id: 'b1', toolName: 'Rifle', shots: 24, costPed: 1.2, correctedAt: 1000 }],
			}),
		);
		expect(screen.getByTestId('weapon-undeclared').textContent).toContain(
			'No weapon was declared for 30 of its shots.',
		);
		const batch = screen.getByTestId('weapon-batch');
		expect(within(batch).getByText('24 shots assigned to Rifle')).toBeTruthy();
		expect(within(batch).getByText('+1.20 PED')).toBeTruthy();
		await fireEvent.click(screen.getByLabelText('Undo assigning 24 shots assigned to Rifle'));
		expect(mocked.undoWeaponBatch).toHaveBeenCalledWith('b1');
	});

	it('offers no bulk assignment while the session runs', () => {
		mount(detail({ correctable: false }));
		expect(screen.queryByTestId('weapon-bulk-trigger')).toBeNull();
	});
});
