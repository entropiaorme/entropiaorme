import { describe, expect, it } from 'vitest';
import { groupTicks, overlapsAny, spreadLabels } from './sliderTicks';

describe('groupTicks', () => {
	it('orders ticks along the track and merges those that read the same', () => {
		const groups = groupTicks(
			[
				{ value: 40, label: 'mean', title: 'mean 40.0' },
				{ value: 61, label: 'A', title: 'Animal 61.0' },
				{ value: 29.96, label: 'M', title: 'Mutant 30.0' },
				{ value: 30.04, label: 'R', title: 'Robot 30.0' },
			],
			1,
		);
		expect(groups).toEqual([
			{ value: 29.96, label: 'M · R', title: 'Mutant 30.0, Robot 30.0' },
			{ value: 40, label: 'mean', title: 'mean 40.0' },
			{ value: 61, label: 'A', title: 'Animal 61.0' },
		]);
	});

	it('keeps ticks apart that differ at the shown precision', () => {
		const ticks = [
			{ value: 30, label: 'A', title: 'A' },
			{ value: 30.1, label: 'R', title: 'R' },
		];
		expect(groupTicks(ticks, 1)).toHaveLength(2);
		expect(groupTicks(ticks, 0)).toHaveLength(1);
	});
});

describe('spreadLabels', () => {
	it('centres labels under their ticks when there is room', () => {
		expect(
			spreadLabels(
				[
					{ centre: 50, width: 10 },
					{ centre: 150, width: 20 },
				],
				300,
			),
		).toEqual([45, 140]);
	});

	it('pushes colliding labels apart by the gap, keeping their order', () => {
		// Given out of track order; the lefts come back in the order given.
		const lefts = spreadLabels(
			[
				{ centre: 102, width: 10 },
				{ centre: 100, width: 10 },
				{ centre: 101, width: 30 },
			],
			300,
			4,
		);
		expect(lefts).toEqual([143, 95, 109]);
	});

	it('keeps labels inside the row at both ends', () => {
		expect(spreadLabels([{ centre: 0, width: 20 }], 200)).toEqual([0]);
		expect(
			spreadLabels(
				[
					{ centre: 196, width: 10 },
					{ centre: 200, width: 10 },
				],
				200,
				4,
			),
		).toEqual([176, 190]);
	});

	it('overlaps rather than leave a row too narrow for every label', () => {
		const lefts = spreadLabels(
			[
				{ centre: 5, width: 20 },
				{ centre: 10, width: 20 },
			],
			30,
			4,
		);
		expect(lefts.every((left) => left >= 0 && left <= 30)).toBe(true);
	});
});

describe('overlapsAny', () => {
	const boxes = [{ centre: 0, width: 10 }];
	it('reports a fixed label a tick label would cover', () => {
		expect(overlapsAny(0, 8, [0], boxes)).toBe(true);
		expect(overlapsAny(0, 8, [20], boxes)).toBe(false);
		expect(overlapsAny(0, 8, [], [])).toBe(false);
	});
});
