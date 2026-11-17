/**
 * The pure rules of naming a limited set's pieces: which names are offered,
 * which start ticked, and whether a confirmation would change anything.
 * The account readings bill a confirmed set by what its pieces lose, so a
 * set is named once, from the suggestions the latest account reading makes.
 */

import type { ProtectionSetPieces } from '$lib/api';

/** What the pieces section shows. */
export type PiecesView = 'empty' | 'confirmed' | 'choosing';

/** One name the player can tick. */
export interface PieceChoice {
	readonly name: string;
	/** Named by the set's name, or already one of its pieces. */
	readonly suggested: boolean;
}

/** The view before any change: the confirmed pieces when there are some,
 * otherwise a choice when the reading offers anything, otherwise empty. */
export function piecesView(pieces: ProtectionSetPieces): PiecesView {
	if (pieces.confirmed.length > 0) return 'confirmed';
	if (pieces.suggested.length + pieces.others.length > 0) return 'choosing';
	return 'empty';
}

/** Every name on offer, each once: the confirmed pieces and the suggestions
 * first, then the reading's other limited armour and plates. */
export function pieceChoices(pieces: ProtectionSetPieces): PieceChoice[] {
	const seen = new Set<string>();
	const choices: PieceChoice[] = [];
	const offer = (name: string, suggested: boolean) => {
		if (seen.has(name)) return;
		seen.add(name);
		choices.push({ name, suggested });
	};
	for (const piece of pieces.confirmed) offer(piece.name, true);
	for (const name of pieces.suggested) offer(name, true);
	for (const name of pieces.others) offer(name, false);
	return choices;
}

/** The names ticked when the choice opens: the confirmed pieces, or else
 * the suggestions. */
export function initialSelection(pieces: ProtectionSetPieces): Set<string> {
	return new Set(
		pieces.confirmed.length > 0 ? pieces.confirmed.map((piece) => piece.name) : pieces.suggested,
	);
}

/** Whether confirming `selection` would change the set's pieces. */
export function selectionChanged(
	pieces: ProtectionSetPieces,
	selection: ReadonlySet<string>,
): boolean {
	const confirmed = new Set(pieces.confirmed.map((piece) => piece.name));
	if (confirmed.size !== selection.size) return true;
	for (const name of selection) if (!confirmed.has(name)) return true;
	return false;
}

/** A set's pieces in a few words, for its row: "6 pieces", or that none
 * are confirmed yet. */
export function piecesSummary(count: number): string {
	if (count === 0) return 'Pieces not confirmed';
	return `${count} ${count === 1 ? 'piece' : 'pieces'}`;
}
