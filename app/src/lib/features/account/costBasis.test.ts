import { describe, expect, it } from 'vitest';
import type { CostBasis, CostBreakdown, CostReason } from '$lib/api/commands.gen';
import { tracked } from './__fixtures__/measure';
import {
	BASIS_MARKS,
	bucketMark,
	bucketReason,
	COST_REASONS,
	readingsSpeak,
	restsOnReadings,
	sessionBasisLine,
	showsBucket,
	trackedEstimate,
} from './costBasis';
import { trackedFromRow } from './measure';

const NO_OVERHEAD_REASONS = { travel: null, fieldLogistics: null, unclassified: null };

const BASES: readonly CostBasis[] = ['measured', 'tracked', 'mixed'];

function breakdown(overrides: Partial<CostBreakdown> = {}): CostBreakdown {
	const none = {
		weaponCost: 0,
		healCost: 0,
		consumableCost: 0,
		enhancerCost: 0,
		armourCost: 0,
		harvestCost: 0,
	};
	return {
		...none,
		tracked: { ...none },
		basis: {
			weaponCost: 'tracked',
			healCost: 'tracked',
			consumableCost: 'tracked',
			enhancerCost: 'tracked',
			armourCost: 'tracked',
			harvestCost: 'tracked',
		},
		reasons: {
			weaponCost: 'noMeasuredCosts',
			healCost: 'noMeasuredCosts',
			consumableCost: 'noMeasuredCosts',
			enhancerCost: 'noMeasuredCosts',
			armourCost: 'noMeasuredCosts',
			harvestCost: 'noMeasuredCosts',
		},
		overheadCost: 0,
		overhead: { travel: 0, fieldLogistics: 0, unclassified: 0, reasons: NO_OVERHEAD_REASONS },
		measured: null,
		armourUnrecorded: false,
		...overrides,
	};
}

describe('the basis marks', () => {
	it('gives every basis a label and a plain meaning', () => {
		for (const basis of BASES) {
			expect(BASIS_MARKS[basis].label.length).toBeGreaterThan(0);
			expect(BASIS_MARKS[basis].meaning).toMatch(/\.$/);
		}
		expect(BASIS_MARKS.measured.label).toBe('from your account');
		expect(BASIS_MARKS.tracked.label).toBe('tracked');
		expect(BASIS_MARKS.mixed.label).toBe('partly from your account');
		// Armour is tracked from recordings, not the chat log: the meaning
		// names no one source the app tracks from.
		expect(BASIS_MARKS.tracked.meaning).not.toMatch(/chat log/);
		expect(BASIS_MARKS.mixed.meaning).not.toMatch(/chat log/);
	});

	it('marks a bucket only in a session that mixes bases, when no reason speaks of readings', () => {
		for (const bucket of BASES) {
			expect(bucketMark('mixed', bucket)).toBe(bucket);
			expect(bucketMark('measured', bucket)).toBeNull();
			expect(bucketMark('tracked', bucket)).toBeNull();
			expect(bucketMark('tracked', bucket, 'noMeasuredCosts')).toBeNull();
		}
	});

	it('marks a bucket the readings missed in a session that has readings, so its reason can be read', () => {
		expect(bucketMark('tracked', 'tracked', 'partialCoverage')).toBe('tracked');
		expect(bucketMark('measured', 'mixed', 'limitedSetUnconfirmed')).toBe('mixed');
		// A measured bucket carries no reason, and so no mark of its own.
		expect(bucketMark('measured', 'measured', null)).toBeNull();
	});

	it('says once that a session measured throughout is all from the account', () => {
		expect(sessionBasisLine('measured')).toBe('All from your account');
		expect(sessionBasisLine('mixed')).toBeNull();
		expect(sessionBasisLine('tracked')).toBeNull();
	});

	it('takes a session to rest on the readings when any of its costs does', () => {
		expect(restsOnReadings('measured')).toBe(true);
		expect(restsOnReadings('mixed')).toBe(true);
		expect(restsOnReadings('tracked')).toBe(false);
	});
});

describe('the tracked estimate kept beside a measured figure', () => {
	it('is kept where the readings measured the bucket and the estimate differed', () => {
		const costs = breakdown({
			weaponCost: 123.41,
			tracked: { ...breakdown().tracked, weaponCost: 123.91 },
			basis: { ...breakdown().basis, weaponCost: 'measured' },
		});
		expect(trackedEstimate(costs, 'weaponCost')).toBe(123.91);
	});

	it('is kept for a bucket measured in part', () => {
		const costs = breakdown({
			armourCost: 0.75,
			tracked: { ...breakdown().tracked, armourCost: 0.5 },
			basis: { ...breakdown().basis, armourCost: 'mixed' },
		});
		expect(trackedEstimate(costs, 'armourCost')).toBe(0.5);
	});

	it('is dropped where the two agree within a PEC', () => {
		const costs = breakdown({
			healCost: 1,
			tracked: { ...breakdown().tracked, healCost: 1.01 },
			basis: { ...breakdown().basis, healCost: 'measured' },
		});
		expect(trackedEstimate(costs, 'healCost')).toBeNull();
	});

	it('is never set beside a tracked figure, which is the estimate itself', () => {
		const costs = breakdown({
			healCost: 1,
			tracked: { ...breakdown().tracked, healCost: 2 },
		});
		expect(trackedEstimate(costs, 'healCost')).toBeNull();
	});
});

