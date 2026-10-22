// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { HpOptimizerAllocation, HpOptimizerResult } from '$lib/api/commands.gen';
import { forecastResult } from './__fixtures__/skilling';
import SkillingPath from './SkillingPath.svelte';
import { createSkillingModel, GOAL_DEBOUNCE_MS } from './skillingModel.svelte';

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

function row(overrides: Partial<HpOptimizerAllocation> = {}): HpOptimizerAllocation {
	return {
		name: 'Athletics',
		levelsPerHp: 533.3,
		currentLevel: 3000,
		levelsToGain: 533.3,
		newLevel: 3533.3,
		hpGain: 1,
		pedCost: 6.4,
		codexCategory: 'cat1',
		codexDivisor: 200,
		...overrides,
	};
}

function hpPath(overrides: Partial<HpOptimizerResult> = {}): HpOptimizerResult {
	return {
		targetHp: 189,
		currentHp: 188.4,
		endHp: 189,
		totalPed: 6.4,
		allocations: [row()],
		attributes: [{ name: 'Stamina', currentLevel: 40, levelsPerHp: 9.25 }],
		excluded: [],
		...overrides,
	};
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

async function renderHp(first: HpOptimizerResult) {
	mocked.getHpOptimizer.mockResolvedValue(first);
	const hub = createSkillingModel({ professions: () => [], hp: () => 188 });
	hub.setTarget({ kind: 'hp' });
	hub.setView('optimiser');
	await settle();
	render(SkillingPath, { hub });
	return hub;
}

beforeEach(() => {
	vi.clearAllMocks();
	prefs.getPreference.mockResolvedValue(null);
	prefs.setPreference.mockResolvedValue(undefined);
	mocked.getSkillingForecast.mockResolvedValue(forecastResult({ current: 188, goal: 189 }));
});

describe('SkillingPath for HP', () => {
	it('prices the next HP by default', async () => {
		await renderHp(hpPath());
		expect(mocked.getHpOptimizer).toHaveBeenCalledWith(189);
		expect(screen.getByText(/reaches 189 HP, over 1\s+skill\./)).toBeTruthy();
		expect(screen.getByText('Athletics')).toBeTruthy();
		expect(screen.getByText('Levels per HP')).toBeTruthy();
		expect(screen.getByText('3,533.30')).toBeTruthy();
	});

	it('re-answers when the goal is raised, spreading across more skills', async () => {
		const hub = await renderHp(hpPath());
		mocked.getHpOptimizer.mockResolvedValue(
			hpPath({
				targetHp: 200,
				endHp: 200,
				totalPed: 120.5,
				allocations: [
					row({ levelsToGain: 4800, newLevel: 7800, hpGain: 9, pedCost: 98.2 }),
					row({
						name: 'Handgun',
						levelsPerHp: 1600,
						currentLevel: 2500,
						levelsToGain: 4160,
						newLevel: 6660,
						hpGain: 2.6,
						pedCost: 22.3,
					}),
				],
			}),
		);
		hub.setGoal('200');
		await new Promise((resolve) => setTimeout(resolve, GOAL_DEBOUNCE_MS + 10));
		await settle();
		expect(mocked.getHpOptimizer).toHaveBeenLastCalledWith(200);
		expect(await screen.findByText(/reaches 200 HP, over 2\s+skills\./)).toBeTruthy();
		expect(screen.getByText('Handgun')).toBeTruthy();
		expect(screen.getByText('7,800.00')).toBeTruthy();
	});

	it('says so when every skill tops out short of the goal', async () => {
		// Rendering only: the fixture tops out just below the default goal.
		await renderHp(hpPath({ endHp: 188.9 }));
		expect(
			screen.getByText(/tops every skill out at 188\.90 HP, short of\s+189 HP\./),
		).toBeTruthy();
	});

	it('asks for a scan when no skill that adds HP is unlocked', async () => {
		await renderHp(hpPath({ allocations: [], endHp: 188.4, totalPed: 0 }));
		expect(screen.getByText('Scan your skills first.')).toBeTruthy();
		expect(screen.queryByText(/short of/)).toBeNull();
	});
});
