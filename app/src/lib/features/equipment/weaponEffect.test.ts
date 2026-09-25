import { describe, expect, it } from 'vitest';
import {
	activationRange,
	describeEffectProfile,
	effectFields,
	effectFormProblem,
	effectRequest,
	type WeaponEffectFields,
} from './weaponEffect';

const compound: WeaponEffectFields = {
	mode: 'compound',
	hitMin: 100,
	hitMax: 160,
	durationSeconds: 25,
	tickMin: 35,
	tickMax: 75,
	tickSeconds: 1.2,
};

describe('a declared weapon effect', () => {
	it('needs nothing for a weapon with no effect', () => {
		expect(effectFormProblem(effectFields(null))).toBeNull();
		expect(effectRequest(effectFields(null))).toBeNull();
	});

	it('says what is missing or inconsistent before it can be saved', () => {
		expect(effectFormProblem(compound)).toBeNull();
		expect(effectFormProblem({ ...compound, hitMax: null })).toBe('Enter the initial hit range');
		expect(effectFormProblem({ ...compound, hitMin: 200 })).toBe(
			'The hit minimum must not exceed its maximum',
		);
		expect(effectFormProblem({ ...compound, durationSeconds: 0 })).toBe(
			'Enter how long the effect lasts',
		);
		expect(effectFormProblem({ ...compound, durationSeconds: 601 })).toBe(
			'An effect lasts at most ten minutes',
		);
		expect(effectFormProblem({ ...compound, tickMax: null })).toBe('Enter the tick range');
		expect(effectFormProblem({ ...compound, tickMin: 0, tickMax: 0 })).toBe(
			'The tick maximum must be above zero',
		);
		expect(effectFormProblem({ ...compound, tickMin: -1 })).toBe(
			'The tick minimum cannot be negative',
		);
		expect(effectFormProblem({ ...compound, tickMax: 100_001 })).toBe(
			'A tick is at most 100,000 damage',
		);
		expect(effectFormProblem({ ...compound, tickMin: 80 })).toBe(
			'The tick minimum must not exceed its maximum',
		);
		expect(effectFormProblem({ ...compound, tickSeconds: 0 })).toBe(
			'The tick cadence must be above zero',
		);
		// Only a compound effect has an initial hit to enter.
		expect(
			effectFormProblem({ ...compound, mode: 'over_time', hitMin: null, hitMax: null }),
		).toBeNull();
	});

	it('shapes the request, dropping a hit range an effect with only ticks has no use for', () => {
		expect(effectRequest(compound)).toEqual({
			mode: 'compound',
			hit_min: 100,
			hit_max: 160,
			duration_seconds: 25,
			tick_min: 35,
			tick_max: 75,
			tick_seconds: 1.2,
		});
		expect(effectRequest({ ...compound, mode: 'over_time' })).toEqual({
			mode: 'over_time',
			hit_min: null,
			hit_max: null,
			duration_seconds: 25,
			tick_min: 35,
			tick_max: 75,
			tick_seconds: 1.2,
		});
	});

	it('reads a stored profile back into the form', () => {
		const stored = {
			mode: 'over_time' as const,
			hitMin: null,
			hitMax: null,
			durationSeconds: 12,
			tickMin: 4,
			tickMax: 6,
			tickSeconds: null,
		};
		expect(effectFields(stored)).toEqual({
			mode: 'over_time',
			hitMin: null,
			hitMax: null,
			durationSeconds: 12,
			tickMin: 4,
			tickMax: 6,
			tickSeconds: null,
		});
	});

	it('describes itself in one line, and names the range a cast prints', () => {
		const hitThenTicks = {
			mode: 'compound' as const,
			hitMin: 100,
			hitMax: 160,
			durationSeconds: 25,
			tickMin: 35,
			tickMax: 75.5,
			tickSeconds: null,
		};
		expect(describeEffectProfile(hitThenTicks)).toBe('Hit 100–160, then ticks 35–75.5 for 25 s');
		expect(activationRange(hitThenTicks)).toEqual({ min: 100, max: 160 });
		const ticksOnly = { ...hitThenTicks, mode: 'over_time' as const, hitMin: null, hitMax: null };
		expect(describeEffectProfile(ticksOnly)).toBe('Over time: ticks 35–75.5 for 25 s');
		expect(activationRange(ticksOnly)).toEqual({ min: 35, max: 75.5 });
	});
});
