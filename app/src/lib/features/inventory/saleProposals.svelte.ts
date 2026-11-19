/**
 * What looks like a sale to another player in the account readings: one
 * item leaving as the PED Card rose by more than its TT, outside every
 * tracked session. A gift paid back or a deposit landing in the same step
 * looks the same, so it is a guess, recorded only through the trade forms
 * the player uses by hand: the loot sale form of stock held under that
 * name, or the asset sale form of the one asset held under it, opened on
 * "Trade" and filled with what the readings saw, every figure editable
 * before it is saved. "Dismiss" says it is not what happened.
 *
 * The proposal is claimed just before the save and linked to the trade the
 * save recorded after it, through the one way every proposal is recorded
 * through a form (`recordThroughProposal`); a save that fails releases it,
 * and once the trade is saved nothing undoes the claim, so a re-read that
 * fails after it never offers the sale twice. A claim left with no link
 * (the app closed between the two) is shown as possibly recorded, and
 * recording one of those asks first.
 *
 * Kept current by the account topic. Read from the main window only: the
 * account commands are refused to every other window.
 */
import { ACCOUNT_TOPIC, dismissProposal, getProposals } from '$lib/api/account';
import type { AccountProposal, InventoryItem } from '$lib/api/commands.gen';
import type { TradeStart, TreeCuttingStock } from '$lib/features/analytics/treeCuttingModel.svelte';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { formatQuantity, localDateOf } from '$lib/utils/format';
import { describeError } from '$lib/view/errorState';
import { linkProposal, recordThroughProposal } from './purchaseProposals.svelte';

export type SaleProposal = AccountProposal & {
	outcome: Extract<AccountProposal['outcome'], { kind: 'sale' }>;
};

export function isSaleProposal(proposal: AccountProposal): proposal is SaleProposal {
	return proposal.outcome.kind === 'sale';
}

/** What left: "Ares Ring, Improved", or "40 Weapon Cells". */
export function saleWhat(proposal: SaleProposal): string {
	const { itemName, units } = proposal.outcome;
	return units === 1 ? itemName : `${formatQuantity(units)} ${itemName}`;
}

/** Where a sale would be recorded: the loot held under its name, the one
 * asset held under it, or nowhere the app can choose. */
export type SaleTarget =
	| { kind: 'loot'; stock: TreeCuttingStock }
	| { kind: 'asset'; asset: InventoryItem }
	| { kind: 'several' }
	| { kind: 'none' };

/** Where a sale of `itemName` would be recorded: held as loot or as one
 * asset, never both. */
export function saleTargetFor(
	itemName: string,
	loot: readonly TreeCuttingStock[],
	assets: readonly InventoryItem[],
): SaleTarget {
	const wanted = itemName.trim().toLocaleLowerCase();
	const stock = loot.filter((row) => row.itemName.trim().toLocaleLowerCase() === wanted);
	const held = assets.filter((asset) => asset.name.trim().toLocaleLowerCase() === wanted);
	if (stock.length + held.length > 1) return { kind: 'several' };
	if (stock.length === 1) return { kind: 'loot', stock: stock[0] };
	if (held.length === 1) return { kind: 'asset', asset: held[0] };
	return { kind: 'none' };
}

/** Where it goes, as the row says it. */
export function saleTargetPhrase(target: SaleTarget): string {
	switch (target.kind) {
		case 'loot':
			return 'from your loot';
		case 'asset':
			return `from your ${target.asset.name}`;
		case 'several':
			return 'held as loot and as an asset: record it from the right row, then dismiss this';
		case 'none':
			return 'not held here: record it by hand if it should be, then dismiss this';
	}
}

/** What the readings saw, to open a trade form with. Every figure stays
 * editable there. */
export function saleStart(proposal: SaleProposal): TradeStart {
	const { units, received } = proposal.outcome;
	return {
		key: `proposal-${proposal.id}`,
		quantity: units,
		soldFor: received.ped,
		soldAt: localDateOf(proposal.between.to),
	};
}

/** A sale recorded whose proposal could not be marked done. */
interface Unlinked {
	recordId: string;
}

export function createSaleProposals() {
	/** Every read forgets what a row was once its proposal is gone from
	 * the list, so one an undo opens again is recordable afresh. */
	const store = createSnapshotStore<AccountProposal[]>(ACCOUNT_TOPIC, async () => {
		const listed = await getProposals('sale');
		const open = new Set(listed.map((proposal) => proposal.id));
		settled = settled.filter((id) => open.has(id));
		unlinked = Object.fromEntries(Object.entries(unlinked).filter(([id]) => open.has(Number(id))));
		return listed;
	});
	const pending = $derived((store.current ?? []).filter(isSaleProposal));
	/** The proposal a trade form is open for, if any. */
	let opened = $state<SaleProposal | null>(null);
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

	/**
	 * Record `proposal` through `save`, the trade form's own save, which
	 * answers the sale it recorded: claim it, save, then link it to that
	 * sale. A failed save releases the claim this attempt made (one shown as
	 * possibly recorded keeps the claim it had) and rejects, so the form
	 * shows why and stays open.
	 */
	async function record(proposal: SaleProposal, save: () => Promise<string>): Promise<void> {
		if (busy() || settled.includes(proposal.id) || unlinked[proposal.id]) {
			throw new Error('This sale is already being recorded.');
		}
		recording = proposal.id;
		error = null;
		try {
			let recorded: { recordId: string; linked: boolean };
			try {
				recorded = await recordThroughProposal(proposal, save, (id) => id, 'sale');
			} catch (cause) {
				void store.hydrate();
				throw cause;
			}
			if (recorded.linked) settle(proposal.id);
			else unlinked = { ...unlinked, [proposal.id]: { recordId: recorded.recordId } };
			void store.hydrate();
		} finally {
			recording = null;
		}
	}

	return {
		/** The sales waiting, as the list comes. */
		get pending(): SaleProposal[] {
			return pending;
		},
		/** The proposal a trade form is open for, and the figures it opens with. */
		get start(): TradeStart | null {
			return opened ? saleStart(opened) : null;
		},
		isSettled(id: number): boolean {
			return settled.includes(id);
		},
		isUnlinked(id: number): boolean {
			return id in unlinked;
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
		/** Open a trade form for `proposal`; the form is the caller's. */
		begin(proposal: SaleProposal) {
			opened = proposal;
		},
		/** The trade form closed. */
		end() {
			opened = null;
		},
		/** `save` as the open proposal's, when a trade form is open for one. */
		around<Input>(save: (input: Input) => Promise<string>): (input: Input) => Promise<void> {
			return async (input) => {
				const from = opened;
				if (from) await record(from, () => save(input));
				else await save(input);
			};
		},
		/** Mark a recorded sale done without saving it again. */
		async markRecorded(proposal: SaleProposal): Promise<void> {
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
					error = `Still could not mark the sale of ${proposal.outcome.itemName} as recorded.`;
				}
			} finally {
				linking = null;
			}
		},
		/** "Not a sale". */
		async dismiss(proposal: SaleProposal): Promise<void> {
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

export type SaleProposals = ReturnType<typeof createSaleProposals>;
