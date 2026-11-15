/**
 * The account connection as the Settings row shows it: what the player
 * reads, what they can do, and nothing of the connection's mechanics.
 *
 * The session lives in memory only, so the usual state at launch, for a
 * player who has connected before, is simply not signed in: nothing went
 * wrong, and the row asks for a sign-in without a warning. Only the site
 * ending the session during a run is a warning.
 */

import type { AccountStatus } from '$lib/api/commands.gen';
import { formatLedgerDate } from '$lib/utils/format';

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
	'While a session is tracked, the app reads your inventory and PED Card once a minute; it never changes anything.';

/** The line while the sign-in window is open. */
export const FINISH_SIGNING_IN = 'Finish signing in through the window the app opened.';

/** When the latest reading was taken, quietly. */
export function lastReading(at: string | null): string {
	return at ? `Last reading ${formatLedgerDate(at)}` : 'No reading yet';
}

/** The row for a status, and whether the sign-in window was opened from
 * it and has not yet connected. */
export function connectionView(status: AccountStatus, signingIn: boolean): ConnectionView {
	const waiting = signingIn ? [FINISH_SIGNING_IN] : [];
	const reopen = 'Show sign-in window';
	switch (status.state) {
		case 'connected':
			return {
				tone: 'positive',
				title: 'Connected',
				lines: [
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
				lines: waiting,
				signIn: signingIn ? reopen : 'Sign in again',
				disconnect: true,
				explainsSignIn: false,
				describesReading: true,
			};
		case 'unconfirmed':
			return {
				tone: 'neutral',
				title: 'Checking your sign-in',
				lines: [],
				signIn: null,
				disconnect: true,
				explainsSignIn: false,
				describesReading: true,
			};
		case 'disconnected':
			return {
				tone: 'none',
				title: 'Account readings',
				lines: waiting,
				signIn: signingIn ? reopen : status.hasConnected ? 'Sign in' : 'Connect',
				disconnect: false,
				explainsSignIn: true,
				describesReading: status.hasConnected,
			};
	}
}
