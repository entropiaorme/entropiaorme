/**
 * One tracked session's measure, re-read on every account frame (a reading
 * stored, a measure sealed) so a running session's measure follows the
 * readings.
 *
 * Each read carries the session it was asked for, and an answer that
 * arrives for a session other than the one now shown is never rendered:
 * a measure fetched for one session cannot land under another after the
 * player moves on.
 */

import { ACCOUNT_TOPIC, getSessionMeasure } from '$lib/api/account';
import type { AccountSessionMeasure } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';

interface MeasureRead {
	sessionId: string;
	measure: AccountSessionMeasure | null;
}

export function createSessionMeasureModel(sessionId: () => string) {
	let failure = $state<{ sessionId: string; message: string } | null>(null);
	const store = createSnapshotStore<MeasureRead>(ACCOUNT_TOPIC, async () => {
		const asked = sessionId();
		try {
			const measure = await getSessionMeasure(asked);
			if (failure?.sessionId === asked) failure = null;
			return { sessionId: asked, measure };
		} catch (cause) {
			failure = { sessionId: asked, message: describeError(cause, 'Could not read the measure.') };
			throw cause;
		}
	});

	const read = $derived(store.current?.sessionId === sessionId() ? store.current : null);

	return {
		/** Whether the shown session's measure has been read at least once. */
		get answered(): boolean {
			return read !== null;
		},
		/** The shown session's measure; `null` while unread or absent. */
		get measure(): AccountSessionMeasure | null {
			return read?.measure ?? null;
		},
		/** Why the latest read of the shown session failed, while it did. */
		get error(): string | null {
			return failure?.sessionId === sessionId() ? failure.message : null;
		},
		dismissError() {
			failure = null;
		},
		refresh(): Promise<void> {
			return store.hydrate();
		},
		/** Attach the account topic listener; returns the detach. Reads are
		 * the caller's, keyed on the session shown. */
		connect(): () => void {
			let detach: (() => void) | undefined;
			let closed = false;
			void store.subscribe().then((unlisten) => {
				if (closed) unlisten();
				else detach = unlisten;
			});
			return () => {
				closed = true;
				detach?.();
			};
		},
	};
}

export type SessionMeasureModel = ReturnType<typeof createSessionMeasureModel>;
