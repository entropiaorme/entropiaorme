<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import type { AssetPurchase, InventoryItem } from '$lib/api';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import Tabs from '$lib/components/Tabs.svelte';
	import ActivityHistory from '$lib/features/analytics/ActivityHistory.svelte';
	import AdjustStockModal from '$lib/features/analytics/AdjustStockModal.svelte';
	import AuctionListings from '$lib/features/analytics/AuctionListings.svelte';
	import ConvertStockModal from '$lib/features/analytics/ConvertStockModal.svelte';
	import { createListingProposals } from '$lib/features/analytics/listingProposals.svelte';
	import SellStockModal from '$lib/features/analytics/SellStockModal.svelte';
	import AssetForms from '$lib/features/inventory/AssetForms.svelte';
	import { createAssetForms } from '$lib/features/inventory/assetForms.svelte';
	import ConvertItemsModal from '$lib/features/inventory/ConvertItemsModal.svelte';
	import CreateListingModal from '$lib/features/inventory/CreateListingModal.svelte';
	import EquipmentSaleModal from '$lib/features/inventory/EquipmentSaleModal.svelte';
	import InventoryHoldings from '$lib/features/inventory/InventoryHoldings.svelte';
	import PurchaseProposals from '$lib/features/inventory/PurchaseProposals.svelte';
	import { createPurchaseProposals } from '$lib/features/inventory/purchaseProposals.svelte';
	import {
		createInventoryModel,
		type InventoryKind,
		type InventoryView,
	} from '$lib/features/inventory/inventoryModel.svelte';
	import type {
		ConfidenceMode,
		TreeCuttingStock,
	} from '$lib/features/analytics/treeCuttingModel.svelte';

	const model = createInventoryModel();
	const listingProposals = createListingProposals();
	const purchaseProposals = createPurchaseProposals({
		onrecorded: () => model.refresh(),
	});
	const forms = createAssetForms({
		proposals: purchaseProposals,
		addPurchase: model.addPurchase,
		updatePurchase: model.updatePurchase,
		refresh: model.refresh,
	});
	let createListingOpen = $state(false);
	let equipmentToSell = $state<InventoryItem | null>(null);
	let lootToSell = $state<TreeCuttingStock | null>(null);
	let lootToConvert = $state<TreeCuttingStock | null>(null);
	let lootToRemove = $state<TreeCuttingStock | null>(null);
	let shrapnelToConvert = $state<TreeCuttingStock | null>(null);

	/** The holding a form was opened on, as the inventory holds it now: the
	 * account readings can record a conversion or sale while the form is
	 * open, and the amount held follows. Once none is held, the form keeps
	 * what it opened with. */
	function live(row: TreeCuttingStock | null): TreeCuttingStock | null {
		if (!row) return null;
		return model.loot.find((held) => held.itemName === row.itemName) ?? row;
	}
	let convertOpen = $state(false);
	let convertStart = $state<{ name: string; assetId: string | null } | null>(null);

	const kindOptions = [
		{ id: 'loot', label: 'Loot' },
		{ id: 'equipment', label: 'Assets' },
	];
	const tabs = [
		{ id: 'holdings', label: 'Assets' },
		{ id: 'listings', label: 'Listings' },
		{ id: 'history', label: 'History' },
	];

	onMount(() => {
		void model.load();
		// Market data moves from other windows too (a markup set on the
		// dashboard, a paste on the Market page): follow every write.
		let stop: (() => void) | undefined;
		let disposed = false;
		void model
			.subscribe()
			.then((unlisten) => {
				if (disposed) unlisten();
				else stop = unlisten;
			})
			.catch(() => undefined);
		// What the account readings show became of open listings.
		const detachProposals = listingProposals.connect();
		// What looks like a purchase in the readings, waiting to be recorded.
		const detachPurchases = purchaseProposals.connect();
		return () => {
			disposed = true;
			stop?.();
			detachProposals();
			detachPurchases();
		};
	});

	function selectView(id: string) {
		model.view = id as InventoryView;
		if (id === 'history' && !model.historyLoaded && !model.historyLoading) {
			void model.loadHistory().catch(() => undefined);
		}
	}

	function startConversion(start: { name: string; assetId: string | null } | null) {
		convertStart = start;
		convertOpen = true;
		// "Same as last time" reads the history, which loads on first need.
		if (!model.historyLoaded && !model.historyLoading) {
			void model.loadHistory().catch(() => undefined);
		}
	}

	async function deleteEquipment(item: InventoryItem) {
		if (!window.confirm(`Remove ${item.name} from your assets?`)) return;
		await model.deleteEquipment(item).catch(() => undefined);
	}

	async function deletePurchase(item: InventoryItem, purchase: AssetPurchase) {
		if (!window.confirm(`Remove this purchase of ${item.name}?`)) return;
		await model.deletePurchase(purchase.id).catch(() => undefined);
	}
