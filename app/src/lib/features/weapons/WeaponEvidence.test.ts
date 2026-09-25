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
		await fireEvent.click(await screen.findByRole('button', { name: 'Overrode the hotbar: 2' }));
		expect(mocked.getWeaponShots).toHaveBeenLastCalledWith('s1', 'evidence', 0, 50);
	});
});
