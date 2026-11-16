/**
 * The Settings row's model: the connection's status, signing in through
 * the window the app opens, and disconnecting.
 *
 * Signing in happens on the account site's own page. Whether its window is
 * open is the status's to say, so the row follows the window closing, its
 * time running out, or the site signing the player out, with no press.
 * Pressing the button again while the window is open brings the same
 * window forward rather than opening a second one.
 *
 * Disconnecting needs no confirmation: signing in again undoes it. An
 * action's error stands until the status the row shows changes.
 */

import { disconnectAccount } from '$lib/api/account';
import { signInToAccount } from '$lib/api/shell';
import { describeError } from '$lib/view/errorState';
import { createAccountStatus } from './accountStatus.svelte';
import { type ConnectionView, connectionView, rowKey } from './connection';

export function createConnectionModel() {
	let busy = $state<'signIn' | 'disconnect' | null>(null);
	let actionError = $state<string | null>(null);
	let shown: string | null = null;

	const status = createAccountStatus({
		onStatus: (next) => {
			const key = rowKey(next);
			if (shown !== null && key !== shown) actionError = null;
			shown = key;
		},
	});

	const view = $derived<ConnectionView | null>(
		status.current ? connectionView(status.current) : null,
	);

	async function signIn() {
		if (busy) return;
		busy = 'signIn';
		actionError = null;
		try {
			await signInToAccount();
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
		dismissActionError() {
			actionError = null;
		},
		connect: status.connect,
		signIn,
		disconnect,
	};
}

export type ConnectionModel = ReturnType<typeof createConnectionModel>;
