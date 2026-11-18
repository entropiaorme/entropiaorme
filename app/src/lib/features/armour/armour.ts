/**
 * What the Armour tab reads off the pieces: the two lists, the pieces no
 * longer held, how a markup reads and is typed, and how a declarable item
 * is described. Every rule that decides a figure lives in the backend.
 */

import type {
	ArmourCandidateHint,
	ArmourCandidateRow,
	ArmourPieceList,
	ArmourPieceRow,
	ArmourPiecesView,
} from '$lib/api/commands.gen';
import { formatDate } from '$lib/utils/format';
import { filterByQuery } from '$lib/view/search';

export type Piece = ArmourPieceRow;
export type PieceList = ArmourPieceList;
export type Candidate = ArmourCandidateRow;

/** The lowest markup a limited piece takes, in percent. */
export const LOWEST_MARKUP = 100;

/** The pieces the latest reading holds, in one list. */
export function heldIn(view: ArmourPiecesView | null, list: PieceList): Piece[] {
	return (view?.pieces ?? []).filter((piece) => piece.held && piece.list === list);
}

/** The pieces no longer held that still carry a markup or a declaration:
 * their sessions keep the markup they were billed at. */
export function noLongerHeld(view: ArmourPiecesView | null): Piece[] {
	return (view?.pieces ?? []).filter((piece) => !piece.held);
}

/** A markup as the row shows it: "120.65%". */
export function markupText(percent: number): string {
	return `${percent.toFixed(2).replace(/\.?0+$/, '')}%`;
}

/** What a row says beside its markup field. */
export function pricingNote(piece: Piece): string {
	if (piece.markupPercent === null) return 'No markup set: billed at TT';
	if (piece.markupSetAt === null) return 'Billed at this markup';
	return `Set ${formatDate(piece.markupSetAt)}; applies from then on`;
}

/** A markup as typed: a number from 100, or empty for none. `undefined`
 * when the text is not one. */
export function parseMarkup(text: string): number | null | undefined {
	const trimmed = text.trim().replace(/%$/, '').trim();
	if (trimmed === '') return null;
	const value = Number(trimmed.replace(',', '.'));
	if (!Number.isFinite(value) || value < LOWEST_MARKUP) return undefined;
	return Math.round(value * 100) / 100;
}

/** Why a typed markup is refused. */
export const MARKUP_REFUSED = 'A markup is a number from 100%; leave it empty to bill at TT.';

/** Why an item is offered first when declaring a piece, in a few words. */
export const HINT_LABEL: Record<ArmourCandidateHint, string | null> = {
	awaitingPlacement: 'Wore in a session, not yet placed',
	attached: 'Attached to another item',
	tagged: 'Tagged like armour',
	other: null,
};

/** The items to offer for one list, the likeliest first, narrowed by a
 * filter. Plates put attached items before tagged ones; armour the other
 * way round. */
export function candidatesFor(
	view: ArmourPiecesView | null,
	list: PieceList,
	query: string,
): Candidate[] {
	const rank: Record<ArmourCandidateHint, number> =
		list === 'plates'
			? { awaitingPlacement: 0, attached: 1, tagged: 2, other: 3 }
			: { awaitingPlacement: 0, tagged: 1, attached: 2, other: 3 };
	return filterByQuery(view?.candidates ?? [], query, (candidate) => candidate.item)
		.slice()
		.sort((a, b) => rank[a.hint] - rank[b.hint] || a.item.localeCompare(b.item));
}
