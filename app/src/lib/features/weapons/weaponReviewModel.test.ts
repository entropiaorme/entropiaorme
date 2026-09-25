import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { WeaponAttributionSummary, WeaponShot } from '$lib/api/weapons';
import type { SessionDetail } from '$lib/types/tracking';
import { createWeaponReviewModel, SHOT_PAGE } from './weaponReviewModel.svelte';

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
			agreed: 10,
			evidenced: 0,
			evidenceShots: 0,
			unresolved: 3,
			unpriced: 3,
			assigned: 0,
			effectTicks: 0,
			reviews: [],
			...overrides,
		},
	} as SessionDetail;
}

function shot(id: string): WeaponShot {
	return {
		id,
		observedAt: 1,
		amount: 30,
		critical: false,
		reason: 'r',
		hotbarTool: null,
		toolName: null,
		costPerShot: 0,
		candidates: [],
		correctionId: null,
		reviewDecision: null,
		correctable: true,
	};
}

function harness(initial = detail()) {
	let current = initial;
	const model = createWeaponReviewModel({
		detail: () => current,
		apply: (fresh) => {
			current = fresh;
		},
	});
	return { model, current: () => current };
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('the weapon review model', () => {
	it('applies an assignment’s refreshed detail and re-reads the open list in place', async () => {
		const { model, current } = harness();
		mocked.getWeaponShots.mockResolvedValue({ shots: [shot('a'), shot('b')], total: 3 });
		await model.toggleReview();
		expect(mocked.getWeaponShots).toHaveBeenLastCalledWith('s1', 'unresolved', 0, SHOT_PAGE);
		expect(model.hasMore).toBe(true);

		const fresh = detail({ unpriced: 2, assigned: 1 });
		mocked.assignWeaponShot.mockResolvedValue(fresh);
		await model.assign('a', 7);
		expect(mocked.assignWeaponShot).toHaveBeenCalledWith('a', 7);
		expect(current()).toBe(fresh);
		expect(mocked.getWeaponShots).toHaveBeenLastCalledWith('s1', 'unresolved', 0, SHOT_PAGE);
		expect(model.busy).toBeNull();
	});

	it('pages on demand and settles on a group that has shots', async () => {
		const { model } = harness(detail({ unresolved: 0, evidenceShots: 2 }));
		mocked.getWeaponShots.mockResolvedValue({ shots: [shot('a')], total: 2 });
		await model.toggleReview();
		expect(model.group).toBe('evidence');
		mocked.getWeaponShots.mockResolvedValue({ shots: [shot('b')], total: 2 });
		await model.loadMore();
		expect(mocked.getWeaponShots).toHaveBeenLastCalledWith('s1', 'evidence', 1, SHOT_PAGE);
		expect(model.shots.map((s) => s.id)).toEqual(['a', 'b']);
		expect(model.hasMore).toBe(false);
	});

	it('reports a failed assignment and a failed page without losing state', async () => {
		const { model } = harness();
		mocked.getWeaponShots.mockRejectedValueOnce(new Error('offline'));
		await model.toggleReview();
		expect(model.shotsError).toBe('offline');
		mocked.assignWeaponShot.mockRejectedValue(
			new Error('Stop the session before assigning its shots'),
		);
		await model.assign('a', 1);
		expect(model.error).toBe('Stop the session before assigning its shots');
		model.dismissError();
		expect(model.error).toBeNull();
	});

	it('loads the offered weapons once per shot, retrying after a failure', async () => {
		const { model } = harness();
		mocked.getWeaponCorrectionWeapons.mockRejectedValueOnce(new Error('nope'));
		await model.loadWeapons('a');
		expect(model.weaponsFor('a')).toEqual({ status: 'error', message: 'nope' });
		mocked.getWeaponCorrectionWeapons.mockResolvedValue([
			{ equipmentId: 1, name: 'Pistol', costPerShotPed: 0.05, fits: true },
		]);
		await model.loadWeapons('a');
		await model.loadWeapons('a');
		expect(mocked.getWeaponCorrectionWeapons).toHaveBeenCalledTimes(2);
		expect(model.weaponsFor('a')?.status).toBe('ready');
	});
});
