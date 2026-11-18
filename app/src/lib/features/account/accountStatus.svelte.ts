/**
 * The account connection's status, kept current by the account topic: a
 * connect, a disconnect, the site ending the session, or a reading stored
 * each re-reads it, so a surface showing the connection changes the moment
 * the connection does, with no polling.
 *
 * Read from the main window only: the account commands are refused to
 * every other window.
 */

import { ACCOUNT_TOPIC, getAccountStatus } from '$lib/api/account';
import type { AccountStatus } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';

export interface AccountStatusOptions {
	/** Called with each status read, before it is published. */
	onStatus?: (status: AccountStatus) => void;
}

export function createAccountStatus(options: AccountStatusOptions = {}) {
	let error = $state<string | null>(null);
	const store = createSnapshotStore<AccountStatus>(ACCOUNT_TOPIC, async () => {
		try {
			const status = await getAccountStatus();
			error = null;
			options.onStatus?.(status);
			return status;
		} catch (cause) {
			error = describeError(cause, 'Could not read the account connection');
			throw cause;
		}
	});

	return {
		/** The latest status; `null` before the first read answers. */
		get current(): AccountStatus | null {
			return store.current;
		},
		/** Why the latest read failed, while it did. */
		get error() {
			return error;
		},
		refresh(): Promise<void> {
			return store.hydrate();
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

export type AccountStatusModel = ReturnType<typeof createAccountStatus>;
