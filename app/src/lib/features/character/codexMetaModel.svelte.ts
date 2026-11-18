/**
 * Codex meta view model: the six attributes a meta reward is claimed
 * into, and the metas earned against the cap. Earned metas follow the
 * recorded species ranks; calibration corrects the count to what the game
 * shows (a meta from a species the app does not carry, or ranks earned
 * before tracking), and the backend keeps that correction as an
 * adjustment, so breakpoints reached afterwards still raise the total.
 */

import {
	calibrateCodexMeta,
	claimCodexMeta,
	getCodexMetaAttributes,
	getCodexMetaProgress,
} from '$lib/api';
import type { CodexMetaAttribute, CodexMetaProgress } from '$lib/api/commands.gen';
import { formatPed } from '$lib/utils/format';
import { describeError } from '$lib/view/errorState';

export interface MetaNotice {
	tone: 'success' | 'error';
	text: string;
}

/** A typed count parsed as a whole number in `0..=cap`, or `null`. */
export function parseEarned(value: string | number | null, cap: number): number | null {
	if (value === null || value === '') return null;
	const earned = typeof value === 'number' ? value : Number(value.trim());
	return Number.isInteger(earned) && earned >= 0 && earned <= cap ? earned : null;
}

export function createCodexMetaModel() {
	let attributes = $state<CodexMetaAttribute[]>([]);
	let progress = $state<CodexMetaProgress | null>(null);
	let loading = $state(true);
	let loadError = $state<string | null>(null);
	let notice = $state<MetaNotice | null>(null);
	let calibrating = $state(false);
	let saving = $state(false);
	let draft = $state<string | number | null>('');

	/** Calibrated metas beyond (or short of) what the ranks account for. */
	const adjustment = $derived(progress ? progress.earned - progress.tracked : 0);

	function adopt(next: CodexMetaProgress) {
		progress = next;
		draft = next.earned;
	}

	async function load() {
		loading = true;
		loadError = null;
		try {
			const [loadedAttributes, loadedProgress] = await Promise.all([
				getCodexMetaAttributes(),
				getCodexMetaProgress(),
			]);
			attributes = loadedAttributes;
			adopt(loadedProgress);
		} catch (e) {
			loadError = describeError(e, 'Failed to load the meta codex');
		} finally {
			loading = false;
		}
	}

	async function claim(attributeName: string) {
		try {
			const result = await claimCodexMeta(attributeName);
			notice = {
				tone: 'success',
				text: `Claimed! ${result.attributeName} +${formatPed(result.pedValue)} PES`,
			};
			attributes = await getCodexMetaAttributes();
		} catch (e) {
			notice = { tone: 'error', text: describeError(e, 'Failed to claim the meta reward') };
		}
	}

	/** Set the metas earned to `earned`; a no-op when it already is. */
	async function calibrate(earned: number) {
		if (!progress || saving || earned === progress.earned) return;
		saving = true;
		try {
			adopt(await calibrateCodexMeta(earned));
			notice = null;
		} catch (e) {
			draft = progress.earned;
			notice = { tone: 'error', text: describeError(e, 'Failed to calibrate the metas earned') };
		} finally {
			saving = false;
		}
	}

	/** Commit the typed count; an unusable entry reverts to the saved one. */
	async function commitDraft() {
		if (!progress) return;
		const earned = parseEarned(draft, progress.cap);
		if (earned === null) {
			draft = progress.earned;
			notice = { tone: 'error', text: `Enter a whole number from 0 to ${progress.cap}.` };
			return;
		}
		await calibrate(earned);
	}

	function step(delta: number) {
		if (!progress) return;
		void calibrate(Math.max(0, Math.min(progress.cap, progress.earned + delta)));
	}

	function toggleCalibrating() {
		calibrating = !calibrating;
		if (progress) draft = progress.earned;
	}

	return {
		get attributes() {
			return attributes;
		},
		get progress() {
			return progress;
		},
		get loading() {
			return loading;
		},
		get loadError() {
			return loadError;
		},
		get notice() {
			return notice;
		},
		get calibrating() {
			return calibrating;
		},
		get saving() {
			return saving;
		},
		get adjustment() {
			return adjustment;
		},
		/** The count being typed while calibrating. */
		get draft() {
			return draft;
		},
		set draft(value: string | number | null) {
			draft = value;
		},

		load,
		claim,
		calibrate,
		commitDraft,
		step,
		toggleCalibrating,
	};
}

export type CodexMetaModel = ReturnType<typeof createCodexMetaModel>;
