/**
 * The Settings row's model: the connection's status, signing in through
 * the window the app opens, and disconnecting.
 *
 * Signing in happens on the account site's own page; the app learns of it
 * only when the connection reports itself connected. Until then the row
 * says to finish signing in there, and pressing its button again brings
 * the same window forward rather than opening a second one.
 *
 * Disconnecting needs no confirmation: signing in again undoes it.
 */

import { disconnectAccount } from '$lib/api/account';
import { signInToAccount } from '$lib/api/shell';
import { describeError } from '$lib/view/errorState';
import { createAccountStatus } from './accountStatus.svelte';
import { type ConnectionView, connectionView } from './connection';

export function createConnectionModel() {
	let signingIn = $state(false);
	let busy = $state<'signIn' | 'disconnect' | null>(null);
	let actionError = $state<string | null>(null);

	const status = createAccountStatus({
		onStatus: (next) => {
			// A connection made, or one being confirmed, ends the wait for the
			// sign-in window.
			if (next.state === 'connected' || next.state === 'unconfirmed') signingIn = false;
		},
	});

	const view = $derived<ConnectionView | null>(
		status.current ? connectionView(status.current, signingIn) : null,
	);

	async function signIn() {
		if (busy) return;
		busy = 'signIn';
		actionError = null;
		try {
			await signInToAccount();
			signingIn = true;
		} catch (cause) {
			actionError = describeError(cause, 'Could not open the sign-in window.');
		} finally {
			busy = null;
		}
	}

	async function disconnect() {
		if (busy) return;
		busy = 'disconnect';
		actionError = null;
		try {
			await disconnectAccount();
			signingIn = false;
			await status.refresh();
		} catch (cause) {
			actionError = describeError(cause, 'Could not disconnect.');
		} finally {
			busy = null;
		}
	}

	return {
		get view() {
			return view;
		},
		/** Why the status could not be read, while none is known. */
		get error() {
			return status.current ? null : status.error;
		},
		get busy() {
			return busy;
		},
		get actionError() {
			return actionError;
		},
		connect: status.connect,
		signIn,
		disconnect,
	};
}

export type ConnectionModel = ReturnType<typeof createConnectionModel>;
