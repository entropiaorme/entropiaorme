// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillingForecastSource } from '$lib/api/commands.gen';
import type { ProfessionLevel } from '$lib/types/analytics';
import { NO_DATA } from '$lib/utils/format';
import SkillingForecast from './SkillingForecast.svelte';
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

const mocked = vi.mocked(api);

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

async function renderWith(sources: SkillingForecastSource[]) {
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
	mocked.getSkillingForecast.mockResolvedValue({ current: 40.79, goal: 41, sources });
	const hub = createSkillingModel({ professions: () => PROFESSIONS, hp: () => 188 });
	hub.setTarget({ kind: 'profession', name: 'Animal Looter' });
	await new Promise((resolve) => setTimeout(resolve, 0));
	render(SkillingForecast, { hub });
	return hub;
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('SkillingForecast', () => {
	it('reads the chosen session to the goal, with no markup until something sells', async () => {
		await renderWith([source()]);
		expect(screen.getByText('252.43')).toBeTruthy();
		expect(screen.getByText('9.6h')).toBeTruthy();
		expect(screen.getByText('41.38')).toBeTruthy();
		// No confirmed sales: the after-markup slot is empty, never a copy of TT.
		expect(screen.getByText(NO_DATA)).toBeTruthy();
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

	it('says why a session cannot answer', async () => {
		await renderWith([source({ status: 'does_not_train', skills: [] })]);
		expect(screen.getByText('Tree Cutting does not train Animal Looter.')).toBeTruthy();
	});

	it('points at tracking when no named session exists', async () => {
		await renderWith([]);
		expect(screen.getByText(/No named sessions recorded yet/)).toBeTruthy();
	});

	it('flags a thin sample with the figure behind it', async () => {
		await renderWith([
			source({ warnings: ['thin_sessions'], sample: { ...source().sample, sessions: 2 } }),
		]);
		expect(screen.getByText('Thin sample')).toBeTruthy();
		expect(screen.getByText('only 2 sessions recorded.')).toBeTruthy();
	});
});
