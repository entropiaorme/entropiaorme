import { describe, expect, it } from 'vitest';
import type {
	SkillingForecastResult,
	SkillingForecastSource,
	SkillingRouteForecast,
} from '$lib/api/commands.gen';
import {
	isStoredRoute,
	routePick,
	routeRows,
	routeStatusLabel,
	type StoredRoute,
} from './routesView';

const STORED: StoredRoute = {
	weaponId: 'sword',
	weaponName: 'Laconian Sword',
	species: 'Carabok',
	maturity: 'Puny',
	display: 'Puny Carabok',
};

function route(overrides: Partial<SkillingRouteForecast> = {}): SkillingRouteForecast {
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
		skills: [],
		...overrides,
	};
}

function session(overrides: Partial<SkillingForecastSource> = {}): SkillingForecastSource {
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
		markup: null,
		ttCost: 4,
		netCost: null,
		skills: [],
		warnings: [],
		...overrides,
	};
}

function result(
	routes: SkillingRouteForecast[],
	sources: SkillingForecastSource[] = [],
): SkillingForecastResult {
	return { current: 48, goal: 50, sources, routes, skillBoost: 'fifty', looterLevel: 33 };
}

describe('routeRows', () => {
	it('lists answering routes cheapest first with the premium over the cheapest', () => {
		const rows = routeRows(
			result(
				[
					route({ index: 0, pedLost: 3.6, hours: 2 }),
					route({ index: 1, pedLost: 1.6, mob: 'Default Bank Robber' }),
				],
				[session({ ttCost: 4 })],
			),
			[STORED, { ...STORED, species: 'Bank Robber', maturity: 'Default' }],
		);
		expect(rows.map((row) => row.key)).toEqual(['route:1', 'route:0', 'session:9']);
		expect(rows[0].name).toBe('Laconian Sword on Default Bank Robber');
		expect(rows[0].premium).toBe(0);
		expect(rows[1].premium).toBeCloseTo(2);
		// Two PED more over two hours of this route: one PED an hour.
		expect(rows[1].premiumPerHour).toBeCloseTo(1);
		expect(rows[2].kind).toBe('recorded');
		expect(rows[2].pedLost).toBe(4);
	});

	it('leaves the premium out with only one answering route, and sinks the rest', () => {
		const rows = routeRows(
			result([route({ status: 'unknown_mob', pedLost: 0, hours: null }), route({ index: 1 })]),
			[STORED, STORED],
		);
		expect(rows.map((row) => row.key)).toEqual(['route:1', 'route:0']);
		expect(rows[0].premium).toBeNull();
		expect(rows[1].premium).toBeNull();
	});

	it('drops a planned route the stored list no longer holds, and answers nothing without a result', () => {
		expect(routeRows(result([route({ index: 3 })]), [STORED])).toEqual([]);
		expect(routeRows(null, [STORED])).toEqual([]);
	});
});

describe('stored routes', () => {
	it('asks the forecast for the weapon on the maturity', () => {
		expect(routePick(STORED)).toEqual({
			setup: { weapon: { catalogId: 'sword' } },
			species: 'Carabok',
			maturity: 'Puny',
		});
	});

	it('recognises only a well-formed stored route', () => {
		expect(isStoredRoute(STORED)).toBe(true);
		expect(isStoredRoute({ ...STORED, weaponId: 7 })).toBe(false);
		expect(isStoredRoute(null)).toBe(false);
	});

	it('says why a route does not answer', () => {
		expect(routeStatusLabel('ready')).toBe('');
		expect(routeStatusLabel('unknown_weapon')).toBe('Weapon not in the catalogue');
		expect(routeStatusLabel('no_evidence')).toBe('Nothing recorded');
	});
});
