<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import { filterByQuery, isSearchQuery } from '$lib/view/search';
	import ExpandingActionButton from '$lib/components/ExpandingActionButton.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import type { InventoryItem } from '$lib/api';
	import MarkupConfidenceInfo from '$lib/features/analytics/MarkupConfidenceInfo.svelte';
	import MarkupConfidenceSelector from '$lib/features/analytics/MarkupConfidenceSelector.svelte';
	import TreeCuttingStockView from '$lib/features/analytics/TreeCuttingStock.svelte';
	import type {
		ConfidenceMode,
		TreeCuttingStock,
	} from '$lib/features/analytics/treeCuttingModel.svelte';
	import { formatLedgerDate, formatPed } from '$lib/utils/format';
	import type { InventoryKind } from './inventoryModel.svelte';
	import PacketFeeInfo from './PacketFeeInfo.svelte';
	import PacketFeeSelector from './PacketFeeSelector.svelte';

	let {
		kind,
		onkindchange,
		confidenceMode,
		onconfidencechange,
		packetFeeSharePct,
		onpacketfeesharechange,
		loot,
		equipment,
		onsellloot,
		onconvert,
		onremove,
		onshrapnel,
		onaddequipment,
		oncreatelisting,
		oneditequipment,
		onsellequipment,
		ondeleteequipment,
	}: {
		kind: InventoryKind;
		onkindchange: (id: string) => void;
		confidenceMode: ConfidenceMode;
		onconfidencechange: (id: string) => void;
		packetFeeSharePct: number;
		onpacketfeesharechange: (value: number) => void | Promise<void>;
		loot: TreeCuttingStock[];
		equipment: InventoryItem[];
		onsellloot: (item: TreeCuttingStock) => void;
		onconvert: (item: TreeCuttingStock) => void;
		onremove: (item: TreeCuttingStock) => void;
		onshrapnel: (item: TreeCuttingStock) => void;
		onaddequipment: () => void;
		oncreatelisting: () => void;
		oneditequipment: (item: InventoryItem) => void;
		onsellequipment: (item: InventoryItem) => void;
		ondeleteequipment: (item: InventoryItem) => void;
	} = $props();

	let query = $state('');
	const visibleEquipment = $derived(filterByQuery(equipment, query, (item) => item.name));
	const kindOptions = [
		{ id: 'loot', label: 'Loot' },
		{ id: 'equipment', label: 'Assets' },
	];
</script>

<section
	aria-label={kind === 'loot' ? 'Loot inventory' : 'Assets'}
	class="flex h-full min-h-0 flex-col"
