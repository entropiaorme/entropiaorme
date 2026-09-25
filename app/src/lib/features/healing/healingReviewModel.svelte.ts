/**
 * Runes-native state for a session's healing evidence and its corrections.
 *
 * A correction answers with the session's refreshed detail, which the
 * owner applies; the review list of uncosted heals then re-reads so a heal
 * the correction moved changes group in place. Every correction is
 * undoable, so none asks for confirmation.
 */

import {
	correctHealing,
	getHealingCorrectionTools,
	getHealingOutputs,
	type HealingCorrectionTool,
	type HealingOutput,
	undoHealingCorrection,
} from '$lib/api';
import type { SessionDetail } from '$lib/types/tracking';
import { describeError } from '$lib/view/errorState';
import { reviewFilters, type UncostedClassification } from './healingReview';

/** Uncosted heals fetched per page. */
export const OUTPUT_PAGE = 50;
/** The most rows one in-place re-read asks for (the backend's page cap). */
const REFRESH_LIMIT = 200;

export type ToolsState =
	| { status: 'loading' }
	| { status: 'ready'; tools: HealingCorrectionTool[] }
	| { status: 'error'; message: string };

export interface HealingReviewOptions {
	/** The session detail currently shown. */
	detail: () => SessionDetail;
	/** Replace the shown detail with a correction's refreshed one. */
	apply: (fresh: SessionDetail) => void;
}

export function createHealingReviewModel(options: HealingReviewOptions) {
	let busy = $state<string | null>(null);
	let error = $state<string | null>(null);
	let reviewOpen = $state(false);
	let filter = $state<UncostedClassification>('unattributed');
	let outputs = $state<HealingOutput[]>([]);
	let total = $state(0);
	let loadingOutputs = $state(false);
	let outputsError = $state<string | null>(null);
	let tools = $state<Record<string, ToolsState>>({});
	// Guards against a slow page landing after the filter moved on.
	let generation = 0;

	const healing = $derived(options.detail().healing);
	const filters = $derived(reviewFilters(healing));

	/** Keep the chosen group while it has heals; otherwise the first one
	 * that does. */
	function settleFilter() {
		if (!filters.some((candidate) => candidate.id === filter) && filters.length > 0) {
			filter = filters[0].id;
		}
	}

	async function fetchOutputs(offset: number, limit: number, append: boolean) {
		const sessionId = options.detail().sessionId;
		const asked = filter;
		const run = ++generation;
		loadingOutputs = true;
		outputsError = null;
		try {
			const page = await getHealingOutputs(sessionId, asked, offset, limit);
			if (run !== generation) return;
			outputs = append ? [...outputs, ...page.outputs] : page.outputs;
			total = page.total;
		} catch (e) {
			if (run === generation) outputsError = describeError(e, 'Could not load these heals.');
		} finally {
			if (run === generation) loadingOutputs = false;
		}
	}

	/** Re-read the loaded review rows in place, keeping how far the player
	 * had scrolled into the list. */
	async function reloadOutputs() {
		if (!reviewOpen) return;
		settleFilter();
		const limit = Math.min(Math.max(outputs.length, OUTPUT_PAGE), REFRESH_LIMIT);
		await fetchOutputs(0, limit, false);
	}

	async function act(id: string, run: () => Promise<SessionDetail>) {
		if (busy !== null) return;
		busy = id;
		error = null;
		try {
			options.apply(await run());
			tools = {};
			await reloadOutputs();
		} catch (e) {
			error = describeError(e, 'The correction could not be saved.');
		} finally {
			busy = null;
		}
	}

	return {
		get healing() {
			return healing;
		},
		get filters() {
			return filters;
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		get reviewOpen() {
			return reviewOpen;
		},
		get filter() {
			return filter;
		},
		get outputs() {
			return outputs;
		},
		get total() {
			return total;
		},
		get hasMore() {
			return outputs.length < total;
		},
		get loadingOutputs() {
			return loadingOutputs;
		},
		get outputsError() {
			return outputsError;
		},
		toolsFor(outputId: string): ToolsState | undefined {
			return tools[outputId];
		},
		dismissError() {
			error = null;
		},

		/** Take a billed use back: it was not a paid use. */
		markNotPaid(activationId: string) {
			return act(activationId, () => correctHealing({ kind: 'notPaidUse', activationId }));
		},
		/** Bill an uncosted heal as one paid use of a healing item. */
		markPaid(outputId: string, equipmentId: number) {
			return act(outputId, () => correctHealing({ kind: 'paidUse', outputId, equipmentId }));
		},
		undo(correctionId: string) {
			return act(correctionId, () => undoHealingCorrection(correctionId));
		},

		async toggleReview() {
			reviewOpen = !reviewOpen;
			if (!reviewOpen) return;
			settleFilter();
			await fetchOutputs(0, OUTPUT_PAGE, false);
		},
		async setFilter(next: UncostedClassification) {
			if (next === filter && outputs.length > 0) return;
			filter = next;
			outputs = [];
			total = 0;
			await fetchOutputs(0, OUTPUT_PAGE, false);
		},
		async loadMore() {
			if (loadingOutputs || outputs.length >= total) return;
			await fetchOutputs(outputs.length, OUTPUT_PAGE, true);
		},

		/** Fetch the items a heal could be billed to, once per heal. */
		async loadTools(outputId: string) {
			const known = tools[outputId];
			if (known && known.status !== 'error') return;
			tools = { ...tools, [outputId]: { status: 'loading' } };
			try {
				const offered = await getHealingCorrectionTools(outputId);
				tools = { ...tools, [outputId]: { status: 'ready', tools: offered } };
			} catch (e) {
				tools = {
					...tools,
					[outputId]: {
						status: 'error',
						message: describeError(e, 'Could not load healing items.'),
					},
				};
			}
		},

		/** The detail changed under us (a correction from elsewhere): follow it. */
		refresh() {
			tools = {};
			return reloadOutputs();
		},
	};
}

export type HealingReviewModel = ReturnType<typeof createHealingReviewModel>;
