import type {
	CalibrationStatus,
	ComputedCharacterStats,
	SkillLevel,
	ProfessionLevel,
	CharacterProspectOptions,
	ProspectResult,
	PathOptimizerResult,
	CodexSpecies,
	CodexRankBreakdown,
	CodexSkillOption
} from '$lib/types/analytics';

/**
 * Inline demo data for the character surface guide-mode mount.
 *
 * The character guide uses inline fixtures rather than the bundled demo DB +
 * `/demo/` namespace path because the surface walks a narrow scripted slice
 * (calibration status, a few skill rows, prospect + optimiser snapshots) that
 * does not need the full seeded catalogue.
 */

export const characterDemoCalibration: CalibrationStatus = {
	calibrated: true,
	lastCalibration: '2026-05-10T20:14:00Z',
	stale: false
};

export const characterDemoStats: ComputedCharacterStats = {
	hp: 142,
	topProfessions: [
		{ name: 'Laser Weaponry Technology', level: 62.4, anchorLevel: 60.1, gainSinceAnchor: 2.3, category: 'Hit' },
		{ name: 'Anatomy', level: 58.7, anchorLevel: 57.0, gainSinceAnchor: 1.7, category: 'Hit' },
		{ name: 'Inflict Ranged Damage', level: 55.2, anchorLevel: 53.8, gainSinceAnchor: 1.4, category: 'Damage' }
	]
};

export const characterDemoSkills: SkillLevel[] = [
	{ name: 'Strength', category: 'Attributes', level: 38.20, anchorLevel: 36.50, gainSinceAnchor: 1.70, rankName: 'Adept', ttValue: 0, isAttribute: true },
	{ name: 'Stamina', category: 'Attributes', level: 71.80, anchorLevel: 70.40, gainSinceAnchor: 1.40, rankName: 'Expert', ttValue: 0, isAttribute: true },
	{ name: 'Agility', category: 'Attributes', level: 52.15, anchorLevel: 50.90, gainSinceAnchor: 1.25, rankName: 'Skilled', ttValue: 0, isAttribute: true },
	{ name: 'Psyche', category: 'Attributes', level: 34.10, anchorLevel: 33.20, gainSinceAnchor: 0.90, rankName: 'Adept', ttValue: 0, isAttribute: true },
	{ name: 'Intelligence', category: 'Attributes', level: 60.45, anchorLevel: 58.80, gainSinceAnchor: 1.65, rankName: 'Expert', ttValue: 0, isAttribute: true },

	{ name: 'Laser Weaponry Technology', category: 'Combat', level: 62.40, anchorLevel: 60.10, gainSinceAnchor: 2.30, rankName: 'Expert', ttValue: 18.42, isAttribute: false },
	{ name: 'Inflict Ranged Damage', category: 'Combat', level: 55.20, anchorLevel: 53.80, gainSinceAnchor: 1.40, rankName: 'Skilled', ttValue: 14.88, isAttribute: false },
	{ name: 'Aim', category: 'Combat', level: 48.65, anchorLevel: 47.20, gainSinceAnchor: 1.45, rankName: 'Skilled', ttValue: 11.20, isAttribute: false },
	{ name: 'Combat Reflexes', category: 'Combat', level: 41.10, anchorLevel: 40.05, gainSinceAnchor: 1.05, rankName: 'Skilled', ttValue: 9.18, isAttribute: false },
	{ name: 'Wounded', category: 'Combat', level: 37.55, anchorLevel: 36.30, gainSinceAnchor: 1.25, rankName: 'Adept', ttValue: 7.34, isAttribute: false },

	{ name: 'Anatomy', category: 'Healing', level: 58.70, anchorLevel: 57.00, gainSinceAnchor: 1.70, rankName: 'Expert', ttValue: 16.05, isAttribute: false },
	{ name: 'First Aid', category: 'Healing', level: 44.20, anchorLevel: 43.10, gainSinceAnchor: 1.10, rankName: 'Skilled', ttValue: 10.40, isAttribute: false },

	{ name: 'Athletics', category: 'Athletic', level: 33.85, anchorLevel: 32.50, gainSinceAnchor: 1.35, rankName: 'Adept', ttValue: 6.10, isAttribute: false },
	{ name: 'Evade', category: 'Athletic', level: 46.30, anchorLevel: 45.00, gainSinceAnchor: 1.30, rankName: 'Skilled', ttValue: 10.85, isAttribute: false },

	{ name: 'Concentration', category: 'Wisdom', level: 40.95, anchorLevel: 39.80, gainSinceAnchor: 1.15, rankName: 'Skilled', ttValue: 8.45, isAttribute: false }
];