</script>

<div class="flex h-full min-h-0 flex-col gap-6 px-6 pb-6">
	<header class="shrink-0">
		<div class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold tracking-tight text-text">Inventory</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="mt-0.5 text-sm text-text-secondary">
				Manage your loot and assets; review activity performance in Analytics.
			</p>
		</div>
	</header>

	<Tabs {tabs} active={model.view} onchange={selectView} class="shrink-0" />

	<ErrorNotice message={model.error} />

	<div class="min-h-0 flex-1">
		{#if model.loading}
			<p class="py-16 text-center text-sm text-text-tertiary">Reading inventory...</p>
		{:else if model.view === 'holdings'}
			<div class="flex h-full min-h-0 flex-col gap-5">
				<PurchaseProposals
					model={purchaseProposals}
					assets={model.equipment}
					onrecord={forms.recordProposal}
				/>
				<div class="min-h-0 flex-1">
					<InventoryHoldings
						kind={model.kind}
						onkindchange={(id) => (model.kind = id as InventoryKind)}
						confidenceMode={model.confidenceMode}
						onconfidencechange={(id) => (model.confidenceMode = id as ConfidenceMode)}
						packetFeeSharePct={model.packetFeeSharePct}
						onpacketfeesharechange={model.setPacketFeeSharePct}
						loot={model.loot}
						equipment={model.equipment}
						onsellloot={(item) => (lootToSell = item)}
						onconvert={(item) => (lootToConvert = item)}
						onremove={(item) => (lootToRemove = item)}
						onshrapnel={(item) => (shrapnelToConvert = item)}
						onconvertitems={startConversion}
						onopenboxes={() => goto('/inventory/strongboxes')}
						onreconcile={() => goto('/inventory/reconcile')}
						onaddequipment={forms.addAsset}
						oncreatelisting={() => (createListingOpen = true)}
						oneditequipment={forms.editAsset}
						onsellequipment={(item) => (equipmentToSell = item)}
						ondeleteequipment={deleteEquipment}
						onaddpurchase={forms.buyMore}
						oneditpurchase={forms.editPurchase}
						ondeletepurchase={deletePurchase}
					/>
				</div>
			</div>
		{:else if model.view === 'listings'}
			<div class="py-3">
			<SegmentedControl
				options={kindOptions}
				active={model.kind}
				size="md"
				onchange={(id) => (model.kind = id as InventoryKind)}
			/>
			</div>
			<AuctionListings
			open={model.openListings}
			resolved={model.resolvedListings}
			onresolve={model.resolveListing}
			proposals={listingProposals}
			central
			embedded
			/>
		{:else}
			<div class="py-3">
			<SegmentedControl
				options={kindOptions}
				active={model.kind}
				size="md"
				onchange={(id) => (model.kind = id as InventoryKind)}
			/>
			</div>
			<ActivityHistory
			entries={model.history}
			loading={model.historyLoading}
			onundo={model.undo}
			onundocheck={model.undoCheck}
			embedded
			/>
		{/if}
	</div>
</div>

<CreateListingModal
	bind:open={createListingOpen}
	holdings={model.holdingOptions}
	onresolve={model.resolveDraftName}
	oncapture={model.captureSaleWindow}
	oncollect={model.takeSaleWindowCapture}
	onsubmit={model.createFromDraft}
/>
<SellStockModal
	item={live(lootToSell)}
	onlist={model.listLoot}
	ontrade={model.sellLootByTrade}
	oncancel={() => (lootToSell = null)}
	activityAttributionNoun="its source activities"
/>
<ConvertItemsModal
	bind:open={convertOpen}
	items={model.convertibleItems}
	start={convertStart}
	history={model.allHistory}
	onsubmit={model.convert}
/>
<ConvertStockModal
	item={live(lootToConvert)}
	onconvert={model.recycle}
	oncancel={() => (lootToConvert = null)}
/>
<AdjustStockModal
	item={live(lootToRemove)}
	mode="remove"
	onconfirm={model.remove}
	oncancel={() => (lootToRemove = null)}
/>
<AdjustStockModal
	item={live(shrapnelToConvert)}
	mode="shrapnel"
	onconfirm={(_itemName, quantity) => model.shrapnel(quantity)}
	oncancel={() => (shrapnelToConvert = null)}
/>
<EquipmentSaleModal
	item={equipmentToSell}
	onlist={model.listEquipment}
	ontrade={model.sellEquipmentByTrade}
	oncancel={() => (equipmentToSell = null)}
/>
<AssetForms {forms} />
