/**
 * What looks like a purchase in the account readings: one item arriving as
 * the PED Card fell by more than its TT, outside every tracked session. It
 * may be a purchase at auction or a private trade paid in PED, so it is a
 * guess, recorded only through the asset forms the player uses by hand:
 * the purchase form of an asset held under that name, or the form that adds
 * a new asset, filled with what the readings saw and editable before it is
 * saved. "Dismiss" says it is not what happened.
 *
 * The proposal is claimed just before the save, and linked to what the save
 * recorded after it; a save that fails releases it. A link that fails is
 * said on the row, with a way to mark it recorded without saving again. A
 * claim left with no link (the app closed between the two) is kept by the
 * backend and shown as possibly recorded, and recording one of those asks
 * first.
 *
 * Kept current by the account topic. Read from the main window only: the
 * account commands are refused to every other window.
 */
import {
	ACCOUNT_TOPIC,
	claimProposals,
	dismissProposal,
	getProposals,
	linkProposals,
	releaseProposals,
} from '$lib/api/account';
import type { AccountProposal, InventoryItem } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { formatQuantity, localDateOf } from '$lib/utils/format';
import { describeError } from '$lib/view/errorState';

export type PurchaseProposal = AccountProposal & {
	outcome: Extract<AccountProposal['outcome'], { kind: 'purchase' }>;
};

export function isPurchaseProposal(proposal: AccountProposal): proposal is PurchaseProposal {
	return proposal.outcome.kind === 'purchase';
}

/** What arrived: "Ares Ring, Improved", or "40 Weapon Cells". */
export function purchaseWhat(proposal: PurchaseProposal): string {
	const { itemName, units } = proposal.outcome;
	return units === 1 ? itemName : `${formatQuantity(units)} ${itemName}`;
}

/** Where a purchase would be recorded: more of the one asset held under
 * its name that can take more, a new asset, or nowhere the app can choose
 * between. */
export type PurchaseTarget =
	| { kind: 'asset'; asset: InventoryItem }
	| { kind: 'new' }
	| { kind: 'several' };

/** Where more of an item named `itemName` would be recorded. Only an asset
 * that can be bought more of is a match: one a conversion made keeps the
 * count it was made as. */
export function assetTargetFor(itemName: string, assets: InventoryItem[]): PurchaseTarget {
	const wanted = itemName.trim().toLocaleLowerCase();
	const matches = assets.filter(
		(asset) => asset.divisible && asset.name.trim().toLocaleLowerCase() === wanted,
	);
	if (matches.length > 1) return { kind: 'several' };
	return matches.length === 1 ? { kind: 'asset', asset: matches[0] } : { kind: 'new' };
}

export function purchaseTarget(
	proposal: PurchaseProposal,
	assets: InventoryItem[],
): PurchaseTarget {
	return assetTargetFor(proposal.outcome.itemName, assets);
}

/** Where it goes, as the row says it. */
export function targetPhrase(target: PurchaseTarget): string {
	switch (target.kind) {
		case 'asset':
			return `adds to your ${target.asset.name}`;
		case 'new':
			return 'as a new asset';
		case 'several':
			return "more than one asset has this name: add it from the right asset's row, then dismiss this";
	}
}

/** What the readings saw, to fill the form with. Every figure stays
 * editable there. */
export interface PurchaseDraft {
	/** Tells one proposal's fill from another's. */
	key: string;
	name: string;
	quantity: number;
	ttValue: number;
	markupPaid: number;
	acquiredAt: string;
}

export function purchaseDraft(proposal: PurchaseProposal): PurchaseDraft {
	const { itemName, units, tt, markup } = proposal.outcome;
	return {
		key: `proposal-${proposal.id}`,
		name: itemName,
		quantity: units,
		ttValue: tt.ped,
		markupPaid: markup.ped,
		acquiredAt: localDateOf(proposal.between.to),
	};
}

/** What a recorded purchase's proposal is linked to: the purchase just
 * recorded, which is the asset's purchase with the highest id (purchases are
 * listed by date, so never simply the last). */
export function recordIdOf(saved: InventoryItem): string {
	const newest = saved.purchases.reduce<number | null>(
		(highest, purchase) => (highest === null || purchase.id > highest ? purchase.id : highest),
		null,
	);
	if (newest === null) throw new Error(`${saved.name} holds no purchase to link to`);
	return String(newest);
}

/** How many times linking a proposal to its record is tried at once. */
const LINK_ATTEMPTS = 2;

/** Link a proposal to the record it became, trying once more on a
 * failure; whether it was linked. */
export async function linkProposal(id: number, recordId: string): Promise<boolean> {
	for (let attempt = 0; attempt < LINK_ATTEMPTS; attempt += 1) {
		try {
			await linkProposals({ ids: [id], recordId });
			return true;
		} catch {
			// Tried once more; then the caller says so.
		}
	}
	return false;
}

/** What recording a proposal through a form did. */
export interface ProposalRecorded<Saved = InventoryItem> {
	saved: Saved;
	/** The record it was linked to, or is to be. */
	recordId: string;
	/** False when the record stands but the proposal could not be marked
	 * done: the caller offers to mark it without saving again. */
	linked: boolean;
}

/**
 * Record a proposal through `save`, a form's own save: claim it, save,
 * then link it to the record saved, named by `idOf`. A claim that fails
 * rejects with why; a save that fails releases the claim this attempt
 * made (one shown as possibly recorded keeps the claim it had, which it
 * took over) and rejects with the save's own error, so the form shows it
 * and stays open. Once the save returns, it is never released: whatever
 * follows, it is never recorded again from here. Every proposal recorded
 * through a form goes this way: a purchase seen outside a session or an
 * arrival inside one (an asset with a purchase, `idOf` its newest
 * purchase), and a sale to another player (the trade's own id).
 */
