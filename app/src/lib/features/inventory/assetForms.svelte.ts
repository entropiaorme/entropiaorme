/**
 * The asset forms Inventory opens: adding or correcting an asset, and
 * buying into one or correcting a purchase of it. Either can open filled
 * in from a purchase the account readings proposed; then it is recorded as
 * that proposal's, claimed before the save and linked after it.
 */

import type { AssetPurchase, AssetPurchaseInput, InventoryItem } from '$lib/api/commands.gen';
import { addAssetPurchase } from '$lib/api/inventory';
import {
	type PurchaseDraft,
	type PurchaseProposal,
	type PurchaseProposals,
	type PurchaseTarget,
	purchaseDraft,
} from './purchaseProposals.svelte';

/** A proposal and the figures its form opens with. */
interface FromProposal {
	proposal: PurchaseProposal;
	draft: PurchaseDraft;
}

export interface AssetFormsOptions {
	proposals: PurchaseProposals;
	addPurchase: (item: InventoryItem, input: AssetPurchaseInput) => Promise<void>;
	updatePurchase: (purchaseId: number, input: AssetPurchaseInput) => Promise<void>;
	/** Re-read the inventory once an asset is saved. */
	refresh: () => Promise<void>;
}

export function createAssetForms(options: AssetFormsOptions) {
	let assetOpen = $state(false);
	let assetToEdit = $state<InventoryItem | null>(null);
	/** The account proposal a new asset is filled in from, if any. */
	let newAssetFrom = $state<FromProposal | null>(null);
	/** The asset being bought into, the purchase being corrected if any, and
	 * the account proposal a new purchase is filled in from, if any. */
	let purchaseFor = $state<{
		item: InventoryItem;
		purchase: AssetPurchase | null;
		proposal?: FromProposal;
	} | null>(null);

	function openAsset(item: InventoryItem | null, from: FromProposal | null) {
		assetToEdit = item;
		newAssetFrom = from;
		assetOpen = true;
	}

	/** Open the form a proposal is recorded through, filled in. */
	function recordProposal(proposal: PurchaseProposal, target: PurchaseTarget) {
		const draft = purchaseDraft(proposal);
		if (target.kind === 'asset') {
			purchaseFor = { item: target.asset, purchase: null, proposal: { proposal, draft } };
		} else if (target.kind === 'new') {
			openAsset(null, { proposal, draft });
		}
	}

	async function savePurchase(input: AssetPurchaseInput) {
		if (!purchaseFor) return;
		const { item, purchase, proposal } = purchaseFor;
		if (proposal) {
			// Recorded as the proposal's: set aside before the save, marked
			// after it; the inventory re-reads once it is.
			await options.proposals.record(proposal.proposal, () => addAssetPurchase(item.id, input));
			return;
		}
		await (purchase
			? options.updatePurchase(purchase.id, input)
			: options.addPurchase(item, input));
	}

	/** How a new asset's save is wrapped: as its proposal's, when it has one. */
	const around = $derived(
		newAssetFrom
			? (save: () => Promise<InventoryItem>) => {
					const from = newAssetFrom;
					return from ? options.proposals.record(from.proposal, save) : save();
				}
			: null,
	);

	return {
		get assetOpen() {
			return assetOpen;
		},
		set assetOpen(open: boolean) {
			assetOpen = open;
		},
		get assetToEdit() {
			return assetToEdit;
		},
		/** The figures a new asset opens with, from its proposal. */
		get assetPrefill() {
			return newAssetFrom?.draft ?? null;
		},
		get around() {
			return around;
		},
		get purchaseItem() {
			return purchaseFor?.item ?? null;
		},
		get purchase() {
			return purchaseFor?.purchase ?? null;
		},
		get purchasePrefill() {
			return purchaseFor?.proposal?.draft ?? null;
		},
		addAsset: () => openAsset(null, null),
		editAsset: (item: InventoryItem) => openAsset(item, null),
		buyMore(item: InventoryItem) {
			purchaseFor = { item, purchase: null };
		},
		editPurchase(item: InventoryItem, purchase: AssetPurchase) {
			purchaseFor = { item, purchase };
		},
		recordProposal,
		savePurchase,
		closePurchase() {
			purchaseFor = null;
		},
		assetSaved() {
			newAssetFrom = null;
			void options.refresh().catch(() => undefined);
		},
	};
}

export type AssetForms = ReturnType<typeof createAssetForms>;
