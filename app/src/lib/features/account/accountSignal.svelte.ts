/**
 * The account signal for a window other than the main one: the overlay
 * may not read the connection's status, only this one fact. Kept current
 * by the account topic, with no polling.
 */

import { ACCOUNT_TOPIC, getAccountSignal } from '$lib/api/account';
import type { AccountSignal } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';

export function createAccountSignal() {
	const store = createSnapshotStore<AccountSignal>(ACCOUNT_TOPIC, getAccountSignal);
	return {
		/** The latest signal; `null` before the first read answers or when
		 * it cannot be read. */
		get current(): AccountSignal | null {
			return store.current;
		},
		/** Attach the topic listener, then read; returns the detach. */
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
	};
}

export type AccountSignalModel = ReturnType<typeof createAccountSignal>;
