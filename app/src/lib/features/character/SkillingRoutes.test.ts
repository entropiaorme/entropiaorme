// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillingForecastSource, SkillingRouteForecast } from '$lib/api/commands.gen';
import type { ProfessionLevel } from '$lib/types/analytics';
import SkillingRoutes from './SkillingRoutes.svelte';
import { createSkillingModel } from './skillingModel.svelte';

vi.mock('$lib/api', () => ({
	getActivityRecommender: vi.fn(),
	getProfessionPathOptimizer: vi.fn(),
	getHpOptimizer: vi.fn(),
	getSkillingForecast: vi.fn(),
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
		name: 'Animal Looter',
		level: 40.79,
		anchorLevel: null,
		gainSinceAnchor: null,
		category: 'Looting',
	},
];

function source(overrides: Partial<SkillingForecastSource> = {}): SkillingForecastSource {
	return {
		definitionId: 3,
		name: 'Tree Cutting',
		archived: false,
		sample: {
			sessions: 38,
			hours: 31,
			cycledPed: 826.5,
			lootTt: 691,
			pes: 38.29,
			realisedMarkup: null,
			markupLift: null,
		},
		status: 'ready',
		cycledPed: 252.43,
		hours: 9.6,
		lootTt: 211.05,
		markup: null,
		ttCost: 41.38,
		netCost: null,
		skills: [
			{
				name: 'Analysis',
				isAttribute: false,
				currentLevel: 1380.02,
				pesShare: 0.143,
				levelGain: 172.77,
				endLevel: 1552.79,
				targetGain: 0.086,
				movesTarget: true,
			},
			{
				name: 'Skinning',
				isAttribute: false,
				currentLevel: 4992.06,
				pesShare: 0.0001,
				levelGain: 0.03,
				endLevel: 4992.09,
				targetGain: 0.0001,
				movesTarget: true,
			},
			{
				name: 'Botany',
				isAttribute: false,
				currentLevel: 200,
				pesShare: 0.2,
				levelGain: 160.95,
				endLevel: 360.95,
				targetGain: 0,
				movesTarget: false,
			},
		],
		warnings: [],
		...overrides,
	};
}

function planned(overrides: Partial<SkillingRouteForecast> = {}): SkillingRouteForecast {
	return {
		index: 0,
		weapon: 'Laconian Sword',
		mob: 'Default Bank Robber',
		mobHp: 2,
		mobLevel: 4,
		status: 'ready',
		skillPerKillPec: 0.4906,
		cycledPerKillPec: 2.1667,
		lossPerKillPec: 0.1699,
		secondsPerKill: 2,
		kills: 12000,
		cycledPed: 260,
		pedLost: 20.39,
		hours: 6.67,
		measured: null,
		skills: source().skills,
		...overrides,
	};
}

const STORED = {
	weaponId: 'sword',
	weaponName: 'Laconian Sword',
	species: 'Bank Robber',
	maturity: 'Default',
	display: 'Default Bank Robber',
};

async function renderWith(sources: SkillingForecastSource[], routes: SkillingRouteForecast[] = []) {
	prefs.getPreference.mockImplementation(async (key: string) =>
		key === 'skilling_routes' ? routes.map(() => STORED) : null,
	);
	prefs.setPreference.mockResolvedValue(undefined);
	mocked.getActivityRecommender.mockResolvedValue({
		pesCap: 1000,
		sampleStep: 20,
		direct: null,
		candidates: [],
	});
	mocked.getProfessionPathOptimizer.mockResolvedValue({
		allocations: [],
		attributes: [],
		profession: 'Animal Looter',
		mode: 'target',
		inputTargetLevel: 41,
		inputPedBudget: null,
		currentLevel: 40.79,
		endLevel: 41,
		professionLevelsGained: 0.21,
		totalPed: 3.26,
		excluded: [],
	});
	mocked.getSkillingForecast.mockResolvedValue({
		current: 40.79,
		goal: 41,
		sources,
		routes,
		skillBoost: 'fifty',
		looterLevel: 40,
	});
	const hub = createSkillingModel({ professions: () => PROFESSIONS, hp: () => 188 });
	await hub.restore();
	hub.setTarget({ kind: 'profession', name: 'Animal Looter' });
	await new Promise((resolve) => setTimeout(resolve, 0));
	render(SkillingRoutes, { hub, professions: PROFESSIONS });
	return hub;
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('SkillingRoutes', () => {
	it('reads a recorded session to the goal, with no markup until something sells', async () => {
		await renderWith([source()]);
		// The row and the detail both carry the figures.
		expect(screen.getAllByText('252').length).toBe(2);
		expect(screen.getAllByText('9.6h').length).toBe(2);
		expect(screen.getAllByText('41.38').length).toBe(2);
		expect(screen.getByText('Recorded')).toBeTruthy();
		// No confirmed sales: the after-markup slot is empty, never a copy of TT.
		expect(screen.getByText('No sales yet')).toBeTruthy();
		// The skill that moves the target is a row; the negligible one and the
		// off-target one sit behind a tip.
		expect(screen.getByText('Analysis')).toBeTruthy();
		expect(screen.queryByText('Skinning')).toBeNull();
		expect(screen.getByText('+2 other skills trained')).toBeTruthy();
		expect(screen.getByText(/Skinning \+0\.03, Botany \+160\.95/)).toBeTruthy();
	});

	it('applies the session realised markup when it has one', async () => {
		await renderWith([
			source({
				sample: { ...source().sample, realisedMarkup: 12.5, markupLift: 0.0181 },
				markup: 3.82,
				netCost: 37.56,
			}),
		]);
		expect(screen.getByText('37.56')).toBeTruthy();
		expect(screen.getByText('+1.8% realised on loot')).toBeTruthy();
	});

	it('says why a route cannot answer', async () => {
		await renderWith([source({ status: 'does_not_train', skills: [] })]);
		expect(screen.getByText('Does not train this')).toBeTruthy();
		expect(screen.getByText('Tree Cutting does not train Animal Looter.')).toBeTruthy();
	});

	it('asks for a route when there is nothing to price', async () => {
		await renderWith([]);
		expect(screen.getByText(/Add a route \(a weapon on a mob\)/)).toBeTruthy();
	});

	it('flags a thin sample with the figure behind it', async () => {
		await renderWith([
			source({ warnings: ['thin_sessions'], sample: { ...source().sample, sessions: 2 } }),
		]);
		expect(screen.getByText('Thin sample: only 2 sessions recorded.')).toBeTruthy();
	});

	it('prices a planned route beside the sessions, with the premium over the cheapest', async () => {
		// The planned route loses 20.39; the session 41.38 over 9.6h.
		await renderWith([source()], [planned()]);
		// The row and the detail heading both name it.
		expect(screen.getAllByText('Laconian Sword on Default Bank Robber').length).toBe(2);
		expect(screen.getByText('Cheapest')).toBeTruthy();
		expect(screen.getByText('+20.99')).toBeTruthy();
		expect(screen.getByText('2.19/h')).toBeTruthy();
		// The cheapest is shown in detail: its kills and what a kill yields.
		expect(screen.getByText('12,000')).toBeTruthy();
		expect(screen.getByText('0.49 PEC of skill per kill')).toBeTruthy();
		expect(screen.getByText(/Planned routes: community model v1/)).toBeTruthy();
	});

	it('marks a route priced from your own kills', async () => {
		await renderWith([], [planned({ measured: { kills: 51435, sessions: 8 } })]);
		expect(screen.getByText('Measured')).toBeTruthy();
		expect(screen.getByText(/Measured from 51,435 of your kills over 8/)).toBeTruthy();
		expect(screen.getByText('2.0 s per kill, at your recorded pace.')).toBeTruthy();
	});

	it('names why a planned route cannot be priced, and removes it', async () => {
		const hub = await renderWith(
			[],
			[planned({ status: 'unknown_mob', mobHp: null, mobLevel: null, pedLost: 0, hours: null })],
		);
		expect(screen.getAllByText(/No HP or level for this mob/).length).toBeGreaterThan(0);
		await fireEvent.click(screen.getByLabelText('Remove Laconian Sword on Default Bank Robber'));
		expect(hub.routes).toEqual([]);
		expect(prefs.setPreference).toHaveBeenCalledWith('skilling_routes', []);
	});
});
