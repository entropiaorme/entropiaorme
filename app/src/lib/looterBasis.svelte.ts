/**
 * The looter basis the expected-hunting figures are read at.
 *
 * Community Model v1 takes one looter level. When the hunted target's
 * class is known, Animal, Mutant, or Robot Looter applies; otherwise the
 * mean of the three stands in. The player knows what they mean to hunt
 * with a setup, so the planning surfaces (an Equipment weapon's expected
 * return, Market's break-even) let them choose, and the choice is one
 * app-wide preference: which class a player hunts is a standing fact
 * about their play, and every surface names the basis beside its figures,
 * so a remembered choice is never a hidden one.
 *
 * Historical surfaces never take this choice: a recorded session replays
 * the looter levels it was captured with.
 */

import type { ExpectedLooterSource } from '$lib/api/commands.gen';
import { getPreference, setPreference } from './preferences';

export type LooterBasis = ExpectedLooterSource;

/** The three hunting looters' believed-current levels. */
export type LooterLevels = { animal: number; mutant: number; robot: number };

/** A figure read at every basis, keyed the way the wire carries it. */
export type PerLooterBasis<T> = {
	threeLooterMean: T;
	animal: T;
	mutant: T;
	robot: T;
};

/** Menu order: the fallback first, then the classes. */
export const LOOTER_BASES: readonly LooterBasis[] = [
	'three_looter_mean',
	'animal',
	'mutant',
	'robot',
];

export const DEFAULT_LOOTER_BASIS: LooterBasis = 'three_looter_mean';

const KEY_LOOTER_BASIS = 'looterBasis';

const WIRE_KEY: Record<LooterBasis, keyof PerLooterBasis<unknown>> = {
	three_looter_mean: 'threeLooterMean',
	animal: 'animal',
	mutant: 'mutant',
	robot: 'robot',
};

const NAME: Record<LooterBasis, string> = {
	three_looter_mean: 'Looter mean',
	animal: 'Animal Looter',
	mutant: 'Mutant Looter',
	robot: 'Robot Looter',
};

/** Short tick labels where a full name does not fit. */
const INITIAL: Record<LooterBasis, string> = {
	three_looter_mean: 'mean',
	animal: 'A',
	mutant: 'M',
	robot: 'R',
};

function sanitise(value: unknown): LooterBasis {
	return LOOTER_BASES.includes(value as LooterBasis)
		? (value as LooterBasis)
		: DEFAULT_LOOTER_BASIS;
}

let basis = $state<LooterBasis>(DEFAULT_LOOTER_BASIS);

export const looterBasis = {
	get current(): LooterBasis {
		return basis;
	},
};

export async function initLooterBasis(): Promise<void> {
	basis = sanitise(await getPreference<unknown>(KEY_LOOTER_BASIS, DEFAULT_LOOTER_BASIS));
}

export async function setLooterBasis(value: LooterBasis): Promise<void> {
	const clean = sanitise(value);
	basis = clean;
	await setPreference(KEY_LOOTER_BASIS, clean);
}

/** The figure a per-basis set carries at `chosen`. */
export function atLooterBasis<T>(set: PerLooterBasis<T>, chosen: LooterBasis): T {
	return set[WIRE_KEY[chosen]];
}

/** The level a basis stands for: one looter's, or the mean of the three. */
export function looterBasisLevel(levels: LooterLevels, chosen: LooterBasis): number {
	return chosen === 'three_looter_mean'
		? (levels.animal + levels.mutant + levels.robot) / 3
		: levels[chosen];
}

/** "Looter mean", "Animal Looter", and so on. */
export function looterBasisName(chosen: LooterBasis): string {
	return NAME[chosen];
}

/** How a figure's disclosure names its basis: "three-looter mean 41.2",
 * "Animal Looter 45.0", or, for a level chosen between them (no basis),
 * "looter level 63.0". */
export function looterBasisCaption(chosen: LooterBasis | null, level: number): string {
	const name =
		chosen === null
			? 'looter level'
			: chosen === 'three_looter_mean'
				? 'three-looter mean'
				: NAME[chosen];
	return `${name} ${level.toFixed(1)}`;
}

export function looterBasisInitial(chosen: LooterBasis): string {
	return INITIAL[chosen];
}

/** The three levels from a character's profession list; a profession the
 * list lacks reads as level 0. */
export function looterLevelsFrom(professions: { name: string; level: number }[]): LooterLevels {
	const level = (name: string) =>
		professions.find((profession) => profession.name === name)?.level ?? 0;
	return {
		animal: level('Animal Looter'),
		mutant: level('Mutant Looter'),
		robot: level('Robot Looter'),
	};
}
