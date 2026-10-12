// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from 'vitest';
import {
	atLooterBasis,
	DEFAULT_LOOTER_BASIS,
	initLooterBasis,
	looterBasis,
	looterBasisCaption,
	looterBasisLevel,
	looterBasisName,
	looterLevelsFrom,
	setLooterBasis,
} from './looterBasis.svelte';

beforeEach(async () => {
	localStorage.clear();
	await setLooterBasis(DEFAULT_LOOTER_BASIS);
});

describe('the looter basis preference', () => {
	it('defaults to the three-looter mean', async () => {
		localStorage.clear();
		await initLooterBasis();
		expect(looterBasis.current).toBe('three_looter_mean');
	});

	it('persists a choice and reads it back', async () => {
		await setLooterBasis('robot');
		expect(looterBasis.current).toBe('robot');
		expect(localStorage.getItem('looterBasis')).toBe('"robot"');
		await setLooterBasis('animal');
		localStorage.setItem('looterBasis', '"mutant"');
		await initLooterBasis();
		expect(looterBasis.current).toBe('mutant');
	});

	it('falls back to the mean for a stored value it does not know', async () => {
		localStorage.setItem('looterBasis', '"fish"');
		await initLooterBasis();
		expect(looterBasis.current).toBe('three_looter_mean');
	});
});

describe('reading a basis', () => {
	const levels = { animal: 30, mutant: 45, robot: 90 };

	it('picks the figure a per-basis set carries', () => {
		const set = { threeLooterMean: 'mean', animal: 'a', mutant: 'm', robot: 'r' };
		expect(atLooterBasis(set, 'three_looter_mean')).toBe('mean');
		expect(atLooterBasis(set, 'animal')).toBe('a');
		expect(atLooterBasis(set, 'mutant')).toBe('m');
		expect(atLooterBasis(set, 'robot')).toBe('r');
	});

	it('stands for one looter, or the mean of exactly the three', () => {
		expect(looterBasisLevel(levels, 'three_looter_mean')).toBe(55);
		expect(looterBasisLevel(levels, 'robot')).toBe(90);
	});

	it('names itself for a control and for a disclosure', () => {
		expect(looterBasisName('three_looter_mean')).toBe('Looter mean');
		expect(looterBasisName('mutant')).toBe('Mutant Looter');
		expect(looterBasisCaption('three_looter_mean', 55)).toBe('three-looter mean 55.0');
		expect(looterBasisCaption('animal', 30)).toBe('Animal Looter 30.0');
		expect(looterBasisCaption(null, 63)).toBe('looter level 63.0');
	});

	it('reads the three levels from a profession list, a missing one as 0', () => {
		expect(
			looterLevelsFrom([
				{ name: 'Robot Looter', level: 12 },
				{ name: 'Animal Looter', level: 40 },
				{ name: 'Mineable Looter', level: 99 },
			]),
		).toEqual({ animal: 40, mutant: 0, robot: 12 });
	});
});