export async function recordThroughProposal<Saved>(
	proposal: { id: number; possiblyRecorded: boolean },
	save: () => Promise<Saved>,
	idOf: (saved: Saved) => string,
	noun = 'purchase',
): Promise<ProposalRecorded<Saved>> {
	// One shown as possibly recorded takes over its old claim: the player
	// was asked first.
	const retake = proposal.possiblyRecorded ? [proposal.id] : [];
	try {
		await claimProposals({ ids: [proposal.id], retake });
	} catch (cause) {
		throw new Error(describeError(cause, `Could not set the ${noun} aside to record`));
	}
	let saved: Saved;
	try {
		saved = await save();
	} catch (cause) {
		if (!proposal.possiblyRecorded) {
			await releaseProposals({ ids: [proposal.id] }).catch(() => undefined);
		}
		throw cause;
	}
	// Recorded: whatever follows, it is never recorded again from here.
	const recordId = idOf(saved);
	return { saved, recordId, linked: await linkProposal(proposal.id, recordId) };
}

/** A purchase recorded whose proposal could not be marked done. */
interface Unlinked {
	recordId: string;
	assetName: string;
}

export function createPurchaseProposals({
	onrecorded,
}: {
	/** Re-read whatever shows the assets, once a purchase is recorded. */
	onrecorded: () => Promise<void>;
}) {
	/** Every read forgets what a row was once its proposal is gone from
	 * the list, so one an undo opens again is recordable afresh. */
	const store = createSnapshotStore<AccountProposal[]>(ACCOUNT_TOPIC, async () => {
		const listed = await getProposals('purchase');
		const open = new Set(listed.map((proposal) => proposal.id));
		settled = settled.filter((id) => open.has(id));
		unlinked = Object.fromEntries(Object.entries(unlinked).filter(([id]) => open.has(Number(id))));
		return listed;
	});
	const pending = $derived((store.current ?? []).filter(isPurchaseProposal));
	let recording = $state<number | null>(null);
	let dismissing = $state<number | null>(null);
	let linking = $state<number | null>(null);
	/** Recorded or dismissed: kept still until a re-read shows it gone. */
	let settled = $state<number[]>([]);
	/** Recorded, but not marked done; kept while it is listed. */
	let unlinked = $state<Record<number, Unlinked>>({});
	let error = $state<string | null>(null);

	const busy = () => recording !== null || dismissing !== null || linking !== null;

	function settle(id: number) {
		if (!settled.includes(id)) settled = [...settled, id];
	}

	return {
		/** The purchases waiting, as the list comes. */
		get pending(): PurchaseProposal[] {
			return pending;
		},
		/** Whether a row is done with and only waits for the list to drop it. */
		isSettled(id: number): boolean {
			return settled.includes(id);
		},
		/** A row recorded but not marked done: where it was recorded. */
		unlinkedOf(id: number): Unlinked | null {
			return unlinked[id] ?? null;
		},
		get recording(): number | null {
			return recording;
		},
		get dismissing(): number | null {
			return dismissing;
		},
		get linking(): number | null {
			return linking;
		},
		/** Whether any row is mid-action. */
		get busy(): boolean {
			return busy();
		},
		get error(): string | null {
			return error;
		},
		dismissError() {
			error = null;
		},
		refresh(): Promise<void> {
			return store.hydrate();
		},

		/**
		 * Record a proposal through `save`, the asset form's own save: claim
		 * it, save, then link it to what was saved. A failed save releases
		 * the claim this attempt made and rejects, so the form shows why and
		 * stays open; one shown as possibly recorded keeps the claim it had.
		 */
		async record(
			proposal: PurchaseProposal,
			save: () => Promise<InventoryItem>,
		): Promise<InventoryItem> {
			if (busy() || settled.includes(proposal.id) || unlinked[proposal.id]) {
				throw new Error('This purchase is already being recorded.');
			}
			recording = proposal.id;
			error = null;
			try {
				let recorded: ProposalRecorded;
				try {
					recorded = await recordThroughProposal(proposal, save, recordIdOf);
				} catch (cause) {
					void store.hydrate();
					throw cause;
				}
				const { saved, recordId, linked } = recorded;
				if (linked) settle(proposal.id);
				else unlinked = { ...unlinked, [proposal.id]: { recordId, assetName: saved.name } };
				await onrecorded().catch(() => undefined);
				void store.hydrate();
				return saved;
			} finally {
				recording = null;
			}
		},

		/** Mark a recorded purchase done without saving it again. */
		async markRecorded(proposal: PurchaseProposal): Promise<void> {
			const kept = unlinked[proposal.id];
			if (!kept || busy()) return;
			linking = proposal.id;
			error = null;
			try {
				if (await linkProposal(proposal.id, kept.recordId)) {
					unlinked = Object.fromEntries(
						Object.entries(unlinked).filter(([id]) => Number(id) !== proposal.id),
					);
					settle(proposal.id);
					void store.hydrate();
				} else {
					error = `Still could not mark the purchase of ${kept.assetName} as recorded.`;
				}
			} finally {
				linking = null;
			}
		},

		/** "Not a purchase". */
		async dismiss(proposal: PurchaseProposal): Promise<void> {
			if (busy() || settled.includes(proposal.id)) return;
			dismissing = proposal.id;
			error = null;
			try {
				await dismissProposal({ id: proposal.id });
				settle(proposal.id);
				void store.hydrate();
			} catch (cause) {
				error = describeError(cause, 'Failed to dismiss it');
			} finally {
				dismissing = null;
			}
		},

		/** Attach the topic listener, then read; returns the detach. */
		connect(): () => void {
			return store.connect();
		},
	};
}

export type PurchaseProposals = ReturnType<typeof createPurchaseProposals>;
