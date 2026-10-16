// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillingForecastResult, SkillingForecastSource } from '$lib/api/commands.gen';
import type { ProfessionLevel } from '$lib/types/analytics';
import { forecastResult, sourceForecast } from './__fixtures__/skilling';
import SkillingSessions from './SkillingSessions.svelte';
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

const TREES = (overrides: Partial<SkillingForecastSource> = {}) =>
	sourceForecast({
		definitionId: 3,
		name: 'Tree Cutting',
		cycledPed: 252.43,
		hours: 9.6,
		lootTt: 211.05,
		kills: null,
		ttCost: 41.38,
		skills: [
			{
				name: 'Analysis',
				currentLevel: 1380.02,
				pesShare: 0.143,
				levelGain: 172.77,
				endLevel: 1552.79,
				targetGain: 0.086,
				movesTarget: true,
			},
			{
				name: 'Botany',
				currentLevel: 200,
				pesShare: 0.2,
				levelGain: 160.95,
				endLevel: 360.95,
				targetGain: 0,
				movesTarget: false,
			},
		],
		...overrides,
	});

async function renderWith(overrides: Partial<SkillingForecastResult>) {
	prefs.getPreference.mockResolvedValue(null);
	prefs.setPreference.mockResolvedValue(undefined);
	mocked.getSkillingForecast.mockResolvedValue(
		forecastResult({ current: 40.79, goal: 41, ...overrides }),
	);
	const hub = createSkillingModel({ professions: () => PROFESSIONS, hp: () => 188 });
	hub.setTarget({ kind: 'profession', name: 'Animal Looter' });
	await new Promise((resolve) => setTimeout(resolve, 0));
	render(SkillingSessions, { hub, professions: PROFESSIONS });
	return hub;
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('SkillingSessions', () => {
	it('carries each named session to the goal, expected and as recorded, in the table', async () => {
		await renderWith({
			sources: [
				TREES({ expected: { ttCost: 30.5, weaponReturn: 0.92, coverage: 1 } }),
				sourceForecast({
					definitionId: 4,
					name: 'Carabok Skilling',
					ttCost: 20,
					expected: { ttCost: 39.12, weaponReturn: 0.9, coverage: 1 },
					hours: 2,
				}),
			],
		});
		expect(screen.getByText('252')).toBeTruthy();
		expect(screen.getByText('30.50')).toBeTruthy();
		expect(screen.getByText('41.38')).toBeTruthy();
		expect(screen.getByText('9.6h')).toBeTruthy();
		expect(screen.getByText('Cheapest')).toBeTruthy();
		// The expected losses compare: 8.62 more over the two hours of that
		// session, though its recorded loss is the lower.
		expect(screen.getByText('+8.62')).toBeTruthy();
		expect(screen.getByText('4.31/h')).toBeTruthy();
	});

	it('names the expected return beside the recorded one, with what it covers', async () => {
		await renderWith({
			sources: [TREES({ expected: { ttCost: 30.5, weaponReturn: 0.9216, coverage: 0.8 } })],
		});
		expect(screen.getByText(/92\.2% expected on\s+80% of\s+weapon spend at/)).toBeTruthy();
		// The looter basis it is read at is the shared picker.
		expect(screen.getByRole('button', { name: /Looter/ })).toBeTruthy();
	});

	it('dashes the expected loss the model cannot price', async () => {
		await renderWith({ sources: [TREES()] });
		expect(screen.getByText('no expected return modelled')).toBeTruthy();
	});

	it('shows every skill the session trains, dashing those the goal does not count', async () => {
		await renderWith({ sources: [TREES()] });
		expect(screen.getByText('Analysis')).toBeTruthy();
		expect(screen.getByText('Botany')).toBeTruthy();
		expect(screen.getByText('+0.086')).toBeTruthy();
		expect(screen.getByText(/90\.0% loot return/)).toBeTruthy();
		expect(screen.getByText('no loot sales recorded')).toBeTruthy();
	});

	it('names the realised markup when the loot has sold', async () => {
		await renderWith({
			sources: [
				TREES({
					sample: { ...TREES().sample, realisedMarkup: 12.5, markupLift: 0.0181 },
					markup: 3.82,
					netCost: 37.56,
				}),
			],
		});
		expect(screen.getByText('37.56')).toBeTruthy();
		expect(screen.getByText('+1.8% markup realised from sales')).toBeTruthy();
	});

	it('says why a session cannot answer, and flags a thin sample', async () => {
		await renderWith({
			sources: [
				TREES({ status: 'does_not_train', skills: [] }),
				sourceForecast({ definitionId: 5, name: 'Bank Robbers', warnings: ['thin_sessions'] }),
			],
		});
		expect(screen.getByText('Tree Cutting does not train Animal Looter.')).toBeTruthy();
		expect(screen.getByText('thin sample')).toBeTruthy();
	});

	it('narrows the sessions to a chosen weapon', async () => {
		const hub = await renderWith({
			sources: [TREES()],
			sessionWeapons: [{ name: 'Laconian Sword', kills: 52000 }],
		});
		await fireEvent.change(screen.getByLabelText('Weapon to narrow the sessions to'), {
			target: { value: 'Laconian Sword' },
		});
		expect(hub.sessionWeapon).toBe('Laconian Sword');
		expect(mocked.getSkillingForecast).toHaveBeenLastCalledWith(
			expect.objectContaining({ sessionWeapon: 'Laconian Sword' }),
		);
	});
});
