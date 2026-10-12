import { describe, expect, it } from 'vitest';
import { NO_DATA } from '$lib/utils/format';
import {
	BC30,
	bc30AgainstGauss,
	comparison,
	component,
	economics,
	GAUSS,
	setup,
} from './__fixtures__/comparison';
import {
	buildChart,
	describeUnmodelled,
	formatCompact,
	formatCount,
	formatDuration,
	formatKillPec,
	formatPed,
	isLimitedSetup,
	lossPerKill,
	niceRangeTicks,
	niceTicks,
	parseAmount,
	partsUsedUp,
	payback,
	planFigures,
	setupLabels,
} from './comparisonView';

describe('setupLabels', () => {
	it('names each setup by weapon and amplifier, and numbers identical ones', () => {
		const amped = setup(1, BC30, {
			components: [
				component({ catalogId: 'bc', name: BC30, markupPercent: 1800 }),
				component({
					role: 'amplifier',
					catalogId: 'a106',
					name: 'Omegaton A106',
					isLimited: false,
				}),
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

// The fixture curve takes 1 + HP / 30 uses per kill, sampled at 1, 250,
// 500, 750, and 1,000 HP; a use costs 8 PEC, 0.03 of it premium, and loses
// 0.4 PEC.
const USES_AT_250 = 1 + 250 / 30;

describe('planFigures', () => {
	it('scales the per-use figures by the uses a number of kills takes', () => {
		const figures = planFigures(setup(0, BC30), 1000, 250);
		const uses = 1000 * USES_AT_250;
		expect(figures?.uses).toBeCloseTo(uses, 9);
		// TT cycled leaves out the premium, which is part of the loss instead.
		expect(figures?.cycledPed).toBeCloseTo((uses * 7.97) / 100, 9);
		expect(figures?.markupPed).toBeCloseTo((uses * 0.03) / 100, 9);
		expect(figures?.efficiencyPed).toBeCloseTo((uses * 0.09) / 100, 9);
		expect(figures?.unmodelledPed).toBe(0);
		expect(figures?.houseEdgePed).toBeCloseTo((uses * 0.28) / 100, 9);
		expect(figures?.seconds).toBeCloseTo((uses * 60) / 67, 9);
	});

	it('adds the house edge and the leak up to the expected loss', () => {
		const leaky = setup(0, BC30, {
			economics: economics({ outsideModelPec: 0.1, lossPerUsePec: 0.5 }),
		});
		const figures = planFigures(leaky, 200, 500);
		if (!figures) throw new Error('expected figures');
		expect(figures.leakPed).toBeCloseTo(
			figures.markupPed + figures.efficiencyPed + figures.unmodelledPed,
			12,
		);
		expect(figures.lossPed).toBeCloseTo(figures.houseEdgePed + figures.leakPed, 12);
		// The parts add up to the loss per use the backend computed.
		expect(figures.lossPed).toBeCloseTo((figures.uses * 0.5) / 100, 9);
		expect(figures.unmodelledPed).toBeCloseTo((figures.uses * 0.1) / 100, 9);
	});

	it('reads uses between the sampled points by interpolation', () => {
		expect(planFigures(setup(0, BC30), 1, 125)?.uses).toBeCloseTo(1 + 125 / 30, 9);
		expect(planFigures(setup(0, BC30), 1, 1)?.uses).toBeCloseTo(1 + 1 / 30, 9);
		expect(planFigures(setup(0, BC30), 1, 1000)?.uses).toBeCloseTo(1 + 1000 / 30, 9);
	});

	it('shares out the damage that lands past a small kill', () => {
		// A use deals 3.3 x 8 = 26.4 damage on average; a 1 HP mob takes
		// 1.033 uses, so almost all of it is overkill.
		const tiny = planFigures(setup(0, BC30), 1, 1);
		expect(tiny?.overkillShare).toBeCloseTo(1 - 1 / ((1 + 1 / 30) * 26.4), 9);
		// Past one use's damage there is none on this curve.
		expect(planFigures(setup(0, BC30), 1, 250)?.overkillShare).toBe(0);
	});

	it('has no figures off the curve or for an unpriced setup', () => {
		expect(planFigures(setup(0, BC30), 1, 1001)).toBeNull();
		expect(planFigures(setup(0, BC30), 1, 0.5)).toBeNull();
		expect(planFigures(setup(0, BC30, { curve: [] }), 1, 100)).toBeNull();
		expect(planFigures(setup(0, BC30, { economics: null }), 1, 100)).toBeNull();
		expect(
			planFigures(setup(0, BC30, { economics: economics({ usesPerMinute: null }) }), 1, 100)
				?.seconds,
		).toBeNull();
	});
});

describe('lossPerKill', () => {
	it('is the expected loss of one kill, in PED', () => {
		expect(lossPerKill(setup(0, BC30), 250)).toBeCloseTo((USES_AT_250 * 0.4) / 100, 12);
		expect(lossPerKill(setup(0, BC30, { economics: null }), 250)).toBeNull();
	});
});

describe('payback', () => {
	const cheaper = setup(1, 'Unlimited', {
		economics: economics({ lossPerUsePec: 0.3, houseEdgePec: 0.18 }),
	});
	const reference = setup(0, BC30);

	it('counts the kills a year whose saving covers the charge on the capital', () => {
		const result = payback(cheaper, reference, 250, 1000, 0.1);
		const saving = (USES_AT_250 * 0.1) / 100;
		expect(result?.kind).toBe('pays_back');
		expect(result?.capitalCostPed).toBeCloseTo(100, 12);
		expect(result?.saving).toBeCloseTo(saving, 12);
		if (result?.kind !== 'pays_back') throw new Error('expected a payback');
		expect(result.perYear).toBeCloseTo(100 / saving, 6);
	});

	it('never pays back when it loses at least as much per kill', () => {
		const result = payback(reference, cheaper, 250, 1000, 0.1);
		expect(result).toEqual({
			kind: 'never',
			capitalCostPed: 100,
			saving: expect.closeTo(-(USES_AT_250 * 0.1) / 100, 12),
		});
		expect(payback(reference, setup(2, BC30), 250, 1000, 0.1)?.kind).toBe('never');
	});

	it('has no answer where either setup has no figures', () => {
		expect(payback(cheaper, reference, 5000, 1000, 0.1)).toBeNull();
		expect(payback(cheaper, setup(0, BC30, { economics: null }), 250, 1000, 0.1)).toBeNull();
	});
});

describe('isLimitedSetup', () => {
	it('reads the weapon, not the other parts', () => {
		expect(isLimitedSetup(setup(0, BC30))).toBe(true);
		const unlimited = setup(0, 'Scimitar', {
			components: [
				component({ name: 'Scimitar', isLimited: false }),
				component({ role: 'absorber', name: 'ArMatrix Extender P20 (L)' }),
			],
		});
		expect(isLimitedSetup(unlimited)).toBe(false);
	});
});

describe('partsUsedUp', () => {
	it('counts the limited parts a number of uses wears out', () => {
		const full = setup(0, BC30, {
			components: [
				component({ decayPecPerUse: 0.002, maxTtPed: 45 }),
				component({ role: 'amplifier', isLimited: false, decayPecPerUse: 0.5, maxTtPed: 300 }),
				// A part without a full TT, or one that takes no decay, wears nothing out.
				component({ role: 'absorber', decayPecPerUse: 0.0004, maxTtPed: null }),
				component({ role: 'implant', decayPecPerUse: 0.0022, maxTtPed: 188 }),
			],
		});
		const used = partsUsedUp(full, 1_000_000);
		expect(used.map((part) => part.role)).toEqual(['weapon', 'implant']);
		expect(used[0].count).toBeCloseTo((1_000_000 * 0.002) / 100 / 45, 12);
		expect(used[1].count).toBeCloseTo((1_000_000 * 0.0022) / 100 / 188, 12);
		expect(
			partsUsedUp(setup(0, BC30, { components: [component({ decayPecPerUse: 0 })] }), 10),
		).toEqual([]);
	});
});

describe('parseAmount', () => {
	it('reads plain, grouped, and suffixed amounts', () => {
		expect(parseAmount('42')).toBe(42);
		expect(parseAmount(' 2.5 ')).toBe(2.5);
		expect(parseAmount('150,000')).toBe(150_000);
		expect(parseAmount('250k')).toBe(250_000);
		expect(parseAmount('1.5M')).toBe(1_500_000);
		expect(parseAmount('1.5 m')).toBe(1_500_000);
		expect(parseAmount('+2k')).toBe(2000);
	});

	it('refuses anything else', () => {
		for (const text of ['', 'abc', '-5', '1e3', '2b', '1.2.3', 'k']) {
			expect(parseAmount(text)).toBeNull();
		}
	});
});

describe('the plan formatters', () => {
	it('formats PED by size', () => {
		expect(formatPed(1234.5)).toBe('1,235');
		expect(formatPed(12.345)).toBe('12.3');
		expect(formatPed(1.234)).toBe('1.23');
		expect(formatPed(Number.NaN)).toBe(NO_DATA);
	});

	it('formats counts, durations, and compact figures', () => {
		expect(formatCount(12_345.9)).toBe('12,345');
		expect(formatCount(null)).toBe(NO_DATA);
		expect(formatDuration(59)).toBe('1 min');
		expect(formatDuration(2 * 3600 + 5 * 60)).toBe('2 h 05 min');
		expect(formatDuration(null)).toBe(NO_DATA);
		// The suffix's case follows the runtime's locale data.
		expect(formatCompact(375_000)).toMatch(/^375k$/i);
		expect(formatCompact(1_234_567)).toMatch(/^1\.2m$/i);
		expect(formatCompact(950)).toBe('950');
		expect(formatCompact(Number.POSITIVE_INFINITY)).toBe(NO_DATA);
	});
});
