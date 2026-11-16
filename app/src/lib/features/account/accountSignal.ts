/**
 * The account connection at a glance, as the dashboard's sign-in control
 * and the overlay's dot show it. Both read the backend's one signal, so
 * the two never disagree about whether the account is being read.
 *
 * Being signed out by the site is the one state that asks for attention:
 * it pulses until the player signs in again. Not being signed in at launch
 * is the usual state for a player who connected before (the sign-in lasts
 * for one run of the app), so it is an offer, not a warning.
 */

import type { AccountSignal } from '$lib/api/commands.gen';

/** How a signal is drawn: a steady green dot, a pulsing amber one, an
 * empty ring, or a quiet grey dot. */
export type SignalMark = 'live' | 'alert' | 'absent' | 'checking';

export interface SignInView {
	/** The quiet status beside the control, when there is one to say. */
	status: { text: string; mark: SignalMark } | null;
	/** The sign-in button's label, when one is offered. */
	action: string | null;
	/** What the control says on hover. */
	hint: string;
}

const READ_WHEN = 'tracked sessions read your inventory and PED Card once a minute';

/** The dashboard's control for a signal, with the sign-in window open or
 * not. */
export function signInView(signal: AccountSignal, signingIn: boolean): SignInView {
	const view = baseView(signal);
	if (signingIn && view.action) {
		return { ...view, action: 'Finish signing in', hint: 'Bring the sign-in window back.' };
	}
	return view;
}

function baseView(signal: AccountSignal): SignInView {
	switch (signal) {
		case 'signedIn':
			return {
				status: { text: 'Signed in', mark: 'live' },
				action: null,
				hint: `Signed in to your Entropia Universe account: ${READ_WHEN}.`,
			};
		case 'signedOut':
			return {
				status: { text: 'Signed out', mark: 'alert' },
				action: 'Sign in again',
				hint: 'The account site ended your sign-in. Nothing is read until you sign in again.',
			};
		case 'checking':
			return {
				status: { text: 'Checking sign-in', mark: 'checking' },
				action: null,
				hint: 'Checking your sign-in with the account site.',
			};
		case 'notSignedIn':
			return {
				status: null,
				action: 'Sign in',
				hint: `A sign-in lasts until the app closes. Sign in so ${READ_WHEN}.`,
			};
		case 'off':
			return {
				status: null,
				action: 'Connect account',
				hint: `Sign in to your Entropia Universe account so ${READ_WHEN}.`,
			};
	}
}

export interface OverlaySignalView {
	mark: SignalMark;
	label: string;
}

/** The overlay's dot for a signal; none for a player who has not
 * connected. */
export function overlaySignal(signal: AccountSignal): OverlaySignalView | null {
	switch (signal) {
		case 'signedIn':
			return { mark: 'live', label: 'Account signed in: tracked sessions are read' };
		case 'signedOut':
			return {
				mark: 'alert',
				label: 'Signed out by the account site: sign in again from the dashboard',
			};
		case 'notSignedIn':
			return { mark: 'absent', label: 'Account not signed in: sign in from the dashboard' };
		case 'checking':
			return { mark: 'checking', label: 'Checking the account sign-in' };
		case 'off':
			return null;
	}
}
