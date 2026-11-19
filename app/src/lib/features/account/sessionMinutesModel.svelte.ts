/**
 * One measured session minute by minute, re-read on every account frame (a
 * reading stored, a proposal found or decided) and after every decision
 * made here, so what is shown is always the backend's own.
 *
 * An arrival nothing explains is decided here in one of three ways:
 * recorded as a purchase through the asset forms the player uses by hand
 * (claimed before the save and linked to the purchase after it, exactly as
 * a purchase seen outside a session is), held as stock of unknown origin
 * (which can be taken back), or ignored (and offered again). Loot that
 * never arrived is taken out of the session's loot through the session's
 * own loot correction.
 *
 * Each read carries the session it was asked for, and an answer that
 * arrives for another session than the one now shown is never rendered.
 */

import {
	ACCOUNT_TOPIC,
	dismissProposal,
	getSessionMinutes,
	recordArrivalUnknownOrigin,
	undoArrivalUnknownOrigin,
	unignoreArrival,
} from '$lib/api/account';
import type { AccountSessionMinutes, InventoryItem, MinuteArrival } from '$lib/api/commands.gen';
import { getEquipmentInventory } from '$lib/api/inventory';
import {
	assetTargetFor,
	linkProposal,
	type PurchaseTarget,
	recordIdOf,
	recordThroughProposal,
} from '$lib/features/inventory/purchaseProposals.svelte';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';

interface MinutesRead {
	sessionId: string;
	minutes: AccountSessionMinutes;
}

/** An arrival the player can decide: one stored as a proposal. */
export interface DecidableArrival {
	key: string;
	proposalId: number;
	possiblyRecorded: boolean;
	arrival: MinuteArrival;
}

/** What was decided of an arrival here, in this sitting. Stock of unknown
 * origin not `linked` stands, but the arrival was not marked done. */
export type DecidedHere =
	| { kind: 'purchase'; assetName: string }
	| { kind: 'unknownOrigin'; recordId: string; linked: boolean };

/** A purchase saved whose arrival could not be marked done. */
export interface UnlinkedPurchase {
	proposalId: number;
	recordId: string;
	assetName: string;
}

export type MinuteAction =
	| 'prepare'
	| 'purchase'
	| 'unknownOrigin'
	| 'undo'
	| 'ignore'
	| 'unignore'
	| 'link'
	| 'remove';

/** An arrival decision's proposal handle, when it has one. */
export function decidable(key: string, arrival: MinuteArrival): DecidableArrival | null {
	const disposition = arrival.disposition;
	if (disposition.state !== 'open') return null;
	return {
		key,
		proposalId: disposition.proposalId,
		possiblyRecorded: disposition.possiblyRecorded,
		arrival,
	};
}

