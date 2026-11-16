/**
 * Openings the account readings found, as the strongbox view offers them:
 * which belong to a run just read, and one quiet line for each that waits
 * to be reviewed. Pure.
 */
import type { AccountProposal } from '$lib/api/commands.gen';
import { formatQuantity, formatSeenAt } from '$lib/utils/format';
import { isOpeningProposal, type OpeningProposal } from './openingDraft';

/** The openings seen in a run: every step after the reading `from` (the
 * one before the boxes) up to the reading `to` (the one after), oldest
 * first. */
export function runProposals(
	proposals: AccountProposal[],
	from: number,
	to: number,
): OpeningProposal[] {
	return proposals
		.filter(isOpeningProposal)
		.filter((proposal) => proposal.previousReadingId >= from && proposal.readingId <= to)
		.sort((a, b) => a.readingId - b.readingId);
}

/** What a detected opening was: "7 Halloween Strongbox". */
export function detectedWhat(proposal: OpeningProposal): string {
	return proposal.outcome.boxes
		.map((boxes) => `${formatQuantity(boxes.count)} ${boxes.name}`)
		.join(' and ');
}

/** When and where it was seen: "today, 3:04 PM, during a tracked session".
 * The time is when the reading that saw it was taken. */
export function detectedWhen(proposal: OpeningProposal, now: Date = new Date()): string {
	const when = formatSeenAt(proposal.between.to, now);
	return proposal.sessionId ? `${when}, during a tracked session` : when;
}
