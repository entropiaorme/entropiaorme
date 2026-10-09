import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CodexMetaTarget, CodexMetaTargets } from '$lib/api/commands.gen';
import {
	ALL_PLANETS,
	createCodexMetaTargetsModel,
	onPlanet,
	planetsOf,
} from './codexMetaTargetsModel.svelte';

vi.mock('$lib/api', () => ({
	getCodexMetaTargets: vi.fn(),
}));

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import * as api from '$lib/api';
import * as preferences from '$lib/preferences';

const mocked = vi.mocked(api);
const prefs = vi.mocked(preferences);

function target(overrides: Partial<CodexMetaTarget> = {}): CodexMetaTarget {
	return {
		speciesName: 'Atrox',
		planets: ['Calypso'],
		currentRank: 0,
		targetRank: 5,
		cost: 112,
		estimatedPed: 168,
		...overrides,
	};
}

function ranking(targets: CodexMetaTarget[]): CodexMetaTargets {
	return { metasEarned: 3, metaCap: 750, pedPerCost: 1.5, targets };
}

const RANKED = [
	target({ speciesName: 'Snablesnot', planets: ['Calypso'] }),
	target({ speciesName: 'Mermoth', planets: ['Arkadia', 'Calypso'] }),
	target({ speciesName: 'Kerberos', planets: ['Cyrene'] }),
	target({ speciesName: 'Easter Robots', planets: [] }),
];

const names = (list: CodexMetaTarget[]) => list.map((row) => row.speciesName);

/** Preferences keyed by name, so the hidden set and planet read their own. */
function savedPreferences(saved: Record<string, unknown>) {
	prefs.getPreference.mockImplementation(async (key, fallback) =>
		key in saved ? (saved[key] as never) : fallback,
	);
}

beforeEach(() => {
	vi.clearAllMocks();
	savedPreferences({});
	prefs.setPreference.mockResolvedValue(undefined);
});

describe('planet helpers', () => {
	it('collects every planet once, sorted', () => {
		expect(planetsOf(RANKED)).toEqual(['Arkadia', 'Calypso', 'Cyrene']);
		expect(planetsOf([])).toEqual([]);
	});

	it('narrows to a planet, counting a multi-planet species on each', () => {
		expect(names(onPlanet(RANKED, 'Calypso'))).toEqual(['Snablesnot', 'Mermoth']);
		expect(names(onPlanet(RANKED, 'Arkadia'))).toEqual(['Mermoth']);
		expect(names(onPlanet(RANKED, ALL_PLANETS))).toEqual(names(RANKED));
	});
});

describe('createCodexMetaTargetsModel', () => {
	it('loads the ranking in the backend order over every planet by default', async () => {
		mocked.getCodexMetaTargets.mockResolvedValue(ranking(RANKED));
		const model = createCodexMetaTargetsModel();
		expect(model.loading).toBe(true);
		await model.load();
		expect(model.loading).toBe(false);
		expect(model.error).toBeNull();
		expect(model.result?.metasEarned).toBe(3);
		expect(model.planet).toBe(ALL_PLANETS);
		expect(model.planets).toEqual(['Arkadia', 'Calypso', 'Cyrene']);
		expect(names(model.rows)).toEqual(names(RANKED));
		expect(model.visibleCount).toBe(4);
	});

	it('narrows to the chosen planet and saves the choice', async () => {
		mocked.getCodexMetaTargets.mockResolvedValue(ranking(RANKED));
		const model = createCodexMetaTargetsModel();
		await model.load();
		model.setPlanet('Calypso');
		expect(model.planet).toBe('Calypso');
		expect(names(model.rows)).toEqual(['Snablesnot', 'Mermoth']);
		expect(prefs.setPreference).toHaveBeenLastCalledWith('codex_meta_target_planet', 'Calypso');

		model.setPlanet(ALL_PLANETS);
		expect(names(model.rows)).toEqual(names(RANKED));
	});

	it('restores a saved planet, and reads one the ranking no longer reaches as every planet', async () => {
		savedPreferences({ codex_meta_target_planet: 'Cyrene' });
		mocked.getCodexMetaTargets.mockResolvedValue(ranking(RANKED));
		const restored = createCodexMetaTargetsModel();
		await restored.load();
		expect(restored.planet).toBe('Cyrene');
		expect(names(restored.rows)).toEqual(['Kerberos']);

		savedPreferences({ codex_meta_target_planet: 'Monria' });
		const stale = createCodexMetaTargetsModel();
		await stale.load();
		expect(stale.planet).toBe(ALL_PLANETS);
		expect(names(stale.rows)).toEqual(names(RANKED));
	});

	it('trails hidden species within the planet, restorable, and saves the set', async () => {
		savedPreferences({ codex_hidden_species: ['Easter Robots'] });
		mocked.getCodexMetaTargets.mockResolvedValue(ranking(RANKED));
		const model = createCodexMetaTargetsModel();
		await model.load();
		expect(names(model.rows)).toEqual(['Snablesnot', 'Mermoth', 'Kerberos', 'Easter Robots']);
		expect(model.visibleCount).toBe(3);

		model.hide('Snablesnot');
		expect(names(model.rows)).toEqual(['Mermoth', 'Kerberos', 'Snablesnot', 'Easter Robots']);
		expect(model.isHidden('Snablesnot')).toBe(true);
		expect(prefs.setPreference).toHaveBeenLastCalledWith('codex_hidden_species', [
			'Easter Robots',
			'Snablesnot',
		]);

		model.setPlanet('Calypso');
		expect(names(model.rows)).toEqual(['Mermoth', 'Snablesnot']);
		expect(model.visibleCount).toBe(1);

		model.restore('Snablesnot');
		expect(names(model.rows)).toEqual(['Snablesnot', 'Mermoth']);
	});

	it('reports a failed load in its error slot', async () => {
		mocked.getCodexMetaTargets.mockRejectedValue(new Error('backend down'));
		const model = createCodexMetaTargetsModel();
		await model.load();
		expect(model.loading).toBe(false);
		expect(model.error).toBe('backend down');
		expect(model.rows).toEqual([]);
	});
});
