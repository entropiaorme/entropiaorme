/**
 * Codex meta-target view model: every species short of rank 25, ranked
 * by the cost of its next meta reward (the backend's order), narrowed to
 * one planet on request. The player can hide species they cannot hunt
 * (event-only or unreachable ones): a hidden species trails the ranking
 * with a restore control. The planet choice and the hidden set are both
 * UI preferences, so a planet-by-planet campaign survives app restarts.
 */

import { getCodexMetaTargets } from '$lib/api';
import type { CodexMetaTarget, CodexMetaTargets } from '$lib/api/commands.gen';
import { getPreference, setPreference } from '$lib/preferences';
import { describeError } from '$lib/view/errorState';
import { createHiddenSet, orderByVisibility } from '$lib/view/hiddenSet.svelte';

const HIDDEN_PREFERENCE = 'codex_hidden_species';
const PLANET_PREFERENCE = 'codex_meta_target_planet';

/** The planet filter's "every planet" choice. */
export const ALL_PLANETS = '';

/** Every planet the targets live on, sorted. */
export function planetsOf(targets: readonly CodexMetaTarget[]): string[] {
	return [...new Set(targets.flatMap((target) => target.planets))].sort((a, b) =>
		a.localeCompare(b),
	);
}

/** The targets on one planet (a species on several planets counts on
 * each), or all of them for `ALL_PLANETS`. */
export function onPlanet(targets: readonly CodexMetaTarget[], planet: string): CodexMetaTarget[] {
	if (planet === ALL_PLANETS) return [...targets];
	return targets.filter((target) => target.planets.includes(planet));
}

export function createCodexMetaTargetsModel() {
	let result = $state<CodexMetaTargets | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let chosenPlanet = $state(ALL_PLANETS);
	const hidden = createHiddenSet(HIDDEN_PREFERENCE);

	const targets = $derived(result?.targets ?? []);
	const planets = $derived(planetsOf(targets));
	// A saved planet the ranking no longer reaches (its last species done,
	// or a game-data change) reads as every planet rather than an empty list.
	const planet = $derived(planets.includes(chosenPlanet) ? chosenPlanet : ALL_PLANETS);
	const rows = $derived(
		orderByVisibility(onPlanet(targets, planet), (target) => hidden.has(target.speciesName)),
	);
	const visibleCount = $derived(rows.filter((row) => !hidden.has(row.speciesName)).length);

	async function loadPlanet() {
		const saved = await getPreference<unknown>(PLANET_PREFERENCE, ALL_PLANETS);
		if (typeof saved === 'string') chosenPlanet = saved;
	}

	async function load() {
		loading = true;
		error = null;
		try {
			const [loaded] = await Promise.all([getCodexMetaTargets(), hidden.load(), loadPlanet()]);
			result = loaded;
		} catch (e) {
			error = describeError(e, 'Failed to load the codex meta targets');
		} finally {
			loading = false;
		}
	}

	function setPlanet(next: string) {
		chosenPlanet = next;
		void setPreference(PLANET_PREFERENCE, next);
	}

	return {
		get result() {
			return result;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get planets() {
			return planets;
		},
		/** The planet the list is narrowed to, or `ALL_PLANETS`. */
		get planet() {
			return planet;
		},
		/** The ranked targets on the chosen planet, hidden species last. */
		get rows() {
			return rows;
		},
		/** How many of `rows` are not hidden. */
		get visibleCount() {
			return visibleCount;
		},

		load,
		setPlanet,
		isHidden: hidden.has,
		hide: hidden.hide,
		restore: hidden.restore,
	};
}

export type CodexMetaTargetsModel = ReturnType<typeof createCodexMetaTargetsModel>;
