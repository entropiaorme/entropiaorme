import {
	cancelSkillScan,
	type SkillScanReply,
	type SkillScanStatus,
	startSkillScan,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { REFUSAL_TEXT, type ScanView, scanView } from './skillScanView';

/**
 * What a scan surface lends the model: the status it renders (the window's
 * scan store), and the re-read that brings that status up to date. An action
 * never folds its own answer into the status; it re-reads, so every surface
 * renders the one snapshot the backend holds.
 */
export interface SkillScanPorts {
	status: () => SkillScanStatus | null;
	refresh: () => Promise<void>;
}

/**
 * A scan surface's view of the skill scan: what to show for the current
 * status, and starting or cancelling it.
 */
export function createSkillScanModel(ports: SkillScanPorts) {
	let busy = $state(false);
	let error = $state<string | null>(null);

	async function act(action: () => Promise<SkillScanReply>, failure: string): Promise<void> {
		busy = true;
		error = null;
		try {
			const reply = await action();
			if (reply.refused !== null) error = REFUSAL_TEXT[reply.refused];
		} catch (e) {
			error = describeError(e, failure);
		} finally {
			busy = false;
		}
		await ports.refresh();
	}

	return {
		/** Worked out afresh from the status on every read, so it can never
		 * lag the status it shows. */
		get view(): ScanView {
			return scanView(ports.status());
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		start: () => act(startSkillScan, 'The scan could not start'),
		cancel: () => act(cancelSkillScan, 'The scan could not be cancelled'),
	};
}

export type SkillScanModel = ReturnType<typeof createSkillScanModel>;