export const characterDemoProfessions: ProfessionLevel[] = [
	{ name: 'Laser Weaponry Technology', level: 62.40, anchorLevel: 60.10, gainSinceAnchor: 2.30, category: 'Hit' },
	{ name: 'Anatomy', level: 58.70, anchorLevel: 57.00, gainSinceAnchor: 1.70, category: 'Hit' },
	{ name: 'Inflict Ranged Damage', level: 55.20, anchorLevel: 53.80, gainSinceAnchor: 1.40, category: 'Damage' },
	{ name: 'Medic', level: 44.20, anchorLevel: 43.10, gainSinceAnchor: 1.10, category: 'Healer' },
	{ name: 'Dodger', level: 46.30, anchorLevel: 45.00, gainSinceAnchor: 1.30, category: 'Evader' }
];

export const characterDemoProspectOptions: CharacterProspectOptions = {
	tags: [],
	mobs: [],
	weapons: []
};

/** Pre-seeded Prospect forecast for the character-tab guide. Global slice, plausible
 *  shape for the Laser Weaponry Technology profession at the demo character's level. */
export const characterDemoProspectProfession = 'Laser Weaponry Technology';
export const characterDemoProspectTargetLevel = '70';
export const characterDemoProspectResult: ProspectResult = {
	profession: 'Laser Weaponry Technology',
	sliceType: 'global',
	sliceValue: null,
	markupUplift: 0,
	currentLevel: 62.40,
	targetLevel: 70.00,
	projectedCycledPed: 4820.50,
	projectedHours: 18.4,
	expectedLootTt: 4531.27,
	expectedNetTtBurn: 289.23,
	speculativeLootTt: null,
	speculativeNetTtBurn: null,
	sample: {
		sessions: 47,
		kills: 2814,
		hours: 38.6,
		cycledPed: 10240.15,
		lootTt: 9625.74,
		pes: 18.42,
		attributeLevels: 5.20,
		cycledPerHour: 265.29,
		lootPerHour: 249.37,
		returnRate: 0.94,
		pesPerPed: 0.0018,
		lootTtPerPed: 0.94
	},
	rows: [],
	warnings: []
};

/** Pre-seeded Profession-path-optimizer result for the character-tab guide.
 *  Laser Weaponry Technology 62.40 → 70.00, plausible weighted-allocation across
 *  the chip-in skills, total PED in the ~4.5k range. Sorted by pedCost descending
 *  (largest investment first per the table's colour-by-rank cue). */
export const characterDemoOptimizerProfession = 'Laser Weaponry Technology';
export const characterDemoOptimizerTargetLevel = '70';
export const characterDemoPathOptimizer: PathOptimizerResult = {
	profession: 'Laser Weaponry Technology',
	mode: 'target',
	inputTargetLevel: 70,
	inputPedBudget: null,
	currentLevel: 62.40,
	endLevel: 70.00,
	professionLevelsGained: 7.60,
	totalPed: 4521.85,
	allocations: [
		{ name: 'Laser Weaponry Technology', weight: 10, currentLevel: 62.40, levelsToGain: 5.50, newLevel: 67.90, pedCost: 1842.30, codexCategory: 'Combat', codexDivisor: null },
		{ name: 'Inflict Ranged Damage', weight: 8, currentLevel: 55.20, levelsToGain: 6.80, newLevel: 62.00, pedCost: 1245.40, codexCategory: 'Combat', codexDivisor: null },
		{ name: 'Aim', weight: 6, currentLevel: 48.65, levelsToGain: 9.20, newLevel: 57.85, pedCost: 875.60, codexCategory: 'Combat', codexDivisor: null },
		{ name: 'Combat Reflexes', weight: 4, currentLevel: 41.10, levelsToGain: 8.30, newLevel: 49.40, pedCost: 412.20, codexCategory: 'Athletics', codexDivisor: null },
		{ name: 'Wounded', weight: 2, currentLevel: 37.55, levelsToGain: 5.40, newLevel: 42.95, pedCost: 146.35, codexCategory: 'Combat', codexDivisor: null }
	],
	attributes: [
		{ name: 'Agility', weight: 6, currentLevel: 52.15, contributionFactor: 120 },
		{ name: 'Intelligence', weight: 4, currentLevel: 60.45, contributionFactor: 80 }
	],
	excluded: []
};

/** Pre-seeded Codex selection for the character-tab guide. Atrox at rank 5 → next 6,
 *  with Laser Weaponry Technology pre-selected as the prioritisation profession.
 *  Skill options ranked by profession contribution; the ranked list resolves a
 *  clear #1 recommendation. */
export const characterDemoCodexSelectedSpecies = 'Atrox';
export const characterDemoCodexSelectedProfession = 'Laser Weaponry Technology';

