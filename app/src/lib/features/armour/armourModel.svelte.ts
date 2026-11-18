/**
 * The Armour tab's state: the limited pieces held, their markups, and
 * declaring an item a piece or taking it back off the list.
 *
 * The pieces are re-read on every account frame, so a new reading (a piece
 * bought, replaced or sold) shows without a reload. A markup is saved on
 * its own and answers with the tab as it now stands, once every session it
 * moves has been billed again. Declaring is the cost placement any
 * unplaced item takes, with a limited category, so the Armour tab and the
 * placement list are one record.
 */

import {
	ACCOUNT_TOPIC,
	clearCostPlacement,
	getArmourPieces,
	placeCostItem,
	setArmourPieceMarkup,
} from '$lib/api/account';
import type { ArmourPiecesView } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';
import type { PieceList } from './armour';

export function createArmourModel() {
	let view = $state<ArmourPiecesView | null>(null);
	let readError = $state<string | null>(null);
	let saving = $state<string | null>(null);
	let failure = $state<{ item: string; message: string } | null>(null);
	let declaring = $state<PieceList | null>(null);
	let generation = 0;

	const store = createSnapshotStore<ArmourPiecesView>(ACCOUNT_TOPIC, async () => {
		const begun = generation;
		try {
			const read = await getArmourPieces();
			readError = null;
			if (begun === generation) view = read;
			return read;
		} catch (cause) {
			readError = describeError(cause, 'Could not read your armour.');
			throw cause;
		}
	});

	/** Run one change to a piece; the backend answers with the tab, or the
	 * tab is read again. One change at a time. */
	async function change(
		item: string,
		act: () => Promise<ArmourPiecesView | null>,
		fallback: string,
	): Promise<boolean> {
		if (saving !== null) return false;
		saving = item;
		failure = null;
		try {
			const next = await act();
			generation += 1;
			view = next ?? (await getArmourPieces());
			return true;
		} catch (cause) {
			failure = { item, message: describeError(cause, fallback) };
			return false;
		} finally {
			saving = null;
		}
	}

	return {
		get view(): ArmourPiecesView | null {
			return view;
		},
		get loading(): boolean {
			return view === null && readError === null;
		},
		get error(): string | null {
			return view === null ? readError : null;
		},
		/** The piece being saved now, if any. */
		get saving(): string | null {
			return saving;
		},
		/** Why the last change failed, with the piece it was for. */
		get failure(): { item: string; message: string } | null {
			return failure;
		},
		/** The list an item is being declared into, while the picker is open. */
		get declaring(): PieceList | null {
			return declaring;
		},
		set declaring(list: PieceList | null) {
			declaring = list;
		},
		connect: () => store.connect(),
		/** Set a piece's markup from now on; `null` bills it at TT. */
		setMarkup(item: string, markupPercent: number | null): Promise<boolean> {
			return change(
				item,
				() => setArmourPieceMarkup({ item, markupPercent }),
				'Could not save the markup.',
			);
		},
		/** Declare an item of the latest reading a limited piece. */
		async declare(item: string, list: PieceList): Promise<boolean> {
			const category = list === 'plates' ? 'limitedPlate' : 'limitedArmour';
			const done = await change(
				item,
				async () => {
					await placeCostItem({ item, category });
					return null;
				},
				'Could not add it to the list.',
			);
			if (done) declaring = null;
			return done;
		},
		/** Take a declared piece back off the list. */
		undeclare(item: string): Promise<boolean> {
			return change(
				item,
				async () => {
					await clearCostPlacement(item);
					return null;
				},
				'Could not take it off the list.',
			);
		},
	};
}

export type ArmourModel = ReturnType<typeof createArmourModel>;
