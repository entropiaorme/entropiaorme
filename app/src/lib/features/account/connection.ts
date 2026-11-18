/**
 * The account connection as the Settings row shows it: what the player
 * reads, what they can do, and nothing of the connection's mechanics.
 *
 * A sign-in is kept in the operating system's keyring, so a launch takes
 * it up while the site still accepts it. The app leans on a connected
 * account: not being signed in, being signed out by the site, and readings
 * that keep failing are all lapses, shown as warnings with the last
 * reading's time.
 */

import type { AccountStatus } from '$lib/api/commands.gen';
import { formatDate, formatTime } from '$lib/utils/format';

export type ConnectionTone = 'none' | 'positive' | 'warning' | 'neutral';

export interface ConnectionView {
	tone: ConnectionTone;
	title: string;
	/** Quiet lines under the title. */
	lines: string[];
	/** The sign-in action, when the row offers one. */
	signIn: string | null;
	disconnect: boolean;
	/** Whether the row explains where signing in happens. */
	explainsSignIn: boolean;
	/** Whether the row says what is read and when. */
	describesReading: boolean;
}

/** Where signing in happens, and what the app never sees. */
export const SIGN_IN_NOTE =
	"You sign in on the account site's own page, in a window the app opens; the app never sees your password.";

/** What is read and when, in one sentence. */
export const READING_NOTE =
	'The app reads your inventory and PED Card every five minutes, and once a minute while a session is tracked; it never changes anything. Your sign-in is kept in your system keyring until you disconnect.';

/** The line while the sign-in window is open. */
export const FINISH_SIGNING_IN = 'Finish signing in through the window the app opened.';

/** The second line while a kept sign-in is confirmed and the site could
 * not be reached. */
export const SITE_UNREACHABLE = 'The account site could not be reached; trying again';

function sameDay(a: Date, b: Date): boolean {
	return (
		a.getFullYear() === b.getFullYear() &&
		a.getMonth() === b.getMonth() &&
		a.getDate() === b.getDate()
	);
}

/** When the latest reading was taken, quietly: the time today, the date
 * and time on another day. */
export function lastReading(at: string | null, now: Date = new Date()): string {
	if (!at) return 'No reading yet';
	const when = sameDay(new Date(at), now) ? formatTime(at) : `${formatDate(at)}, ${formatTime(at)}`;
	return `Last reading ${when}`;
}

/** The row for a status. Whether a sign-in window is open is the status's
 * to say, so the row stays true when the window closes, times out, or the
 * site signs the player out meanwhile. */
export function connectionView(status: AccountStatus): ConnectionView {
	const waiting = status.signingIn ? [FINISH_SIGNING_IN] : [];
	const reopen = 'Show sign-in window';
	switch (status.state) {
		case 'connected':
			return {
				tone: status.failingSince ? 'warning' : 'positive',
				title: 'Connected',
				lines: [
					...(status.failingSince
						? [`Readings failing since ${formatTime(status.failingSince)}; still trying`]
						: []),
					...(status.readingSession ? ['Reading this session'] : []),
					lastReading(status.lastReadingAt),
				],
				signIn: null,
				disconnect: true,
				explainsSignIn: false,
				describesReading: true,
			};
		case 'expired':
			return {
				tone: 'warning',
				title: 'Signed out by the site',
				lines: [...waiting, lastReading(status.lastReadingAt)],
				signIn: status.signingIn ? reopen : 'Sign in again',
				disconnect: true,
				explainsSignIn: false,
				describesReading: true,
			};
		case 'unconfirmed':
			return {
				tone: 'neutral',
				title: 'Checking your sign-in',
				lines: status.confirming === 'siteUnreachable' ? [SITE_UNREACHABLE] : [],
				signIn: null,
				disconnect: true,
				explainsSignIn: false,
				describesReading: true,
			};
		case 'disconnected':
			return {
				tone: status.hasConnected ? 'warning' : 'none',
				title: status.hasConnected ? 'Readings paused: not signed in' : 'Account readings',
				lines: status.hasConnected ? [...waiting, lastReading(status.lastReadingAt)] : waiting,
				signIn: status.signingIn ? reopen : status.hasConnected ? 'Sign in' : 'Connect',
				disconnect: false,
				explainsSignIn: true,
				describesReading: status.hasConnected,
			};
	}
}

/** Whether the review of measured sessions is offered: once anything has
 * been read, or while the player is or has been connected. A disconnect
 * never hides what was already read. */
export function offersReview(status: AccountStatus | null): boolean {
	if (!status) return false;
	return status.hasReadings || status.hasConnected || status.state === 'connected';
}

/** What of a status the row shows, as one key: an action's error belongs
 * to the status it was taken in. */
export function rowKey(status: AccountStatus): string {
	return [
		status.state,
		status.signingIn,
		status.confirming,
		status.hasConnected,
		status.failingSince !== null,
	].join('|');
}
