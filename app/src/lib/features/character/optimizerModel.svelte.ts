/**
 * Optimiser view model: the profession selection shared with the prospect
 * forecast, the profession-level lookup, the HP optimiser load, and the
 * skilling-path load. Presentation lives in the feature components; they
 * compose over this state. Failures land in the page-level error slot the
 * character model shares across the surface.
 */

import { getHpOptimizer, getProfessionOptimizer, getProfessionPathOptimizer } from '$lib/api';
import type {
	HpOptimizerAttribute,
	HpOptimizerSkill,
	PathOptimizerResult,
} from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import type { PageErrorSlot } from './errorSlot.svelte';

export type { PageErrorSlot };

export type OptimizerMode = 'profession' | 'hp';

export function createOptimizerModel(errors: PageErrorSlot) {
	let mode = $state<OptimizerMode>('profession');
	let selectedProfession = $state('');
	let profLevel = $state(0);

	// ── HP optimiser ──
	let hpSkills = $state([] as HpOptimizerSkill[]);
	let hpAttributes = $state([] as HpOptimizerAttribute[]);
	let hpCurrent = $state(0);
	let hpLoading = $state(false);

	// ── Path optimiser ──
	let pathTargetInput = $state('');
	let pathResult = $state<PathOptimizerResult | null>(null);
	let pathLoading = $state(false);

	async function loadOptimizer(profName: string) {
		errors.error = null;
		if (!profName) {
			profLevel = 0;
			return;
		}
		try {
			const result = await getProfessionOptimizer(profName);
			profLevel = result.currentLevel ?? 0;
		} catch (e) {
			profLevel = 0;
			errors.error = describeError(e, 'Failed to load the profession level');
		}
	}

	async function loadHpOptimizer() {
		hpLoading = true;
		errors.error = null;
		try {
			const result = await getHpOptimizer();
			hpSkills = result.skills || [];
			hpAttributes = result.attributes || [];
			hpCurrent = result.currentHp ?? 0;
		} catch (e) {
			// A failed load still lands on empty lists so the view renders empty
			// rather than stale.
			hpSkills = [];
			hpAttributes = [];
			errors.error = describeError(e, 'Failed to load the HP optimiser');
		} finally {
			hpLoading = false;
		}
	}

	async function loadPathOptimizer() {
		errors.error = null;
		if (!selectedProfession) return;
		const target = parseFloat(pathTargetInput);
		if (Number.isNaN(target) || target <= 0) return;
		pathLoading = true;
		pathResult = null;
		try {
			pathResult = await getProfessionPathOptimizer([selectedProfession], { targetLevel: target });
		} catch (e) {
			pathResult = null;
			errors.error = describeError(e, 'Failed to compute the skilling path');
		} finally {
			pathLoading = false;
		}
	}

	return {
		get mode() {
			return mode;
		},
		set mode(value: OptimizerMode) {
			mode = value;
		},
		get selectedProfession() {
			return selectedProfession;
		},
		set selectedProfession(value: string) {
			selectedProfession = value;
		},
		get profLevel() {
			return profLevel;
		},

		// ── HP optimiser ──
		get hpSkills() {
			return hpSkills;
		},
		get hpAttributes() {
			return hpAttributes;
		},
		get hpCurrent() {
			return hpCurrent;
		},
		get hpLoading() {
			return hpLoading;
		},

		// ── Path optimiser ──
		get pathTargetInput() {
			return pathTargetInput;
		},
		set pathTargetInput(value: string) {
			pathTargetInput = value;
		},
		get pathResult() {
			return pathResult;
		},
		set pathResult(value: PathOptimizerResult | null) {
			pathResult = value;
		},
		get pathLoading() {
			return pathLoading;
		},

		loadOptimizer,
		loadHpOptimizer,
		loadPathOptimizer,
	};
}

export type OptimizerModel = ReturnType<typeof createOptimizerModel>;