export const characterDemoCodexSpecies: CodexSpecies[] = [
	{ name: 'Argonaut', baseCost: 100, codexType: 'standard', currentRank: 0, nextRank: 1, nextCategory: 'cat1', nextCost: 100 },
	{ name: 'Atrax', baseCost: 100, codexType: 'standard', currentRank: 2, nextRank: 3, nextCategory: 'cat1', nextCost: 150 },
	{ name: 'Atrox', baseCost: 100, codexType: 'standard', currentRank: 5, nextRank: 6, nextCategory: 'cat2', nextCost: 500 },
	{ name: 'Berycled', baseCost: 100, codexType: 'standard', currentRank: 12, nextRank: 13, nextCategory: 'cat3', nextCost: 1200 },
	{ name: 'Combibo', baseCost: 100, codexType: 'standard', currentRank: 25, nextRank: null, nextCategory: null, nextCost: null },
	{ name: 'Daikiba', baseCost: 100, codexType: 'standard', currentRank: 3, nextRank: 4, nextCategory: 'cat1', nextCost: 200 },
	{ name: 'Equus', baseCost: 100, codexType: 'standard', currentRank: 8, nextRank: 9, nextCategory: 'cat2', nextCost: 700 }
];

export const characterDemoCodexRankBreakdown: CodexRankBreakdown = {
	speciesName: 'Atrox',
	baseCost: 100,
	codexType: 'standard',
	currentRank: 5,
	ranks: [
		{ rank: 1, category: 'cat1', cost: 100, rewardPed: 0.50, cat4Bonus: false, cat4RewardPed: null, skills: ['Aim', 'Combat Reflexes'], cat4Skills: [], claimed: true, claimedSkill: 'Aim', claimedPed: 0.50, isNext: false },
		{ rank: 2, category: 'cat1', cost: 150, rewardPed: 0.75, cat4Bonus: false, cat4RewardPed: null, skills: ['Inflict Ranged Damage', 'Laser Weaponry Technology'], cat4Skills: [], claimed: true, claimedSkill: 'Inflict Ranged Damage', claimedPed: 0.75, isNext: false },
		{ rank: 3, category: 'cat1', cost: 200, rewardPed: 1.00, cat4Bonus: false, cat4RewardPed: null, skills: ['Aim', 'Wounded'], cat4Skills: [], claimed: true, claimedSkill: 'Aim', claimedPed: 1.00, isNext: false },
		{ rank: 4, category: 'cat2', cost: 300, rewardPed: 1.50, cat4Bonus: false, cat4RewardPed: null, skills: ['Combat Reflexes', 'Aim'], cat4Skills: [], claimed: true, claimedSkill: 'Combat Reflexes', claimedPed: 1.50, isNext: false },
		{ rank: 5, category: 'cat2', cost: 400, rewardPed: 2.00, cat4Bonus: false, cat4RewardPed: null, skills: ['Aim', 'Inflict Ranged Damage'], cat4Skills: [], claimed: true, claimedSkill: 'Aim', claimedPed: 2.00, isNext: false },
		{ rank: 6, category: 'cat2', cost: 500, rewardPed: 2.50, cat4Bonus: false, cat4RewardPed: null, skills: ['Aim', 'Inflict Ranged Damage', 'Laser Weaponry Technology', 'Combat Reflexes', 'Wounded'], cat4Skills: [], claimed: false, claimedSkill: null, claimedPed: null, isNext: true }
	]
};

export const characterDemoCodexSkillOptions: CodexSkillOption[] = [
	{ skillName: 'Inflict Ranged Damage', category: 'cat2', rewardPed: 2.50, currentLevel: 55.20, levelsGained: 4.20, professionWeight: 8, profContribution: 0.0035, hpIncrease: null, hpGain: 0, recommendRank: 1 },
	{ skillName: 'Aim', category: 'cat2', rewardPed: 2.50, currentLevel: 48.65, levelsGained: 5.80, professionWeight: 6, profContribution: 0.0028, hpIncrease: null, hpGain: 0, recommendRank: 2 },
	{ skillName: 'Laser Weaponry Technology', category: 'cat2', rewardPed: 2.50, currentLevel: 62.40, levelsGained: 2.10, professionWeight: 10, profContribution: 0.0024, hpIncrease: null, hpGain: 0, recommendRank: 3 },
	{ skillName: 'Combat Reflexes', category: 'cat2', rewardPed: 2.50, currentLevel: 41.10, levelsGained: 7.30, professionWeight: 4, profContribution: 0.0018, hpIncrease: null, hpGain: 0, recommendRank: 4 },
	{ skillName: 'Wounded', category: 'cat2', rewardPed: 2.50, currentLevel: 37.55, levelsGained: 8.50, professionWeight: 2, profContribution: 0.0008, hpIncrease: null, hpGain: 0, recommendRank: 5 }
];
