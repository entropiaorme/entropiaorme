import { describe, expect, it } from 'vitest';
import { BC30, bc30AgainstGauss, comparison, GAUSS, setup } from './__fixtures__/comparison';
import {
	buildChart,
	describeUnmodelled,
	formatKillPec,
	niceRangeTicks,
	niceTicks,
	setupLabels,
} from './comparisonView';

describe('setupLabels', () => {
	it('names each setup by weapon and amplifier, and numbers identical ones', () => {
		const amped = setup(1, BC30, {
			components: [
				{ role: 'weapon', catalogId: 'bc', name: BC30, isLimited: true, markupPercent: 1800 },
				{
					role: 'amplifier',
					catalogId: 'a106',
					name: 'Omegaton A106',
					isLimited: false,
					markupPercent: 100,
				},
			],
		});
		expect(setupLabels(comparison({ setups: [setup(0, BC30), amped] }))).toEqual([
			BC30,
			`${BC30} + Omegaton A106`,
		]);
		expect(
			setupLabels(comparison({ setups: [setup(0, BC30), setup(1, GAUSS), setup(2, BC30)] })),
		).toEqual([`${BC30} (1)`, GAUSS, `${BC30} (2)`]);
	});
});

describe('describeUnmodelled', () => {
	it('says why a setup cannot be priced', () => {
		expect(describeUnmodelled(setup(0, BC30))).toBeNull();
		expect(describeUnmodelled(setup(0, 'Mystery', { unmodelled: 'missing_efficiency' }))).toBe(
			'A weapon or amplifier has no catalogue Efficiency, so its return cannot be modelled.',
		);
	});
});

describe('buildChart', () => {
	it('draws each setup in its colour and marks where the cheapest changes', () => {
		const chart = buildChart(bc30AgainstGauss(), [3, 5]);
		expect(chart?.hpMax).toBe(1000);
		expect(chart?.series.map((series) => [series.index, series.slot, series.label])).toEqual([
			[0, 3, BC30],
			[1, 5, GAUSS],
		]);
		expect(chart?.markers.map((marker) => marker.hp)).toEqual([35.9, 43.9, 95.2]);
		// On the line taking over, between its 1 and 250 HP samples.
		expect(chart?.markers[2].loss).toBeCloseTo(0.7 + 0.0112 * 95.2, 9);
		expect(chart?.lossMax).toBeCloseTo(0.25 + 16, 9);
	});

	it('leaves off an unpriced setup and one no longer listed', () => {
		const unpriced = comparison({
			setups: [setup(0, 'Mystery', { curve: [], unmodelled: 'no_damage' }), setup(1, GAUSS)],
		});
		expect(buildChart(unpriced, [1, 2])?.series.map((series) => series.index)).toEqual([1]);
		expect(buildChart(bc30AgainstGauss(), [1, null])?.series.map((series) => series.index)).toEqual(
			[0],
		);
		expect(buildChart(bc30AgainstGauss(), [null, null])).toBeNull();
	});
});

describe('formatting', () => {
	it('formats per-kill PEC for reading and picks round ticks', () => {
		expect(formatKillPec(1.8454)).toBe('1.85');
		expect(formatKillPec(0.0172)).toBe('0.017');
		expect(formatKillPec(12_345.6)).toBe('12,346');
		expect(niceTicks(2.62)).toEqual([0, 1, 2, 3]);
		expect(niceTicks(37)).toEqual([0, 10, 20, 30, 40]);
		expect(niceTicks(0)).toEqual([0]);
	});

	it('ticks a zoomed range at round values inside it', () => {
		expect(niceRangeTicks(36, 44)).toEqual([36, 38, 40, 42, 44]);
		expect(niceRangeTicks(1.13, 1.97)).toEqual([1.25, 1.5, 1.75]);
		expect(niceRangeTicks(5, 5)).toEqual([5]);
	});
});
