/**
 * Startup readiness: the one gate between this window and the backend facade
 * while the backend is still starting.
 *
 * The shell composes the backend after the webview is already running, so a
 * surface can mount and ask for data before the facade exists. Rather than
 * let those early calls fail (and have every feature suppress or retry the
 * failure), the typed transport awaits {@link whenSubstrateReady} before its
 * first dispatch: calls made while the backend starts are held, then sent
 * once it is ready, and each feature's ordinary loading state simply lasts
 * through startup. The answer comes from a shell command that reports the
 * settled outcome whenever it is asked, so a window that asks after startup
 * finished gets it at once; there is no one-shot event to miss.
 *
 * {@link substrate} exposes the same state to the root layout, which shows
 * the startup indicator while it is `starting` and the failure surface if it
 * is `failed`. Each webview is its own JS context, so every window runs its
 * own gate against the same shell record.
 */

import { ApiError } from './client';
import { awaitSubstrate, type SubstrateDeclineReason } from './shell';

export type SubstratePhase = 'starting' | 'ready' | 'failed';

export interface SubstrateFailure {
	readonly reason: SubstrateDeclineReason;
	/** The shell's logged detail, offered for a bug report. */
	readonly detail: string;
}

/** The message a held call rejects with when startup failed. */
export const SUBSTRATE_FAILED_MESSAGE = 'the backend did not start';

let phase = $state<SubstratePhase>('starting');
let failure = $state<SubstrateFailure | null>(null);
let gate: Promise<void> | undefined;

/** This window's view of backend startup. */
export const substrate = {
	get phase(): SubstratePhase {
		return phase;
	},
	get failure(): SubstrateFailure | null {
		return failure;
	},
};

/**
 * Resolve once the backend is ready; reject with an `unavailable` ApiError if
 * startup failed. The shell is asked once per window and every caller shares
 * that answer, so a burst of early calls costs one readiness query.
 */
export function whenSubstrateReady(): Promise<void> {
	gate ??= settle();
	return gate;
}

async function settle(): Promise<void> {
	let outcome: Awaited<ReturnType<typeof awaitSubstrate>>;
	try {
		outcome = await awaitSubstrate();
	} catch (err) {
		// The readiness question itself could not be asked (no shell behind
		// this page, or a window whose capability lacks the command). Holding
		// every call forever would be strictly worse than not gating, so the
		// gate opens and the facade's own `unavailable` answer covers any call
		// that arrives too early.
		console.warn('startup readiness unavailable; commands are not held', err);
		phase = 'ready';
		return;
	}
	if (outcome.state === 'ready') {
		phase = 'ready';
		return;
	}
	failure = { reason: outcome.reason, detail: outcome.detail };
	phase = 'failed';
	throw new ApiError('unavailable', SUBSTRATE_FAILED_MESSAGE);
}
