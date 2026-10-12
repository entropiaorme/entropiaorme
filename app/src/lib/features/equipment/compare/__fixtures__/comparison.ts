/** Comparison fixtures in the wire shape, for the Compare-tab suites. */

import type { ComparedSetup, KillFiguresView, LoadoutComparison } from '$lib/api/comparison';

export const BC30 = 'Exceptional A&P Series Mayhem BC-30, Modified (L)';
export const GAUSS = 'Exceptional Kriegerum Mayhem G.2 Gauss Rifle, Perfected (L)';

/** A curve whose loss grows linearly at `lambda` PEC per HP over a base. */
export function curve(base: number, lambda: number, secondsPerHp = 0.04): KillFiguresView[] {
	return [1, 250, 500, 750, 1000].map((hp) => ({
		hp,
		uses: 1 + hp / 30,
		lossPec: base + lambda * hp,
		cycledPec: (base + lambda * hp) * 20,
		seconds: hp * secondsPerHp,
	}));
}

/** Per-use figures in the wire shape; a BC-30-like default whose loss
 * splits into its premium, Efficiency shortfall, and house edge. */
export function economics(
	overrides: Partial<NonNullable<ComparedSetup['economics']>> = {},
): NonNullable<ComparedSetup['economics']> {
	return {
		costPerUsePec: 8,
		lossPerUsePec: 0.4,
		consumedPremiumPec: 0.03,
		outsideModelPec: 0,
		efficiencyShortfallPec: 0.09,
		houseEdgePec: 0.28,
		effectiveEfficiency: { status: 'within_model_range' as const, efficiencyPct: 75.9 },
		weightedEfficiencyPct: 80.5,
		maxDamage: 38,
		ampDamageWasted: 0,
		damagePerPec: 3.3,
		usesPerMinute: 67,
		...overrides,
	};
}

/** One part of a setup in the wire shape: a limited weapon by default. */
export function component(
	overrides: Partial<ComparedSetup['components'][number]> = {},
): ComparedSetup['components'][number] {
	return {
		role: 'weapon',
		catalogId: 'weapon',
		name: 'Weapon',
		isLimited: true,
		markupPercent: 100,
		decayPecPerUse: 0.002,
		maxTtPed: 45,
		...overrides,
	};
}

export function setup(
	index: number,
	name: string,
	overrides: Partial<ComparedSetup> = {},
): ComparedSetup {
	return {
		index,
		name,
		components: [component({ catalogId: `weapon-${index}`, name })],
		economics: economics(),
		unmodelled: null,
		curve: curve(0.2, 0.016),
		...overrides,
	};
}

export function comparison(overrides: Partial<LoadoutComparison> = {}): LoadoutComparison {
	return {
		assumptions: {
			modelVersion: 'community_v1',
			looterSource: 'three_looter_mean',
			looterLevel: 50,
			ownLooterLevel: 50,
			hitRate: 0.9,
			damageFloor: 0.5,
			criticalChance: 0.02,
			criticalMultiplier: 2,
		},
		hpMax: 1000,
		setups: [setup(0, BC30), setup(1, GAUSS)],
		winners: [{ index: 0, fromHp: 1, toHp: 1000 }],
		...overrides,
	};
}

/** The BC-30 against the Gauss: the Gauss's one-hit window at 36 to 44
 * HP, then the BC-30 to 95 HP and the Gauss above. */
export function bc30AgainstGauss(): LoadoutComparison {
	return comparison({
		setups: [
			setup(0, BC30, { curve: curve(0.25, 0.016) }),
			setup(1, GAUSS, {
				curve: curve(0.7, 0.0112),
				economics: economics({
					maxDamage: 80.8,
					damagePerPec: 3.7,
				}),
			}),
		],
		winners: [
			{ index: 0, fromHp: 1, toHp: 35.9 },
			{ index: 1, fromHp: 35.9, toHp: 43.9 },
			{ index: 0, fromHp: 43.9, toHp: 95.2 },
			{ index: 1, fromHp: 95.2, toHp: 1000 },
		],
	});
}
