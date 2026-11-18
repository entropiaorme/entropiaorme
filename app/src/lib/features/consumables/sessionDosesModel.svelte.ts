/**
 * Runes-native state for a session's doses in its record: every dose taken
 * in the session, removed ones included, with removal and exact restore.
 *
 * A removal or restore moves the session's dose cost, which the backend
 * announces with `consumables:updated`; the session record re-reads its
 * detail on that frame and asks this model to re-read too. Every correction
 * is undoable, so none asks for confirmation.
 */

import {
	type ConsumableDose,
	getSessionDoses,
	removeConsumableDose,
	restoreConsumableDose,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';

export function createSessionDosesModel(sessionId: () => string) {
	let doses = $state<ConsumableDose[]>([]);
	let loaded = $state(false);
	let busy = $state<string | null>(null);
	let error = $state<string | null>(null);
	// Guards against a slow read landing after the session moved on.
	let generation = 0;

	async function refresh() {
		const asked = sessionId();
		const run = ++generation;
		try {
			const fresh = await getSessionDoses(asked);
			if (run !== generation) return;
			doses = fresh;
			loaded = true;
		} catch (e) {
			if (run === generation) error = describeError(e, 'Could not load the doses.');
		}
	}

	async function correct(dose: ConsumableDose, restore: boolean) {
		if (busy) return;
		busy = dose.id;
		error = null;
		try {
			if (restore) await restoreConsumableDose(dose.id);
			else await removeConsumableDose(dose.id);
			await refresh();
		} catch (e) {
			error = describeError(
				e,
				restore ? 'Could not restore the dose.' : 'Could not remove the dose.',
			);
		} finally {
			busy = null;
		}
	}

	return {
		get doses() {
			return doses;
		},
		get loaded() {
			return loaded;
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		/** The cost the session's standing doses booked, PED. */
		get bookedPed() {
			return doses
				.filter((dose) => dose.removedAt === null)
				.reduce((sum, dose) => sum + dose.costPed, 0);
		},
		refresh,
		remove: (dose: ConsumableDose) => correct(dose, false),
		restore: (dose: ConsumableDose) => correct(dose, true),
		dismissError() {
			error = null;
		},
	};
}

export type SessionDosesModel = ReturnType<typeof createSessionDosesModel>;
