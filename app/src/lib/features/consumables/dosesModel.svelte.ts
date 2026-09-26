/**
 * Runes-native state for a live dose readout (the overlay's doses and the
 * dashboard's panel): the persisted doses read on every `consumables:updated`
 * frame, a local display tick for the countdowns, and the start, remove, and
 * restore actions.
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
	/** Show a healing tool's automatic buffs (the dashboard does; the
	 * overlay, which is for actions, does not). */
	includeOnUse: boolean;
	/** The display clock, epoch seconds; injectable for tests. */
	clock?: () => number;
}

export function createDosesModel(options: DosesModelOptions) {
	const clock = options.clock ?? (() => Date.now() / 1000);
	const store = createSnapshotStore<ConsumableDoses>(CONSUMABLES_TOPIC, getConsumableDoses);
	let now = $state(clock());
	let busy = $state<string | null>(null);
	let error = $state<string | null>(null);
	let removed = $state<{ dose: ConsumableDose; at: number } | null>(null);

	const readout = $derived(store.current);
	const rows = $derived(readout ? liveRows(readout.doses, now, options.includeOnUse) : []);
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
		/** Advance the display tick. */
		tick() {
			now = clock();
		},
		async start(equipmentId: number) {
			return run(
				`start:${equipmentId}`,
				() => startConsumableDose(equipmentId),
				'Could not start the dose.',
			);
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
