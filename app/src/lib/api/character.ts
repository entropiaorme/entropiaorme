/**
 * The character family: calibration, stats, skills, professions, the
 * optimisers, the prospect forecast, and the skilling forecast. Thin wrappers over the
 * generated typed commands; argument shaping only.
 */

import type { ProspectQuery, ProspectResult, ProspectSliceType } from './commands.gen';
import * as commands from './commands.gen';

export const getCalibrationStatus = commands.characterCalibration;
export const getCharacterStats = commands.characterStats;
export const getCharacterSkills = commands.characterSkills;
export const getCharacterProfessions = commands.characterProfessions;
export const getProfessionOptimizer = commands.characterProfessionOptimizer;
export const getHpOptimizer = commands.characterHpOptimizer;
export const getCharacterProspectOptions = commands.characterProspectOptions;
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

export async function getCharacterProspect(params: {
	profession: string;
	targetLevel: number;
	sliceType: ProspectSliceType;
	sliceValue?: string | null;
	markupUplift?: number;
}): Promise<ProspectResult> {
	const query: ProspectQuery = {
		profession: params.profession,
		targetLevel: params.targetLevel,
		sliceType: params.sliceType,
	};
	if (params.sliceType !== 'global' && params.sliceValue) {
		query.sliceValue = params.sliceValue;
	}
	if ((params.markupUplift ?? 0) > 0) {
		query.markupUplift = params.markupUplift;
	}
	return commands.characterProspect(query);
}
