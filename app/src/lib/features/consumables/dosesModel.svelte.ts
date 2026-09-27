/**
 * Runes-native state for the overlay's live dose readout: the running
 * session's doses read on every `consumables:updated` frame (and when a
 * session starts or stops), a local display tick for the countdowns, and the
 * start, end, remove, and restore actions.
 *
 * The tick is display only: which dose is running, and how long it has left,
 * is always measured from the dose's stored end. A removal can be taken back
 * from the readout for a short while; every removal is also restorable from
 * the session's record, so none asks for confirmation.
 */

import {
	CONSUMABLES_TOPIC,
	type ConsumableDose,
	type ConsumableDoses,
	endConsumableDose,
	getConsumableDoses,
	removeConsumableDose,
	restoreConsumableDose,
	startConsumableDose,
} from '$lib/api';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';
import { liveRows } from './doses';

/** How long the readout offers to take a removal back, seconds. */
export const UNDO_SECONDS = 8;

export interface DosesModelOptions {
	/** The display clock, epoch seconds; injectable for tests. */
	clock?: () => number;
}

export function createDosesModel(options: DosesModelOptions = {}) {
	const clock = options.clock ?? (() => Date.now() / 1000);
	const store = createSnapshotStore<ConsumableDoses>(CONSUMABLES_TOPIC, getConsumableDoses);
	let now = $state(clock());
	let busy = $state<string | null>(null);
	let error = $state<string | null>(null);
	let removed = $state<{ dose: ConsumableDose; at: number } | null>(null);

	const readout = $derived(store.current);
	const rows = $derived(readout ? liveRows(readout.doses, now) : []);
	const options_ = $derived(readout?.options ?? []);
	const undoable = $derived(removed !== null && now - removed.at <= UNDO_SECONDS ? removed : null);

	async function run(key: string, action: () => Promise<unknown>, fallback: string) {
		if (busy) return false;
		busy = key;
		error = null;
		try {
			await action();
			await store.hydrate();
			return true;
		} catch (e) {
			error = describeError(e, fallback);
			return false;
		} finally {
			busy = null;
		}
	}

	return {
		/** The last read, or null before the first. */
		get readout() {
			return readout;
		},
		/** The doses on show at the current tick. */
		get rows() {
			return rows;
		},
		/** The configured consumables a dose can be started from. */
		get options() {
			return options_;
		},
		get now() {
			return now;
		},
		/** The action in flight (a dose id, or `start:<equipment id>`). */
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		/** The dose just removed, while the readout still offers its undo. */
		get undoable() {
			return undoable;
		},
		/** Whether the countdowns need the display tick. */
		get ticking() {
			return rows.length > 0 || undoable !== null;
		},
		/** Attach the frame listener, then read; returns the detach. */
		connect(): () => void {
			let detach: (() => void) | undefined;
			let closed = false;
			void store.subscribe().then((unlisten) => {
				if (closed) unlisten();
				else detach = unlisten;
			});
			void store.hydrate();
			return () => {
				closed = true;
				detach?.();
			};
		},
		/** Re-read the readout: a session's start or stop moves which doses
		 * it holds without a consumables frame of its own. */
		refresh() {
			return store.hydrate();
		},
		/** Advance the display tick. */
		tick() {
			now = clock();
		},
		/** Take a dose; `untimed` declares one taken before the session as
		 * still in force, with no expiry and no cost. */
		async start(equipmentId: number, untimed = false) {
			return run(
				`start:${equipmentId}`,
				() => startConsumableDose(equipmentId, untimed),
				untimed ? 'Could not add the effect.' : 'Could not start the dose.',
			);
		},
		/** End an untimed dose's effect now: it ran out. */
		async end(dose: ConsumableDose) {
			const done = await run(
				dose.id,
				() => endConsumableDose(dose.id),
				'Could not end the effect.',
			);
			if (done) now = clock();
			return done;
		},
		async remove(dose: ConsumableDose) {
			const done = await run(
				dose.id,
				() => removeConsumableDose(dose.id),
				'Could not remove the dose.',
			);
			if (done) {
				now = clock();
				removed = { dose, at: now };
			}
			return done;
		},
		async restore(dose: ConsumableDose) {
			const done = await run(
				dose.id,
				() => restoreConsumableDose(dose.id),
				'Could not restore the dose.',
			);
			if (done && removed?.dose.id === dose.id) removed = null;
			return done;
		},
		dismissError() {
			error = null;
		},
	};
}

export type DosesModel = ReturnType<typeof createDosesModel>;
