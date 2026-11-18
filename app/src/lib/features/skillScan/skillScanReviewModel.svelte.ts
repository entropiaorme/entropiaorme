import {
	acceptSkillScan,
	getCharacterSkills,
	getSkillScanReview,
	rejectSkillScan,
	type SkillScanRefusal,
	type SkillScanReview,
	showScanOverlay,
	startSkillScan,
} from '$lib/api';
import type { SkillLevel } from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { needsConfirmation, REFUSAL_TEXT, type ReviewNote, reviewNotes } from './skillScanView';

/**
 * One reviewed skill, as three stops: the level last scanned (the anchor),
 * the level the chat log has carried it to since (believed), and the level
 * this scan read. The correction, scanned less believed, is where the
 * running sum was off.
 */
export interface ReviewRow {
	name: string;
	anchor: number | null;
	believed: number | null;
	scanned: number;
	isAttribute: boolean;
}

/** Scanned less believed; zero when nothing was believed. */
export function correction(row: ReviewRow): number {
	if (row.believed === null) return 0;
	return row.scanned - row.believed;
}

/** The review's rows against what the app already holds, the largest
 * correction first: where the eye should land when checking. */
export function reviewRows(review: SkillScanReview, current: SkillLevel[]): ReviewRow[] {
	const byName = new Map<string, SkillLevel>(current.map((s) => [s.name, s]));
	const rows = Object.entries(review.skills).map(([name, scanned]) => {
		const held = byName.get(name);
		return {
			name,
			anchor: held?.anchorLevel ?? null,
			believed: held?.level ?? null,
			scanned,
			isAttribute: held?.isAttribute ?? false,
		};
	});
	return rows.sort((a, b) => Math.abs(correction(b)) - Math.abs(correction(a)));
}

/**
 * The review of a finished scan: loading it, and settling it. Every request
 * names the review on screen, so an out-of-date one can never settle another
 * scan's result. A review whose figures do not add up offers scanning again
 * first; saving it is a separate, explicit request.
 */
export function createSkillScanReviewModel(onComplete: () => void) {
	let review = $state<SkillScanReview | null>(null);
	let rows = $state<ReviewRow[]>([]);
	let notes = $state<ReviewNote[]>([]);
	let loading = $state(true);
	let busy = $state(false);
	let error = $state<string | null>(null);

	async function load(): Promise<void> {
		loading = true;
		try {
			const [pending, current] = await Promise.all([getSkillScanReview(), getCharacterSkills()]);
			review = pending;
			rows = pending ? reviewRows(pending, current) : [];
			notes = pending ? reviewNotes(pending) : [];
			if (!pending) error = REFUSAL_TEXT.noReview;
		} catch (e) {
			review = null;
			rows = [];
			notes = [];
			error = describeError(e, 'The review could not be loaded');
		} finally {
			loading = false;
		}
	}

	type Settled = { ok?: boolean | null; refused?: SkillScanRefusal | null; error?: string | null };

	/** Run one settling request; answers whether it settled the review. */
	async function settle(request: (scan: number) => Promise<Settled>): Promise<boolean> {
		if (!review || busy) return false;
		busy = true;
		error = null;
		try {
			const result = await request(review.scan);
			if (result.ok) return true;
			const refused = result.refused ?? null;
			error =
				refused === 'saveFailed' && result.error
					? result.error
					: refused
						? REFUSAL_TEXT[refused]
						: (result.error ?? 'The request was refused');
			if (refused === 'staleReview') await load();
			return false;
		} catch (e) {
			error = describeError(e, 'The request failed');
			return false;
		} finally {
			busy = false;
		}
	}

	return {
		get review() {
			return review;
		},
		get rows() {
			return rows;
		},
		get notes() {
			return notes;
		},
		get loading() {
			return loading;
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		/** The figures do not add up or could not be checked: scanning again
		 * comes first, and saving them takes an explicit second request. */
		get needsConfirmation() {
			return review !== null && needsConfirmation(review.total);
		},
		load,
		/** Save a review whose figures add up. */
		async accept() {
			if (await settle((scan) => acceptSkillScan(scan))) onComplete();
		},
		/** Save the figures even though they do not add up. */
		async saveAnyway() {
			if (await settle((scan) => acceptSkillScan(scan, true))) onComplete();
		},
		/** Discard the review. */
		async reject() {
			if (await settle(rejectSkillScan)) onComplete();
		},
		/** Discard the review and collect the pages again from the scan
		 * window, which is opened for it. */
		async scanAgain() {
			try {
				await showScanOverlay();
			} catch (e) {
				error = describeError(e, 'The scan window could not be opened');
				return;
			}
			if (!(await settle(rejectSkillScan))) return;
			try {
				const reply = await startSkillScan();
				if (reply.refused !== null) error = REFUSAL_TEXT[reply.refused];
			} catch (e) {
				error = describeError(e, 'The scan could not start');
			}
			onComplete();
		},
	};
}

export type SkillScanReviewModel = ReturnType<typeof createSkillScanReviewModel>;
