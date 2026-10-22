import { describe, expect, it } from 'vitest';
import type {
	HpOptimizerAllocation,
	HpOptimizerResult,
	PathOptimizerResult,
} from '$lib/api/commands.gen';
import { hpAnswer, professionAnswer } from './pathView';

function hpRow(overrides: Partial<HpOptimizerAllocation> = {}): HpOptimizerAllocation {
	return {
		name: 'Athletics',
		levelsPerHp: 533.3,
		currentLevel: 3000,
		levelsToGain: 1066.6,
		newLevel: 4066.6,
		hpGain: 2,
		pedCost: 14.2,
		codexCategory: 'cat1',
		codexDivisor: 200,
		...overrides,
	};
}

function hpPath(overrides: Partial<HpOptimizerResult> = {}): HpOptimizerResult {
	return {
		targetHp: 190,
		currentHp: 188.4,
		endHp: 190,
		totalPed: 14.2,
		allocations: [
			hpRow(),
			hpRow({ name: 'Handgun', levelsPerHp: 1600, levelsToGain: 0, newLevel: 3000, pedCost: 0 }),
		],
		attributes: [{ name: 'Stamina', currentLevel: 40, levelsPerHp: 9.25 }],
		excluded: [{ name: 'Commando', levelsPerHp: 200, reason: 'not unlocked' }],
		...overrides,
	};
}

function professionPath(overrides: Partial<PathOptimizerResult> = {}): PathOptimizerResult {
	return {
		allocations: [
			{
				name: 'Rifle',
				weight: 40,
				currentLevel: 2000,
				levelsToGain: 120,
				pedCost: 9.5,
				newLevel: 2120,
				codexCategory: 'cat1',
				codexDivisor: 200,
			},
		],
		attributes: [{ name: 'Agility', weight: 5, currentLevel: 40, contributionFactor: 100 }],
		profession: 'Marksman',
		mode: 'target',
		inputTargetLevel: 43,
		inputPedBudget: null,
		currentLevel: 42.52,
		endLevel: 43,
		professionLevelsGained: 0.48,
		totalPed: 9.5,
		excluded: [],
		...overrides,
	};
}

describe('hpAnswer', () => {
	it('lists the skills that climb, with their levels per HP', () => {
		const answer = hpAnswer(hpPath());
		expect(answer.rateLabel).toBe('Levels per HP');
		expect(answer.allocated).toEqual([
			{ name: 'Athletics', rate: '533.3', currentLevel: 3000, newLevel: 4066.6, pedCost: 14.2 },
		]);
		expect(answer.totalPed).toBe(14.2);
		expect(answer.short).toBe(false);
		expect(answer.climbable).toBe(true);
	});

	it('leaves out the skills the goal does not need, then the locked ones', () => {
		expect(hpAnswer(hpPath()).leftOut).toEqual([
			{ name: 'Handgun', reason: 'not needed for this goal' },
			{ name: 'Commando', reason: 'not unlocked' },
		]);
	});

	it('folds a climb the curve prices at nothing into the left out', () => {
		const answer = hpAnswer(
			hpPath({
				allocations: [
					hpRow(),
					hpRow({ name: 'Rifle', levelsPerHp: 1600, levelsToGain: 1, newLevel: 3001, pedCost: 0 }),
				],
				excluded: [],
			}),
		);
		expect(answer.allocated.map((row) => row.name)).toEqual(['Athletics']);
		expect(answer.leftOut).toEqual([{ name: 'Rifle', reason: 'not needed for this goal' }]);
	});

	it('reads attribute rewards in levels per HP, fewer being better', () => {
		const answer = hpAnswer(hpPath());
		expect(answer.attributes).toEqual([{ name: 'Stamina', figure: '9.25 lvl/HP' }]);
		expect(answer.attributeNote).toBe('Fewer levels per HP is better.');
	});

	it('falls short only when the end misses the goal past rounding', () => {
		expect(hpAnswer(hpPath({ targetHp: 190.004, endHp: 190 })).short).toBe(false);
		const short = hpAnswer(hpPath({ targetHp: 900, endHp: 612.4 }));
		expect(short.short).toBe(true);
		expect(short.end).toBe(612.4);
	});

	it('has nothing to climb before a scan', () => {
		const answer = hpAnswer(hpPath({ allocations: [], endHp: 0, totalPed: 0 }));
		expect(answer.climbable).toBe(false);
		expect(answer.allocated).toEqual([]);
	});
});

describe('professionAnswer', () => {
	it('reads the same table with the profession weight as the rate', () => {
		const answer = professionAnswer(professionPath());
		expect(answer.rateLabel).toBe('Weight');
		expect(answer.allocated[0]).toEqual({
			name: 'Rifle',
			rate: '40',
			currentLevel: 2000,
			newLevel: 2120,
			pedCost: 9.5,
		});
		expect(answer.attributes).toEqual([{ name: 'Agility', figure: '×100' }]);
		expect(answer.attributeNote).toBe('Higher contribution is better.');
		expect(answer.short).toBe(false);
	});

	it('falls short of a level beyond every skill ceiling', () => {
		expect(professionAnswer(professionPath({ inputTargetLevel: 400, endLevel: 212.3 })).short).toBe(
			true,
		);
	});
});