>
	{#if kind === 'loot'}
		<TreeCuttingStockView
			stock={loot}
			onsell={onsellloot}
			onconvert={onconvert}
			onremove={onremove}
			onshrapnelconvert={onshrapnel}
			actionLayout="inventory"
			alwaysSearch
			fillAvailable
			emptyMessage="No tracked loot is currently held."
			sourceDescription="Loot pooled across every tracked profession, minus stock you have sold, converted, or removed."
		>
			{#snippet heading()}
				<SegmentedControl
					options={kindOptions}
					active={kind}
					size="md"
					onchange={onkindchange}
				/>
			{/snippet}
			{#snippet controlsLabel()}
				<MarkupConfidenceInfo />
			{/snippet}
			{#snippet controls()}
				<div class="flex flex-wrap items-center gap-x-4 gap-y-2">
					<MarkupConfidenceSelector active={confidenceMode} onchange={onconfidencechange} />
					<div class="flex items-center gap-2.5">
						<PacketFeeInfo />
						<PacketFeeSelector value={packetFeeSharePct} onchange={onpacketfeesharechange} />
					</div>
				</div>
			{/snippet}
			{#snippet actions()}
				<Button size="sm" onclick={oncreatelisting}>Create listing</Button>
			{/snippet}
		</TreeCuttingStockView>
	{:else}
		<div
			class="grid shrink-0 grid-cols-[minmax(0,1fr)_auto] items-start gap-x-5 pb-5"
			data-testid="equipment-utility-strip"
		>
			<div class="flex min-w-0 flex-col items-start gap-3">
				<SegmentedControl
					options={kindOptions}
					active={kind}
					size="md"
					onchange={onkindchange}
				/>
				<SearchInput
					class="w-full sm:w-64"
					bind:value={query}
					placeholder="Find an item"
					aria-label="Find an item"
				/>
			</div>
			<div class="flex h-full items-end justify-end gap-2">
				<Button size="sm" variant="secondary" onclick={onaddequipment}>Add asset</Button>
				<Button size="sm" onclick={oncreatelisting}>Create listing</Button>
			</div>
		</div>

		<div
			class="flex min-h-0 flex-1 flex-col"
			role="table"
			aria-label="Assets"
		>
			<div
				class="flex shrink-0 items-center gap-3 border-b border-border px-2.5 py-2 text-text-tertiary"
				role="row"
			>
				<span class="eyebrow min-w-0 flex-1" role="columnheader">Asset</span>
				<span class="eyebrow w-24 shrink-0 text-right" role="columnheader">TT</span>
				<span class="eyebrow w-24 shrink-0 text-right" role="columnheader">MU paid</span>
				<span class="eyebrow w-24 shrink-0 text-right" role="columnheader">Total cost</span>
				<span class="eyebrow w-28 shrink-0 text-right" role="columnheader">Acquired</span>
				<span class="eyebrow w-[5.25rem] shrink-0 text-right" role="columnheader">Actions</span>
			</div>

		<ul
			class="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto"
			role="rowgroup"
			data-testid="equipment-scroll-list"
		>
			{#each visibleEquipment as item (item.id)}
				<li class="flex items-center gap-3 rounded-md px-2.5 py-2" role="row">
					<div class="min-w-0 flex-1" role="cell">
						<p class="truncate text-sm font-medium tracking-tight text-text">{item.name}</p>
						{#if item.notes}
							<p class="mt-0.5 truncate text-xs text-text-tertiary">{item.notes}</p>
						{/if}
					</div>
					<span class="w-24 shrink-0 text-right text-sm tabular-nums text-text-secondary" role="cell">
						{formatPed(item.ttValue)}
					</span>
					<span class="w-24 shrink-0 text-right text-sm tabular-nums text-text-secondary" role="cell">
						{formatPed(item.markupPaid)}
					</span>
					<span class="w-24 shrink-0 text-right text-sm font-medium tabular-nums text-text" role="cell">
						{formatPed(item.ttValue + item.markupPaid)}
					</span>
					<span class="w-28 shrink-0 text-right text-xs tabular-nums text-text-tertiary" role="cell">
						{formatLedgerDate(item.acquiredAt)}
					</span>
					<div class="flex min-w-[5.25rem] shrink-0 items-center justify-end gap-1.5" role="cell">
						<ExpandingActionButton
							letter="E"
							label="Edit"
							onclick={() => oneditequipment(item)}
						/>
						<ExpandingActionButton
							letter="S"
							label="Sell"
							onclick={() => onsellequipment(item)}
						/>
						<ExpandingActionButton
							letter="X"
							label="Remove"
							onclick={() => ondeleteequipment(item)}
						/>
					</div>
				</li>
			{/each}
			{#if visibleEquipment.length === 0 && isSearchQuery(query)}
				<li class="px-2.5 py-3 text-center text-xs text-text-tertiary" role="row">
					<span role="cell">No asset matches that search.</span>
				</li>
			{:else if visibleEquipment.length === 0}
				<li class="px-2.5 py-10 text-center text-sm text-text-tertiary" role="row">
					<span role="cell">No assets recorded yet.</span>
				</li>
			{/if}
		</ul>
		</div>
	{/if}
</section>
