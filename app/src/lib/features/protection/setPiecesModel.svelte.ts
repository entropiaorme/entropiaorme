/**
 * Runes-native state for one limited set's pieces in its editor: the
 * confirmed pieces and the latest account reading's suggestions, the names
 * ticked, and the one confirmation that names the set's pieces.
 */

import {
	confirmProtectionSetPieces,
	getProtectionSetPieces,
	type ProtectionSetPieces,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';
import {
	initialSelection,
	type PiecesView,
	pieceChoices,
	piecesView,
	selectionChanged,
} from './setPieces';

export function createSetPiecesModel(setId: () => string, onConfirmed: () => void = () => {}) {
	let pieces = $state<ProtectionSetPieces | null>(null);
	let loading = $state(true);
	let saving = $state(false);
	let error = $state<string | null>(null);
	let editing = $state(false);
	let selection = $state<Set<string>>(new Set());

	const view = $derived<PiecesView | null>(
		pieces === null ? null : editing ? 'choosing' : piecesView(pieces),
	);
	const choices = $derived(pieces === null ? [] : pieceChoices(pieces));
	const canConfirm = $derived(
		pieces !== null &&
			!saving &&
			selectionChanged(pieces, selection) &&
			(selection.size > 0 || pieces.confirmed.length > 0),
	);

	async function load(): Promise<void> {
		loading = true;
		error = null;
		try {
			pieces = await getProtectionSetPieces(setId());
			selection = initialSelection(pieces);
		} catch (cause) {
			error = describeError(cause, "The set's pieces could not be read");
		} finally {
			loading = false;
		}
	}

	async function confirm(): Promise<void> {
		if (!canConfirm) return;
		saving = true;
		error = null;
		try {
			pieces = await confirmProtectionSetPieces(setId(), [...selection]);
			selection = initialSelection(pieces);
			editing = false;
			onConfirmed();
		} catch (cause) {
			error = describeError(cause, "The set's pieces could not be confirmed");
		} finally {
			saving = false;
		}
	}

	return {
		get pieces() {
			return pieces;
		},
		get loading() {
			return loading;
		},
		get saving() {
			return saving;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get editing() {
			return editing;
		},
		get view() {
			return view;
		},
		get choices() {
			return choices;
		},
		get selection(): ReadonlySet<string> {
			return selection;
		},
		get canConfirm() {
			return canConfirm;
		},
		load,
		confirm,
		toggle(name: string) {
			const next = new Set(selection);
			if (next.has(name)) next.delete(name);
			else next.add(name);
			selection = next;
		},
		/** Reopen the choice over the confirmed pieces. */
		edit() {
			if (pieces === null) return;
			selection = initialSelection(pieces);
			editing = true;
		},
		cancel() {
			if (pieces !== null) selection = initialSelection(pieces);
			editing = false;
		},
	};
}

export type SetPiecesModel = ReturnType<typeof createSetPiecesModel>;
