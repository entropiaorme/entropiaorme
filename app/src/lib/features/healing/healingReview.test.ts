import { describe, expect, it } from 'vitest';
import type { HealingActivationRow, HealingOutput, HealingSessionSummary } from '$lib/api/healing';
import {
	activationStanding,
	describeOutput,
	evidenceTally,
	formatHeal,
	reviewFilters,
	toolOption,
	uncostedCount,
} from './healingReview';

function summary(overrides: Partial<HealingSessionSummary> = {}): HealingSessionSummary {
	return {
		correctable: true,
		activations: [],
		activationCount: 0,
		outputCount: 0,
		directOutputs: 0,
		effectOutputs: 0,
		passiveOutputs: 0,
		unattributedOutputs: 0,
		...overrides,
	};
}

function activation(overrides: Partial<HealingActivationRow> = {}): HealingActivationRow {
	return {
		id: 'a1',
		toolName: 'FAP',
		observedAt: 1000,
		cost: 0.03,
		provenance: 'direct',
		effectUntil: null,
		outputCount: 1,
		amount: 80,
		superseded: false,
		correction: null,
		...overrides,
	};
}

function output(overrides: Partial<HealingOutput> = {}): HealingOutput {
	return {
		id: 'o1',
		observedAt: 1000,
		amount: 10,
		classification: 'unattributed',
		reason: 'no compatible paid-healer activation',
		toolName: null,
		correction: null,
		correctable: true,
		...overrides,
	};
}

describe('activation standing', () => {
	it('reads a live use as paid, a taken-back one as not paid, and a minted one as corrected', () => {
		expect(activationStanding(activation())).toBe('paid');
		expect(
			activationStanding(
				activation({ superseded: true, correction: { id: 'c', kind: 'notPaidUse' } }),
			),
		).toBe('notPaid');
		expect(
			activationStanding(
				activation({ provenance: 'corrected', correction: { id: 'c', kind: 'paidUse' } }),
			),
		).toBe('corrected');
	});
});

describe('the review groups', () => {
	it('offers only groups with heals in them, unresolved first', () => {
		const groups = reviewFilters(summary({ unattributedOutputs: 2, effectOutputs: 5 }));
		expect(groups.map((group) => [group.id, group.count])).toEqual([
			['unattributed', 2],
			['effect', 5],
		]);
		expect(reviewFilters(summary())).toEqual([]);
		expect(
			uncostedCount(summary({ unattributedOutputs: 2, effectOutputs: 5, passiveOutputs: 1 })),
		).toBe(8);
	});
});

describe('the tally', () => {
	it('never counts ticks as uses and leaves out empty groups', () => {
		expect(evidenceTally(summary({ activationCount: 1, effectOutputs: 12 }))).toBe(
			'1 paid use · 12 effect ticks',
		);
		expect(
			evidenceTally(
				summary({
					activationCount: 2,
					effectOutputs: 1,
					passiveOutputs: 3,
					unattributedOutputs: 4,
				}),
			),
		).toBe('2 paid uses · 1 effect tick · 3 lifesteal · 4 unresolved');
		expect(evidenceTally(summary())).toBe('0 paid uses');
	});
});

describe('describing a heal', () => {
	it('speaks in the player’s terms rather than the tracker’s reasons', () => {
		expect(describeOutput(output())).toBe('No paid use explains it');
		expect(describeOutput(output({ classification: 'passive' }))).toBe('Came with damage dealt');
		expect(describeOutput(output({ classification: 'effect', toolName: 'Restoration Chip' }))).toBe(
			'Tick of Restoration Chip',
		);
		expect(describeOutput(output({ classification: 'effect', toolName: null }))).toBe(
			'Tick of more than one running effect',
		);
		expect(describeOutput(output({ correction: { id: 'c', kind: 'notPaidUse' } }))).toBe(
			'Its paid use was taken back',
		);
		expect(
			describeOutput(
				output({ classification: 'effect', correction: { id: 'c', kind: 'paidUse' } }),
			),
		).toBe('Tick of a use you marked as paid');
	});

	it('formats heal amounts in HP without spurious decimals', () => {
		expect(formatHeal(80)).toBe('80 HP');
		expect(formatHeal(12.5)).toBe('12.5 HP');
		expect(formatHeal(9.96)).toBe('10 HP');
	});

	it('labels an offered item with its per-use cost', () => {
		expect(
			toolOption({ equipmentId: 1, name: 'FAP', costPerUsePed: 0.03, fits: true }, (value) =>
				value.toFixed(2),
			),
		).toBe('FAP · 0.03 PED');
	});
});
