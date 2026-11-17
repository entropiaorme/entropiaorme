/**
 * The account connection at a glance, as the dashboard's sign-in control
 * and the overlay's dot show it. Both read the backend's one signal, so
 * the two never disagree about whether the account is being read.
 *
 * The app leans on a connected account, so a lapse in the readings is
 * what asks for attention: signed out by the site, not signed in at all
 * (the readings wait on a sign-in), or signed in while the readings keep
 * failing. Each pulses, and says since when, until the readings resume.
 */

import type { AccountSignal } from '$lib/api/commands.gen';
import { formatDate, formatTime } from '$lib/utils/format';

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

const READ_WHEN = 'the app reads your inventory and PED Card';

/** When the readings stood last, and since when they have been failing. */
export interface SignalTimes {
	lastReadingAt?: string | null;
	failingSince?: string | null;
}

/** A moment, quietly: the time today, the date and time on another day. */
export function moment(at: string, now: Date = new Date()): string {
	const when = new Date(at);
	const today =
		when.getFullYear() === now.getFullYear() &&
		when.getMonth() === now.getMonth() &&
		when.getDate() === now.getDate();
	return today ? formatTime(at) : `${formatDate(at)}, ${formatTime(at)}`;
}

/** The dashboard's control for a signal, with the sign-in window open or
 * not. */
export function signInView(
	signal: AccountSignal,
	signingIn: boolean,
	times: SignalTimes = {},
	now: Date = new Date(),
): SignInView {
	const view = baseView(signal, times, now);
	if (signingIn && view.action) {
		return { ...view, action: 'Finish signing in', hint: 'Bring the sign-in window back.' };
	}
	return view;
}

function lastRead(times: SignalTimes, now: Date): string {
	return times.lastReadingAt ? ` The last reading was at ${moment(times.lastReadingAt, now)}.` : '';
}

function baseView(signal: AccountSignal, times: SignalTimes, now: Date): SignInView {
	switch (signal) {
		case 'signedIn':
			return {
				status: { text: 'Signed in', mark: 'live' },
				action: null,
				hint: `Signed in to your Entropia Universe account: ${READ_WHEN}.`,
			};
		case 'failing':
			return {
				status: {
					text: times.failingSince
						? `Readings failing since ${moment(times.failingSince, now)}`
						: 'Readings failing',
					mark: 'alert',
				},
				action: null,
				hint: `Signed in, but the account site is not answering the readings; they keep being tried.${lastRead(times, now)}`,
			};
		case 'signedOut':
			return {
				status: { text: 'Signed out', mark: 'alert' },
				action: 'Sign in again',
				hint: `The account site ended your sign-in. Nothing is read until you sign in again.${lastRead(times, now)}`,
			};
		case 'checking':
			return {
				status: { text: 'Checking sign-in', mark: 'checking' },
				action: null,
				hint: 'Checking your sign-in with the account site.',
			};
		case 'notSignedIn':
			return {
				status: { text: 'Readings paused', mark: 'alert' },
				action: 'Sign in',
				hint: `Not signed in, so nothing is read until you sign in.${lastRead(times, now)}`,
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
		case 'failing':
			return {
				mark: 'alert',
				label: 'Account readings failing: the account site is not answering',
			};
		case 'notSignedIn':
			return {
				mark: 'alert',
				label: 'Account readings paused: sign in from the dashboard',
			};
		case 'checking':
			return { mark: 'checking', label: 'Checking the account sign-in' };
		case 'off':
			return null;
	}
}
