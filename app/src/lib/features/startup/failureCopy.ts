/**
 * What the startup failure surface says for each reason the backend can
 * decline to start: one plain sentence for what went wrong and one for what
 * the user can do about it. The shell's logged detail stays out of the prose
 * and is offered only through "Copy details".
 */

import type { SubstrateFailure } from '$lib/api/readiness.svelte';
import type { SubstrateDeclineReason } from '$lib/api/shell';

export interface FailureCopy {
	readonly summary: string;
	readonly remedy: string;
}

const REPORT = 'If it keeps happening, copy the details and include them when you report it.';

const COPY: Record<SubstrateDeclineReason, FailureCopy> = {
	data_dir_unavailable: {
		summary: 'The folder EntropiaOrme keeps its data in could not be created.',
		remedy: 'Check that your account can write to its app data folder, then restart.',
	},
	database_below_baseline: {
		summary: 'Your database comes from a version too old for this release to upgrade.',
		remedy: 'Copy the details and include them when you report it.',
	},
	database_unreadable: {
		summary: 'Your database could not be opened.',
		remedy: `Restart to try again. ${REPORT}`,
	},
	game_data_unavailable: {
		summary: 'The game data bundled with EntropiaOrme is missing.',
		remedy: 'Reinstalling EntropiaOrme restores it.',
	},
	tracking_unavailable: {
		summary: 'The live-tracking services could not start.',
		remedy: `Restart to try again. ${REPORT}`,
	},
	unexpected: {
		summary: 'Something went wrong while EntropiaOrme was starting.',
		remedy: `Restart to try again. ${REPORT}`,
	},
};

export function failureCopy(reason: SubstrateDeclineReason): FailureCopy {
	return COPY[reason];
}

/** The plain-text block "Copy details" puts on the clipboard. */
export function failureReport(failure: SubstrateFailure, version: string | null): string {
	return [
		`EntropiaOrme${version ? ` ${version}` : ''} could not start.`,
		`Reason: ${failure.reason}`,
		`Detail: ${failure.detail}`,
	].join('\n');
}
