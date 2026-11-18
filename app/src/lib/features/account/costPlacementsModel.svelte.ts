/**
 * The player's placements of items the app cannot place: the items still
 * awaiting one, the placements made, and placing or clearing an item.
 *
 * A placement is made once and holds for every session: the backend
 * rebuilds every measured session's costs by it before it answers, so the
 * surface that placed an item re-reads its own figures only once the
 * answer is in (`onChanged`). One item is placed at a time per surface,
 * and every trigger waits meanwhile; a failure keeps the backend's reason
 * beside the item it was for until the next attempt. Once the figures are
 * read again, focus returns to the item wherever it now sits, and a live
 * region says what was done.
 *
 * The list is re-read on every account frame, so an item a newly measured
 * session could not place appears without a reload.
 */

import { tick } from 'svelte';
import {
	ACCOUNT_TOPIC,
	clearCostPlacement,
	getCostPlacements,
	placeCostItem,
} from '$lib/api/account';
import type { CostPlacementCategory, CostPlacements } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';
import { PLACEMENT_LABELS } from './costLines';

/** A placement made or cleared: the item, where it was placed (`null` for
 * a clear), and a count that tells one placement from the next. */
export interface LastPlacement {
	item: string;
	category: CostPlacementCategory | null;
	seq: number;
}

export function createCostPlacementsModel(
	options: {
		/** Read again whatever the placement moved; awaited before focus
		 * returns. */
		onChanged?: (item: string) => Promise<void> | void;
		/** Where focus goes when the placed item is no longer listed: an
		 * element that outlives the list it left. */
		focusFallback?: () => HTMLElement | null;
	} = {},
) {
	let readError = $state<string | null>(null);
	let pending = $state<string | null>(null);
	let failure = $state<{ item: string; message: string } | null>(null);
	let lastPlacement = $state<LastPlacement | null>(null);
	let announcement = $state('');
	let placements = $state<CostPlacements | null>(null);
	// Bumped by every placement's answer: a read begun before it answers
	// what the list was, so it never lands after it and puts a placed item
	// back among those awaiting one.
	let generation = 0;

	const store = createSnapshotStore<CostPlacements>(ACCOUNT_TOPIC, async () => {
		const begun = generation;
		try {
			const read = await getCostPlacements();
			readError = null;
			if (begun === generation) placements = read;
			return read;
		} catch (cause) {
			readError = describeError(cause, 'Could not read the items to place.');
			throw cause;
		}
	});

	async function settle(
		item: string,
		category: CostPlacementCategory | null,
		act: () => Promise<{ placements: CostPlacements }>,
		fallback: string,
	): Promise<boolean> {
		// One placement at a time: every trigger is disabled meanwhile, so a
		// second choice is never taken and dropped.
		if (pending !== null) return false;
		pending = item;
		failure = null;
		let placed = false;
		try {
			const result = await act();
			generation += 1;
			placements = result.placements;
			lastPlacement = { item, category, seq: (lastPlacement?.seq ?? 0) + 1 };
			await options.onChanged?.(item);
			announcement = placementAnnouncement(category);
			placed = true;
		} catch (cause) {
			failure = { item, message: describeError(cause, fallback) };
		} finally {
			pending = null;
		}
		if (placed) await returnFocusToPlacement(item, options.focusFallback?.() ?? null);
		return placed;
	}

	return {
		/** The items awaiting a placement and the placements made; `null`
		 * until first read. */
		get placements(): CostPlacements | null {
			return placements;
		},
		/** Whether the list has been read at least once. */
		get answered(): boolean {
			return placements !== null;
		},
		/** Whether the player has placed anything, or anything awaits a
		 * placement. */
		get holdsAny(): boolean {
			return !!placements && (placements.awaiting.length > 0 || placements.placed.length > 0);
		},
		/** Why the list could not be read, while it could not. */
		get error(): string | null {
			return placements === null ? readError : null;
		},
		/** The item being placed or cleared now, if any. */
		get pending(): string | null {
			return pending;
		},
		/** Why placing or clearing an item failed, with the item it was for. */
		get failure(): { item: string; message: string } | null {
			return failure;
		},
		/** The placement made or cleared last (`category` is `null` for a
		 * clear), so a surface can keep the item in view wherever it now
		 * sits. */
		get lastPlacement(): LastPlacement | null {
			return lastPlacement;
		},
		/** What the last placement did, for a polite live region. */
		get announcement(): string {
			return announcement;
		},
		/** Where the player placed an item; `null` when they have not. */
		placedAs(item: string): CostPlacementCategory | null {
			return placements?.placed.find((entry) => entry.item === item)?.category ?? null;
		},
		/** Place an item for every session; whether it was placed. Focus
		 * returns to the item once the figures it moved are read again. */
		place(item: string, category: CostPlacementCategory): Promise<boolean> {
			return settle(
				item,
				category,
				() => placeCostItem({ item, category }),
				'Could not place the item.',
			);
		},
		/** Clear an item's placement; whether it was cleared. */
		clear(item: string): Promise<boolean> {
			return settle(item, null, () => clearCostPlacement(item), 'Could not clear the placement.');
		},
		refresh(): Promise<void> {
			return store.hydrate();
		},
		/** Attach the account topic listener; returns the detach. */
		connect(): () => void {
			let detach: (() => void) | undefined;
			let closed = false;
			void store.subscribe().then((unlisten) => {
				if (closed) unlisten();
				else detach = unlisten;
			});
			return () => {
				closed = true;
				detach?.();
			};
		},
	};
}

/** What a placement did, in a few words: "Placed as Weapon", "Placed as
 * not a cost", or, for a clear, "Placement cleared". */
export function placementAnnouncement(category: CostPlacementCategory | null): string {
	if (category === null) return 'Placement cleared';
	if (category === 'notACost') return 'Placed as not a cost';
	return `Placed as ${PLACEMENT_LABELS[category]}`;
}

export type CostPlacementsModel = ReturnType<typeof createCostPlacementsModel>;

/** Hand focus back once a placement has settled and the figures it moved
 * have been read again: to the item's own trigger, wherever the item now
 * sits, or, where it is no longer listed, to `fallback`. */
export async function returnFocusToPlacement(
	item: string,
	fallback: HTMLElement | null,
): Promise<void> {
	// Twice: once for the figures read again, once for any list a surface
	// opened in answer to them.
	await tick();
	await tick();
	const trigger = Array.from(document.querySelectorAll<HTMLElement>('[data-place-item]')).find(
		(element) => element.dataset.placeItem === item && !element.hasAttribute('disabled'),
	);
	(trigger ?? fallback)?.focus();
}
