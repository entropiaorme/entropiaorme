/**
 * The character family: calibration, stats, skills, professions, the
 * optimisers, the activity recommender, and the skilling forecast. Thin
 * wrappers over the generated typed commands; argument shaping only.
 */

import * as commands from './commands.gen';

export const getCalibrationStatus = commands.characterCalibration;
export const getCharacterStats = commands.characterStats;
export const getCharacterSkills = commands.characterSkills;
export const getCharacterProfessions = commands.characterProfessions;
export const getHpOptimizer = commands.characterHpOptimizer;
export const getActivityRecommender = commands.characterActivityRecommender;
export const getSkillingForecast = commands.characterSkillingForecast;

/** The cheapest skill path for one profession, or for several optimised as
 * one combined target (a family, levelled as the sum of its members). */
export async function getProfessionPathOptimizer(
	professions: string[],
	params: { targetLevel: number } | { pedBudget: number },
) {
	const targetLevel = 'targetLevel' in params ? params.targetLevel : null;
	const pedBudget = 'pedBudget' in params ? params.pedBudget : null;
	return commands.characterPathOptimizer(professions, targetLevel, pedBudget);
}