export function createSessionMinutesModel({
	sessionId,
	onremoveloot,
}: {
	sessionId: () => string;
	/** Take every line of an item out of the shown session's loot, through
	 * the session's own loot correction; `null` where it is not offered. */
	onremoveloot: () => ((itemName: string) => Promise<void>) | null;
}) {
	let failure = $state<{ sessionId: string; message: string } | null>(null);
	const store = createSnapshotStore<MinutesRead>(ACCOUNT_TOPIC, async () => {
		const asked = sessionId();
		try {
			const minutes = await getSessionMinutes(asked);
			if (failure?.sessionId === asked) failure = null;
			return { sessionId: asked, minutes };
		} catch (cause) {
			failure = {
				sessionId: asked,
				message: describeError(cause, 'Could not read the session minute by minute.'),
			};
			throw cause;
		}
	});
	const read = $derived(store.current?.sessionId === sessionId() ? store.current : null);

	let acting = $state<{ key: string; action: MinuteAction } | null>(null);
	let rowError = $state<{ key: string; message: string } | null>(null);
	/** Decided here, by flag key; kept for this sitting. */
	let decided = $state<Record<string, DecidedHere>>({});
	/** Saved as a purchase but not marked done, by flag key. */
	let unlinked = $state<Record<string, UnlinkedPurchase>>({});
	/** Flags acted on here: the filter keeps them in view. */
	let touched = $state<string[]>([]);

	function touch(key: string) {
		if (!touched.includes(key)) touched = [...touched, key];
	}

	function without<T>(record: Record<string, T>, key: string): Record<string, T> {
		return Object.fromEntries(Object.entries(record).filter(([kept]) => kept !== key));
	}

	/** Run one action on a row: one at a time, its failure said on the row
	 * with the backend's reason, and the minutes re-read after it either
	 * way. */
	async function run(
		key: string,
		action: MinuteAction,
		work: () => Promise<void>,
		fallback: string,
	): Promise<boolean> {
		if (acting) return false;
		acting = { key, action };
		rowError = null;
		touch(key);
		try {
			await work();
			return true;
		} catch (cause) {
			rowError = { key, message: describeError(cause, fallback) };
			return false;
		} finally {
			acting = null;
			await store.hydrate();
		}
	}

	return {
		/** Whether the shown session's minutes have been read at least once. */
		get answered(): boolean {
			return read !== null;
		},
		get minutes(): AccountSessionMinutes | null {
			return read?.minutes ?? null;
		},
		/** Why the latest read of the shown session failed, while it did. */
		get error(): string | null {
			return failure?.sessionId === sessionId() ? failure.message : null;
		},
		dismissError() {
			failure = null;
		},
		get acting(): { key: string; action: MinuteAction } | null {
			return acting;
		},
		/** The action running on a row, if any. */
		actingOn(key: string): MinuteAction | null {
			return acting?.key === key ? acting.action : null;
		},
		/** Why the last action on a row failed. */
		errorOf(key: string): string | null {
			return rowError?.key === key ? rowError.message : null;
		},
		dismissRowError() {
			rowError = null;
		},
		decidedHere(key: string): DecidedHere | null {
			return decided[key] ?? null;
		},
		unlinkedOf(key: string): UnlinkedPurchase | null {
			return unlinked[key] ?? null;
		},
		get touched(): ReadonlySet<string> {
			return new Set(touched);
		},
		/** Whether the loot correction can be asked for here. */
		get removesLoot(): boolean {
			return onremoveloot() !== null;
		},
		refresh(): Promise<void> {
			return store.hydrate();
		},

		/** Where recording an arrival as a purchase would put it, from the
		 * assets held now; `null` (with why, on the row) when that cannot be
		 * read or chosen. */
		async purchaseTarget(item: DecidableArrival): Promise<PurchaseTarget | null> {
			if (acting) return null;
			acting = { key: item.key, action: 'prepare' };
			rowError = null;
			try {
				const assets = await getEquipmentInventory();
				const target = assetTargetFor(item.arrival.itemName, assets);
				if (target.kind === 'several') {
					rowError = {
						key: item.key,
						message:
							"More than one asset has this name: add it from the right asset's row in Inventory, then ignore this.",
					};
					return null;
				}
				return target;
			} catch (cause) {
				rowError = { key: item.key, message: describeError(cause, 'Could not read your assets.') };
				return null;
			} finally {
				acting = null;
			}
		},

		/**
		 * Record an arrival as a purchase through `save`, the asset form's own
		 * save. A failure rejects, so the form shows why and stays open.
		 */
		async recordPurchase(
			item: DecidableArrival,
			save: () => Promise<InventoryItem>,
		): Promise<InventoryItem> {
			if (acting && acting.key !== item.key) {
				throw new Error('Another choice is still being saved.');
			}
			acting = { key: item.key, action: 'purchase' };
			rowError = null;
			touch(item.key);
			try {
				const recorded = await recordThroughProposal(
					{ id: item.proposalId, possiblyRecorded: item.possiblyRecorded },
					save,
					recordIdOf,
				);
				if (recorded.linked) {
					decided = {
						...decided,
						[item.key]: { kind: 'purchase', assetName: recorded.saved.name },
					};
				} else {
					unlinked = {
						...unlinked,
						[item.key]: {
							proposalId: item.proposalId,
							recordId: recorded.recordId,
							assetName: recorded.saved.name,
						},
					};
				}
				return recorded.saved;
			} finally {
				acting = null;
				// Read before the form closes, so the row it closes onto shows
				// what became of the arrival.
				await store.hydrate();
			}
		},

		/** Mark a purchase already saved as recorded, without saving again. */
		async markRecorded(key: string): Promise<boolean> {
			const kept = unlinked[key];
			if (!kept) return false;
			return run(
				key,
				'link',
				async () => {
					if (!(await linkProposal(kept.proposalId, kept.recordId))) {
						throw new Error(`Still could not mark the purchase of ${kept.assetName} as recorded.`);
					}
					unlinked = without(unlinked, key);
					decided = { ...decided, [key]: { kind: 'purchase', assetName: kept.assetName } };
				},
				'Could not mark it as recorded.',
			);
		},

		/** Hold it as stock of unknown origin. */
		recordUnknownOrigin(item: DecidableArrival): Promise<boolean> {
			return run(
				item.key,
				'unknownOrigin',
				async () => {
					const { recordId, linked } = await recordArrivalUnknownOrigin({
						id: item.proposalId,
						retake: item.possiblyRecorded,
					});
					decided = { ...decided, [item.key]: { kind: 'unknownOrigin', recordId, linked } };
				},
				'Could not add it to stock.',
			);
		},

		/** Mark an arrival recorded as the stock of unknown origin already
		 * added for it, without adding it again. */
		markUnknownOriginRecorded(key: string, proposalId: number, recordId: string): Promise<boolean> {
			return run(
				key,
				'link',
				async () => {
					if (!(await linkProposal(proposalId, recordId))) {
						throw new Error('Still could not mark it as recorded.');
					}
					decided = { ...decided, [key]: { kind: 'unknownOrigin', recordId, linked: true } };
				},
				'Could not mark it as recorded.',
			);
		},

		/** Take back stock of unknown origin recorded from an arrival, by
		 * the record's id: the arrival is open to decide again. */
		undoUnknownOrigin(key: string, recordId: string): Promise<boolean> {
			return run(
				key,
				'undo',
				async () => {
					await undoArrivalUnknownOrigin({ id: recordId });
					decided = without(decided, key);
				},
				'Could not take it back.',
			);
		},

		/** Leave it: it is not offered again. */
		ignore(item: DecidableArrival): Promise<boolean> {
			return run(
				item.key,
				'ignore',
				() => dismissProposal({ id: item.proposalId }),
				'Could not ignore it.',
			);
		},

		/** Offer an ignored arrival again. */
		unignore(key: string, proposalId: number): Promise<boolean> {
			return run(
				key,
				'unignore',
				() => unignoreArrival({ id: proposalId }),
				'Could not offer it again.',
			);
		},

		/** Take the item out of the session's loot. */
		removeLoot(key: string, itemName: string): Promise<boolean> {
			const remove = onremoveloot();
			if (!remove) return Promise.resolve(false);
			return run(
				key,
				'remove',
				() => remove(itemName),
				"Could not remove it from the session's loot.",
			);
		},

		/** Attach the account topic listener; returns the detach. Reads are
		 * the caller's, keyed on the session shown. */
		connect(): () => void {
			return store.connect({ hydrate: false });
		},
	};
}

export type SessionMinutesModel = ReturnType<typeof createSessionMinutesModel>;
