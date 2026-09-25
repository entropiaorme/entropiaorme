import { describe, expect, it } from 'vitest';
import type { WeaponAttributionSummary, WeaponShot } from '$lib/api/weapons';
import {
	attributionTally,
	describeReview,
	describeShot,
	formatDamage,
	hasWeaponEvidence,
	reviewGroups,
	shotStanding,
	weaponOption,
} from './weaponReview';

function summary(overrides: Partial<WeaponAttributionSummary> = {}): WeaponAttributionSummary {
	return {
		correctable: true,
		agreed: 398,
		evidenced: 10,
		evidenceShots: 6,
		unresolved: 4,
		unpriced: 3,
		assigned: 1,
		effectTicks: 0,
		reviews: [],
		...overrides,
	};
}

function shot(overrides: Partial<WeaponShot> = {}): WeaponShot {
	return {
		id: 'e1',
		observedAt: 1000,
		amount: 30,
		critical: false,
		reason: 'r',
		hotbarTool: 'Pistol',
		toolName: null,
		costPerShot: 0,
		candidates: [
			{ equipmentId: 1, name: 'Pistol', fits: false },
			{ equipmentId: 2, name: 'Cannon', fits: true },
			{ equipmentId: 3, name: 'Carbine', fits: true },
		],
		correctionId: null,
		reviewDecision: null,
		correctable: true,
		...overrides,
	};
}

describe('the attribution summary', () => {
	it('tallies how shots were priced', () => {
		expect(attributionTally(summary())).toBe(
			'398 matched the hotbar · 10 by damage range · 4 unresolved',
		);
		expect(attributionTally(summary({ agreed: null, evidenced: null, effectTicks: 2 }))).toBe(
			'4 unresolved · 2 effect ticks',
		);
	});

	it('knows when there is nothing to show', () => {
		expect(hasWeaponEvidence(summary())).toBe(true);
		expect(
			hasWeaponEvidence(
				summary({
					agreed: null,
					evidenced: 0,
					evidenceShots: 0,
					unresolved: 0,
					unpriced: 0,
					assigned: 0,
				}),
			),
		).toBe(false);
		expect(
			hasWeaponEvidence(summary({ agreed: 0, evidenced: 0, evidenceShots: 0, unresolved: 0 })),
		).toBe(false);
	});

	it('offers only the groups with shots in them, unresolved first', () => {
		expect(reviewGroups(summary()).map((g) => g.id)).toEqual(['unresolved', 'evidence']);
		expect(reviewGroups(summary({ unresolved: 0 })).map((g) => g.id)).toEqual(['evidence']);
	});
});

describe('a stored shot', () => {
	it('says what the tracker knew about it', () => {
		expect(describeShot(shot())).toBe('Fits Cannon and Carbine (hotbar: Pistol)');
		expect(describeShot(shot({ candidates: [], hotbarTool: null }))).toBe(
			'Fits no carried weapon (no hotbar press)',
		);
		expect(describeShot(shot({ amount: null }))).toBe(
			'A jam, dodge, or evade with no weapon known (hotbar: Pistol)',
		);
		expect(
			describeShot(shot({ candidates: [{ equipmentId: 2, name: 'Cannon', fits: true }] })),
		).toBe('Fits only Cannon (hotbar: Pistol)');
		expect(describeShot(shot({ reviewDecision: 'kept', toolName: 'Pistol' }))).toBe(
			'Fit Cannon, Carbine; you kept Pistol',
		);
	});

	it('says where its price stands', () => {
		expect(shotStanding(shot())).toEqual({ kind: 'unpriced' });
		expect(
			shotStanding(shot({ toolName: 'Cannon', costPerShot: 0.2, correctionId: 'c1' })),
		).toEqual({
			kind: 'assigned',
			tool: 'Cannon',
			cost: 0.2,
			correctionId: 'c1',
		});
		expect(
			shotStanding(shot({ toolName: 'Cannon', costPerShot: 0.2, reviewDecision: 'confirmed' })),
		).toEqual({
			kind: 'confirmed',
			tool: 'Cannon',
			cost: 0.2,
		});
		expect(shotStanding(shot({ toolName: 'Cannon', costPerShot: 0.2 }))).toEqual({
			kind: 'priced',
			tool: 'Cannon',
			cost: 0.2,
		});
	});

	it('formats damage as the game prints it', () => {
		expect(formatDamage(30)).toBe('30.0');
		expect(formatDamage(12.345)).toBe('12.3');
	});
});

describe('a decision and an offered weapon', () => {
	it('reads as a sentence', () => {
		const review = {
			id: 'r',
			decision: 'confirmed' as const,
			hotbarTool: 'Pistol',
			evidenceTool: 'Cannon',
			since: 1,
			decidedAt: 2,
			repricedShots: 1,
			costDelta: 0.15,
		};
		expect(describeReview(review)).toBe('Switched to Cannon from Pistol; 1 shot repriced');
		expect(describeReview({ ...review, decision: 'kept', repricedShots: 3 })).toBe(
			'Kept Pistol over Cannon; 3 shots repriced',
		);
		expect(
			weaponOption({ equipmentId: 2, name: 'Cannon', costPerShotPed: 0.2, fits: true }, (v) =>
				v.toFixed(2),
			),
		).toBe('Cannon, fits · 0.20 PED');
	});
});
