/**
 * What the skill-scan surfaces say, worked out from the scan's status and its
 * review. Only what helps the player act is said: how far the scan has come,
 * which pages are still to show when few remain, the one thing to change
 * when the game's view cannot be used, and why a scan stopped or a request
 * was refused.
 */

import type {
	SkillScanHint,
	SkillScanRefusal,
	SkillScanReview,
	SkillScanStatus,
	SkillScanStop,
	SkillScanTotal,
} from '$lib/api';

/** The pages still to show are listed once this few remain. */
export const FEW_PAGES = 3;

/** The one thing to change in the game for its view to be collected. */
export const HINT_TEXT: Record<SkillScanHint, string> = {
	gameWindowNotFound: 'Start Entropia Universe, or bring its window back.',
	gameWindowOffScreen: 'Move the game window onto the screen shared for capture.',
	captureFailed: 'The game window could not be captured.',
	skillsWindowNotOpen: 'Open the Skills window in the game.',
	skillsWindowCovered: 'Move whatever covers the Skills window.',
	skillsWindowOffScreen: 'Move the Skills window fully into view.',
	notSkillsTab: 'Switch the Skills window to its SKILLS tab.',
	windowTooShort: 'Drag the Skills window taller, until a whole page fits.',
	pageNotRead: 'This page did not read. Show it again, or carry on and come back to it.',
	otherListing:
		'This page is from another category or page count than those collected. Switch back, or cancel and scan again.',
};

/** Why the last scan stopped by itself. */
export const STOP_TEXT: Record<SkillScanStop, string> = {
	readFailed: 'The scan stopped: the Skills window could not be read. Start it again to retry.',
	captureNotAllowed: 'Screen capture was not allowed. Start the scan again to be asked once more.',
	noNewPage: 'The scan stopped after a while with no new page. Start it again.',
};

/** Why a request about the scan was refused. */
export const REFUSAL_TEXT: Record<SkillScanRefusal, string> = {
	noReview: 'There is no scan to review.',
	staleReview: 'That review was out of date; here is the current one.',
	totalNotConfirmed: "These figures do not add up to the game's total.",
	saving: 'The results are being saved.',
	surfaceClosed: 'Open the scan window to start a scan.',
	workerUnavailable: 'The scan could not start.',
	saveFailed: 'The results could not be saved.',
};

/** What a scan surface shows, by phase. */
export type ScanView =
	| { kind: 'loading' }
	| { kind: 'idle'; canStart: boolean; note: string | null }
	| {
			kind: 'collecting';
			/** `7 of 13 pages`, or that none has been seen yet. */
			progress: string;
			/** The share of pages collected, 0 to 1. */
			fraction: number;
			/** The pages still to show, once few remain. */
			missing: string | null;
			hint: string | null;
	  }
	| { kind: 'review' }
	| { kind: 'saving' };

/** `page 4`, `pages 4 and 9`, `pages 4, 9 and 12`. */
export function pageList(pages: readonly number[]): string {
	if (pages.length === 1) return `page ${pages[0]}`;
	const head = pages.slice(0, -1).join(', ');
	return `pages ${head} and ${pages[pages.length - 1]}`;
}

export function scanView(status: SkillScanStatus | null): ScanView {
	if (status === null) return { kind: 'loading' };
	switch (status.phase) {
		case 'idle':
			return {
				kind: 'idle',
				canStart: status.available,
				note: !status.available
					? 'Reading skills is unavailable.'
					: status.stopped === null
						? null
						: STOP_TEXT[status.stopped],
			};
		case 'collecting': {
			const { pageCount, pagesCollected, missingPages, hint } = status;
			const few = pageCount !== null && missingPages.length > 0 && missingPages.length <= FEW_PAGES;
			return {
				kind: 'collecting',
				progress: pageCount === null ? 'No pages yet' : `${pagesCollected} of ${pageCount} pages`,
				fraction: pageCount ? pagesCollected / pageCount : 0,
				missing: few ? `Still to show: ${pageList(missingPages)}` : null,
				hint: hint === null ? null : HINT_TEXT[hint],
			};
		}
		case 'awaitingReview':
			return { kind: 'review' };
		case 'saving':
			return { kind: 'saving' };
	}
}

function points(value: number): string {
	return value.toLocaleString('en-GB');
}

/** Whether a review's figures must be confirmed before they are saved:
 * they do not add up to the game's total, or could not be checked. */
export function needsConfirmation(total: SkillScanTotal): boolean {
	return total.check !== 'matches';
}

/** What the review says of the total, only when there is something to say. */
export function totalNote(total: SkillScanTotal): string | null {
	switch (total.check) {
		case 'matches':
			return null;
		case 'mismatch':
			return (
				`These figures add up to ${points(total.sum)}, but the game's total is ` +
				`${points(total.total)}: something was misread or missed.`
			);
		case 'unchecked':
			return "These figures could not be checked against the game's total.";
	}
}

/** One thing the review tells the player, with the skills it concerns. */
export interface ReviewNote {
	text: string;
	items: string[];
}

/** Everything the review tells the player beyond the changes themselves. */
export function reviewNotes(review: SkillScanReview): ReviewNote[] {
	const notes: ReviewNote[] = [];
	const total = totalNote(review.total);
	if (total) notes.push({ text: total, items: [] });
	if (review.nearMisses.length > 0) {
		notes.push({
			text: 'Matched from a slightly misread name; check these:',
			items: review.nearMisses.map((m) => `${m.skill} (read as "${m.readAs}")`),
		});
	}
	if (review.guesses.length > 0) {
		notes.push({
			text: 'Not saved, as the name did not read clearly; scan again if the skill matters:',
			items: review.guesses.map(
				(g) => `read as "${g.readAs}", probably ${g.probably} ${g.pointsText}`,
			),
		});
	}
	if (review.unknown.length > 0) {
		notes.push({
			text: 'Shown in the game but unknown to this version of the app, so not saved:',
			items: review.unknown.map((s) => `${s.name} ${s.pointsText}`),
		});
	}
	if (review.unplaced.length > 0) {
		notes.push({
			text: 'Not saved, because the name could be either of two skills:',
			items: review.unplaced.map((s) => `${s.name} ${s.pointsText}`),
		});
	}
	if (review.repeated.length > 0) {
		notes.push({
			text: 'Not saved, because each was read more than once:',
			items: review.repeated,
		});
	}
	return notes;
}
