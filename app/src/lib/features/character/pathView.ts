/**
 * The Optimiser's answer in one shape. A profession's path (or a family's)
 * and HP's are the same question: the least skill PES that reaches the
 * goal, spread where each skill moves the target most. Only the rate a
 * skill climbs at differs (a profession weight, or levels per HP).
 */

import type { HpOptimizerResult, PathOptimizerResult } from '$lib/api/commands.gen';

export interface PathRow {
	name: string;
	/** What a level of this skill is worth to the target, as its column reads. */
	rate: string;
	currentLevel: number;
	newLevel: number;
	pedCost: number;
}

export interface PathAnswer {
	/** The rate column's heading. */
	rateLabel: string;
	totalPed: number;
	/** Where the allocation ends. */
	end: number;
	/** Short of the goal: every skill tops out on the curve first. */
	short: boolean;
	/** Whether any unlocked skill moves the target at all. */
	climbable: boolean;
	/** The skills the PES goes to, dearest first. */
	allocated: PathRow[];
	/** The skills left out, with why. */
	leftOut: { name: string; reason: string }[];
	/** What an attribute reward would add, if one is offered. */
	attributes: { name: string; figure: string }[];
	/** Which way the attribute figure is better. */
	attributeNote: string;
}

/** The end figures round to two places, so an end within half a hundredth
 * of the goal has reached it. */
const REACHED_TOLERANCE = 0.005;

interface Allocation {
	name: string;
	rate: string;
	currentLevel: number;
	newLevel: number;
	levelsToGain: number;
	pedCost: number;
}

function answer(
	goal: number,
	end: number,
	totalPed: number,
	allocations: Allocation[],
	excluded: { name: string; reason: string }[],
): Pick<PathAnswer, 'totalPed' | 'end' | 'short' | 'climbable' | 'allocated' | 'leftOut'> {
	const allocated: PathRow[] = [];
	const unneeded: { name: string; reason: string }[] = [];
	for (const alloc of allocations) {
		// A climb the curve prices at nothing (its anchors are to the PED
		// cent, so a few low or flat levels round free) asks for no PES.
		if (alloc.levelsToGain > 0 && alloc.pedCost > 0) {
			allocated.push({
				name: alloc.name,
				rate: alloc.rate,
				currentLevel: alloc.currentLevel,
				newLevel: alloc.newLevel,
				pedCost: alloc.pedCost,
			});
		} else {
			unneeded.push({ name: alloc.name, reason: 'not needed for this goal' });
		}
	}
	return {
		totalPed,
		end,
		short: end + REACHED_TOLERANCE < goal,
		climbable: allocations.length > 0,
		allocated,
		leftOut: [...unneeded, ...excluded.map(({ name, reason }) => ({ name, reason }))],
	};
}

/** A profession's (or family's) path to its target level. */
export function professionAnswer(path: PathOptimizerResult): PathAnswer {
	return {
		...answer(
			path.inputTargetLevel ?? path.endLevel,
			path.endLevel,
			path.totalPed,
			path.allocations.map((alloc) => ({ ...alloc, rate: String(alloc.weight) })),
			path.excluded,
		),
		rateLabel: 'Weight',
		attributes: path.attributes.map((attr) => ({
			name: attr.name,
			figure: `×${attr.contributionFactor}`,
		})),
		attributeNote: 'Higher contribution is better.',
	};
}

/** HP's path to its target HP. */
export function hpAnswer(path: HpOptimizerResult): PathAnswer {
	return {
		...answer(
			path.targetHp,
			path.endHp,
			path.totalPed,
			path.allocations.map((alloc) => ({
				...alloc,
				rate: alloc.levelsPerHp.toLocaleString('en-GB'),
			})),
			path.excluded,
		),
		rateLabel: 'Levels per HP',
		attributes: path.attributes.map((attr) => ({
			name: attr.name,
			figure: `${attr.levelsPerHp.toLocaleString('en-GB')} lvl/HP`,
		})),
		attributeNote: 'Fewer levels per HP is better.',
	};
}
