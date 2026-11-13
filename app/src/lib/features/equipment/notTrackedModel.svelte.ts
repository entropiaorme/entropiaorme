/**
 * Runes-native state for the names the player set aside as not a tool they
 * track: read while the held tool is read off the screen, and each brought
 * back in one action, after which the reader matches it again. A read that
 * lands after a newer one is dropped; while one name is being brought back
 * every other action waits.
 */

import { type DismissedToolName, getDismissedTools, trackToolAgain } from '$lib/api';
import { describeError } from '$lib/view/errorState';

export function createNotTrackedModel() {
	let names = $state<DismissedToolName[]>([]);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let generation = 0;

	return {
		get names() {
			return names;
		},
		get error() {
			return error;
		},
		get busy() {
			return busy;
		},
		async refresh() {
			const run = ++generation;
			try {
				const read = await getDismissedTools();
				if (run === generation) names = read;
			} catch (e) {
				if (run === generation) error = describeError(e, 'Could not load what is not tracked.');
			}
		},
		/** Bring `name` back: the reader matches it again from its next look. */
		async trackAgain(name: string) {
			if (busy) return;
			busy = true;
			error = null;
			const run = ++generation;
			try {
				const left = await trackToolAgain(name);
				if (run === generation) names = left;
			} catch (e) {
				error = describeError(e, 'Could not bring it back.');
			} finally {
				busy = false;
			}
		},
	};
}

export type NotTrackedModel = ReturnType<typeof createNotTrackedModel>;
