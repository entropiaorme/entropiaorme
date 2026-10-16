/**
 * Skilling forecast fixtures shared by the routes, sessions, and hub tests:
 * one priced route, one named session's forecast, and the result around
 * them, each overridable field by field.
 */

import type {
	SkillingForecastResult,
	SkillingForecastSource,
	SkillingRouteForecast,
} from '$lib/api/commands.gen';
import type { StoredRoute } from '../routesView';

export const STORED_ROUTE: StoredRoute = {
	weaponId: 'sword',
	weaponName: 'Laconian Sword',
	species: 'Carabok',
	maturity: 'Puny',
	display: 'Puny Carabok',
};

export function routeForecast(
	overrides: Partial<SkillingRouteForecast> = {},
): SkillingRouteForecast {
	return {
		index: 0,
		weapon: 'Laconian Sword',
		mob: 'Puny Carabok',
		mobHp: 10,
		mobLevel: 4,
		status: 'ready',
		skillPerKillPec: 0.517,
		cycledPerKillPec: 4.56,
		lossPerKillPec: 0.36,
		secondsPerKill: 3.1,
		kills: 1000,
		cycledPed: 45.6,
		pedLost: 3.6,
		hours: 0.86,
		pace: null,
		skills: [],
		...overrides,
	};
}

export function sourceForecast(
	overrides: Partial<SkillingForecastSource> = {},
): SkillingForecastSource {
	return {
		definitionId: 9,
		name: 'Carabok Skilling',
		archived: false,
		sample: {
			sessions: 10,
			hours: 10,
			cycledPed: 500,
			lootTt: 450,
			pes: 40,
			realisedMarkup: null,
			markupLift: null,
		},
		status: 'ready',
		cycledPed: 40,
		hours: 2,
		lootTt: 36,
		kills: 900,
		markup: null,
		ttCost: 4,
		netCost: null,
		expected: null,
		skills: [],
		warnings: [],
		...overrides,
	};
}

export function forecastResult(
	overrides: Partial<SkillingForecastResult> = {},
): SkillingForecastResult {
	return {
		current: 48,
		goal: 50,
		sources: [],
		routes: [],
		sessionWeapons: [],
		huntingTarget: true,
		skillBoostPercent: 50,
		looterLevel: 33,
		...overrides,
	};
}
