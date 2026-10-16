// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillingForecastResult, SkillingRouteForecast } from '$lib/api/commands.gen';
import type { ProfessionLevel } from '$lib/types/analytics';
import { forecastResult, routeForecast } from './__fixtures__/skilling';
import SkillingRoutes from './SkillingRoutes.svelte';
import { createSkillingModel } from './skillingModel.svelte';

vi.mock('$lib/api', () => ({
	getActivityRecommender: vi.fn(),
	getProfessionPathOptimizer: vi.fn(),
	getHpOptimizer: vi.fn(),
	getSkillingForecast: vi.fn(),
	searchEquipmentItems: vi.fn(),
	getManualMobSuggestions: vi.fn(),
}));

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import * as api from '$lib/api';
import * as preferences from '$lib/preferences';

const mocked = vi.mocked(api);
const prefs = vi.mocked(preferences);

const PROFESSIONS: ProfessionLevel[] = [
	{
		name: 'Swordsman (Hit)',
		level: 48.44,
		anchorLevel: null,
		gainSinceAnchor: null,
		category: 'Combat',
	},
];

const ROBBER = {
	weaponId: 'sword',
	weaponName: 'Laconian Sword',
	species: 'Bank Robber',
	maturity: 'Default',
	display: 'Default Bank Robber',
};
const CARABOK = { ...ROBBER, species: 'Carabok', maturity: 'Puny', display: 'Puny Carabok' };

const robber = (overrides: Partial<SkillingRouteForecast> = {}) =>
	routeForecast({
		mob: 'Default Bank Robber',
		mobHp: 2,
		skillPerKillPec: 0.4906,
		cycledPerKillPec: 2.1667,
		lossPerKillPec: 0.1699,
		secondsPerKill: 2,
		kills: 12000,
		cycledPed: 260,
		pedLost: 20.39,
		hours: 6.67,
		...overrides,
	});

async function renderWith(
	routes: SkillingRouteForecast[],
	stored = [ROBBER],
	overrides: Partial<SkillingForecastResult> = {},
) {
	prefs.getPreference.mockImplementation(async (key: string) =>
		key === 'skilling_routes' ? stored : null,
	);
	prefs.setPreference.mockResolvedValue(undefined);
	mocked.getSkillingForecast.mockResolvedValue(
		forecastResult({ current: 48.44, goal: 49, routes, ...overrides }),
	);
	const hub = createSkillingModel({ professions: () => PROFESSIONS, hp: () => 188 });
	await hub.restore();
	hub.setTarget({ kind: 'profession', name: 'Swordsman (Hit)' });
	await new Promise((resolve) => setTimeout(resolve, 0));
	render(SkillingRoutes, { hub, professions: PROFESSIONS });
	return hub;
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('SkillingRoutes', () => {
	it('prices each route in the table, with the premium over the cheapest', async () => {
		await renderWith(
			[
				robber(),
				routeForecast({ index: 1, cycledPed: 900, pedLost: 41.38, hours: 9.6, kills: 9000 }),
			],
			[ROBBER, CARABOK],
		);
		// Cycled, TT lost, time, and kills sit in the row.
		expect(screen.getByText('260')).toBeTruthy();
		expect(screen.getByText('20.39')).toBeTruthy();
		expect(screen.getByText('6.7h')).toBeTruthy();
		expect(screen.getByText('12,000')).toBeTruthy();
		expect(screen.getByText('Cheapest')).toBeTruthy();
		// 20.99 more over 9.6 hours of that route.
		expect(screen.getByText('+20.99')).toBeTruthy();
		expect(screen.getByText('2.19/h')).toBeTruthy();
	});

	it('details the cheapest route by what each kill does', async () => {
		await renderWith([robber()]);
		expect(screen.getByText('Laconian Sword on Default Bank Robber')).toBeTruthy();
		expect(screen.getByText('0.49 PEC of skill')).toBeTruthy();
		expect(screen.getByText('2.17 PEC cycled')).toBeTruthy();
		// Two seconds a kill: one of firing and the one-second allowance.
		expect(screen.getByText(/2\.0 s \(1\.0 s firing, 1\.0 s to the next mob\)/)).toBeTruthy();
	});

	it("shows each route's overkill and the most a better fit could save", async () => {
		await renderWith(
			[
				robber({ overkillShare: 0.6732, pedLost: 2160 }),
				routeForecast({ index: 1, overkillShare: 0.22, pedLost: 3770 }),
			],
			[ROBBER, CARABOK],
		);
		expect(screen.getByText('67%')).toBeTruthy();
		expect(screen.getByText('up to 1,454 PED')).toBeTruthy();
		expect(screen.getByText('22%')).toBeTruthy();
		expect(screen.getByText('up to 829 PED')).toBeTruthy();
		// The cheapest route is selected: its overkill reads as headroom.
		expect(
			screen.getByText(/A lighter weapon on Default Bank Robber could save up to 1,454 PED/),
		).toBeTruthy();
		expect(screen.getByText(/4\.1 HP dealt past the kill/)).toBeTruthy();
	});

	it('says whether overkill is why a dearer route loses', async () => {
		const hub = await renderWith(
			[
				robber({ overkillShare: 0.1, pedLost: 2160 }),
				routeForecast({ index: 1, overkillShare: 0.22, pedLost: 3770 }),
			],
			[ROBBER, CARABOK],
		);
		hub.selectRow('route:1');
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(
			screen.getByText(/Overkill accounts for 829 PED of this route's 1,610 PED premium/),
		).toBeTruthy();
	});

	it('says when a better-fitted weapon could make a dearer route the cheapest', async () => {
		const hub = await renderWith(
			[
				robber({ overkillShare: 0.1, pedLost: 2160 }),
				routeForecast({ index: 1, overkillShare: 0.6, pedLost: 3000 }),
			],
			[ROBBER, CARABOK],
		);
		hub.selectRow('route:1');
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(screen.getByText(/Overkill costs more than this route's 840 PED premium/)).toBeTruthy();
		expect(
			screen.getByText(/would lose 1,200 PED, against the cheapest route's 2,160/),
		).toBeTruthy();
	});

	it('takes the time to the next mob from your recorded pace when there is one', async () => {
		await renderWith([
			robber({ secondsPerKill: 1.6, pace: { betweenKillSeconds: 0.6, kills: 5000 } }),
		]);
		expect(screen.getByText(/1\.6 s \(1\.0 s firing, 0\.6 s to the next mob\)/)).toBeTruthy();
	});

	it('shows the boost the routes were priced at', async () => {
		await renderWith([robber()]);
		expect((screen.getByLabelText('Skill boost percent') as HTMLInputElement).value).toBe('50');
	});

	it('leaves a target hunting only trains as a byproduct unpriced', async () => {
		await renderWith([], [ROBBER], { huntingTarget: false });
		expect(screen.getByText(/only as a byproduct of a weapon's own professions/)).toBeTruthy();
		expect(screen.queryByLabelText('Skill boost percent')).toBeNull();
		expect(screen.queryByText('Laconian Sword')).toBeNull();
	});

	it('names why a route cannot be priced, and removes it', async () => {
		const hub = await renderWith([
			robber({ status: 'unknown_mob', mobHp: null, mobLevel: null, pedLost: 0, hours: null }),
		]);
		expect(screen.getByText('No HP or level for this mob')).toBeTruthy();
		await fireEvent.click(screen.getByLabelText('Remove Laconian Sword on Default Bank Robber'));
		expect(hub.routes).toEqual([]);
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_routes', []);
	});
});