describe('which buckets are shown', () => {
	it('shows a bucket that holds something, whatever it rests on', () => {
		expect(showsBucket(breakdown({ healCost: 1 }), 'healCost')).toBe(true);
	});

	it('shows a measured nothing against a tracked estimate of something', () => {
		const costs = breakdown({
			healCost: 0,
			tracked: { ...breakdown().tracked, healCost: 1.5 },
			basis: { ...breakdown().basis, healCost: 'measured' },
		});
		expect(showsBucket(costs, 'healCost')).toBe(true);
		expect(trackedEstimate(costs, 'healCost')).toBe(1.5);
	});

	it('hides a bucket that holds nothing and differs from nothing', () => {
		expect(showsBucket(breakdown(), 'healCost')).toBe(false);
		const tracked = breakdown({ tracked: { ...breakdown().tracked, healCost: 1.5 } });
		expect(showsBucket(tracked, 'healCost')).toBe(false);
	});
});

describe('why a bucket was not measured', () => {
	// The wire's reasons, spelled out: adding one to the wire without a
	// sentence here fails the type-check on the table, and this list keeps
	// the table from carrying a reason the wire never gives.
	const WIRE_REASONS: readonly CostReason[] = [
		'noMeasuredCosts',
		'partialCoverage',
		'gap',
		'ammoUnsettled',
		'ammoToppedUp',
		'limitedSetUnconfirmed',
		'trackedOnly',
		'playInGap',
		'chatLogMissesGap',
		'chatLogUnchecked',
		'gapUnaccounted',
		'overheadInGap',
	];

	it('gives every reason the wire names one plain sentence, and no other', () => {
		expect(Object.keys(COST_REASONS).sort()).toEqual([...WIRE_REASONS].sort());
		for (const reason of WIRE_REASONS) {
			const sentence = COST_REASONS[reason];
			expect(sentence, reason).toMatch(/^[A-Z].*\.$/);
			expect(sentence, reason).not.toMatch(/\u2014/);
			expect(sentence, reason).not.toMatch(/[a-z][A-Z]/);
		}
	});

	it("reads a bucket's reason where the readings missed it in a session that has readings", () => {
		const costs = breakdown({
			reasons: { ...breakdown().reasons, healCost: 'gap' },
			basis: { ...breakdown().basis, weaponCost: 'measured' },
		});
		expect(bucketReason(costs, 'healCost')).toBe(COST_REASONS.gap);
	});

	it('takes the readings to speak to a session they measured, or said why they could not', () => {
		expect(readingsSpeak(breakdown())).toBe(false);
		expect(
			readingsSpeak(breakdown({ reasons: { ...breakdown().reasons, weaponCost: 'gap' } })),
		).toBe(true);
		expect(
			readingsSpeak(breakdown({ basis: { ...breakdown().basis, healCost: 'measured' } })),
		).toBe(true);
	});

	it('reads none for a measured bucket, or a session the readings never measured', () => {
		const measured = breakdown({
			basis: { ...breakdown().basis, weaponCost: 'measured' },
			reasons: { ...breakdown().reasons, weaponCost: null },
		});
		expect(bucketReason(measured, 'weaponCost')).toBeNull();
		expect(bucketReason(breakdown(), 'healCost')).toBeNull();
	});
});

describe('armour whose hits no recording covers', () => {
	const unrecorded = breakdown({
		armourCost: 0.75,
		tracked: { ...breakdown().tracked, armourCost: 0.5 },
		basis: { ...breakdown().basis, armourCost: 'measured' },
		armourUnrecorded: true,
	});

	it('sets no tracked estimate beside the measured armour', () => {
		expect(trackedEstimate(unrecorded, 'armourCost')).toBeNull();
		expect(trackedEstimate({ ...unrecorded, armourUnrecorded: false }, 'armourCost')).toBe(0.5);
	});

	it('reads one session alike in its detail and in the review', () => {
		const inReview = trackedFromRow(
			tracked({ armourCost: 0.5, armourUnrecorded: true, weaponCost: 0, lootTt: 0 }),
		);
		expect(inReview.armour).toBeNull();
		expect(trackedEstimate(unrecorded, 'armourCost')).toBe(inReview.armour);
	});
});
