<script lang="ts">
	import { onMount } from 'svelte';
	import type {
		LedgerEntry,
		LedgerEntryType,
		LedgerPreset,
		InventoryItem,
		InventorySellResult
	} from '$lib/types/analytics';
	import {
		getLedgerEntries,
		addLedgerEntry,
		deleteLedgerEntry,
		getLedgerPresets,
		addLedgerPreset,
		deleteLedgerPreset,
		getInventoryItems,
		deleteInventoryItem
	} from '$lib/api';
	import { formatPed, formatLedgerDate } from '$lib/utils/format';
	import Card from '$lib/components/Card.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import Divider from '$lib/components/Divider.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import InventoryItemFormModal from './InventoryItemFormModal.svelte';
	import SellInventoryItemModal from './SellInventoryItemModal.svelte';
	import { registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';

	const netRanges = ['All Time', '30d', '90d', '1y'] as const;
	type NetRange = (typeof netRanges)[number];
	const netRangeDays: Record<NetRange, number | null> = {
		'All Time': null,
		'30d': 30,
		'90d': 90,
		'1y': 365
	};
	let netRange = $state<NetRange>('All Time');

	let entries = $state<LedgerEntry[]>([]);
	let presets = $state<LedgerPreset[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	// Form state
	let entryType = $state<LedgerEntryType>('expense');
	let entryAmount = $state(0);
	let entryDescription = $state('');
	let entryTag = $state('');
	let tagInputFocused = $state(false);

	// Preset form state
	let presetName = $state('');
	let presetType = $state<LedgerEntryType>('expense');
	let presetAmount = $state(0);
	let presetDescription = $state('');
	let presetTag = $state('');
	let presetTagInputFocused = $state(false);

	const tagLabels: Record<string, string> = {
		equipment: 'Equipment',
		repair: 'Repair',
		other: 'Other',
		item_sale: 'Auction Sales',
		quest_reward: 'Quest Reward',
		codex: 'Codex',
		inventory_sale: 'Mayhem'
	};

	function buildTagSuggestions(query: string, type: LedgerEntryType): string[] {
		const normalisedQuery = query.trim().toLowerCase();
		if (!normalisedQuery) return [];

		const tagCounts = new Map<string, number>();
		for (const entry of entries) {
			if (entry.type !== type) continue;
			const tag = entry.tag.trim();
			if (!tag) continue;
			const normalised = tag.toLowerCase();
			if (!normalised.includes(normalisedQuery) || normalised === normalisedQuery) continue;
			tagCounts.set(tag, (tagCounts.get(tag) ?? 0) + 1);
		}

		return Array.from(tagCounts.entries())
			.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
			.slice(0, 6)
			.map(([tag]) => tag);
	}

	let ledgerTagSuggestions = $derived(
		tagInputFocused ? buildTagSuggestions(entryTag, entryType) : []
	);

	let presetTagSuggestions = $derived(
		presetTagInputFocused ? buildTagSuggestions(presetTag, presetType) : []
	);

	$effect(() => {
		loadAll();
	});

	async function loadAll() {
		loading = true;
		error = null;
		try {
			const [entryRows, presetRows] = await Promise.all([
				getLedgerEntries(),
				getLedgerPresets()
			]);
			entries = entryRows;
			presets = presetRows;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load ledger';
		} finally {
			loading = false;
		}
	}

	async function addEntry() {
		const description = entryDescription.trim();
		const tag = entryTag.trim();
		if (!description || !tag || entryAmount <= 0) return;
		try {
			const newEntry = await addLedgerEntry({
				date: new Date().toISOString(),
				type: entryType,
				description,
				amount: entryAmount,
				tag
			});
			entries = [newEntry, ...entries];
			entryDescription = '';
			entryAmount = 0;
			entryTag = '';
			currentPage = 1;
			showAddModal = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to add entry';
		}
	}

	function applyTagSuggestion(tag: string) {
		entryTag = tag;
		tagInputFocused = false;
	}

	function applyPresetTagSuggestion(tag: string) {
		presetTag = tag;
		presetTagInputFocused = false;
	}

	async function deleteEntry(id: string) {
		try {
			await deleteLedgerEntry(id);
			entries = entries.filter((e) => e.id !== id);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete entry';
		}
	}

	async function savePreset() {
		const name = presetName.trim();
		const description = presetDescription.trim();
		const tag = presetTag.trim();
		if (!name || !description || !tag || presetAmount <= 0) return;
		try {
			const newPreset = await addLedgerPreset({
				name,
				type: presetType,
				description,
				amount: presetAmount,
				tag
			});
			presets = [...presets, newPreset];
			presetName = '';
			presetAmount = 0;
			presetDescription = '';
			presetTag = '';
			showPresetForm = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to save preset';
		}
	}

	async function removePreset(id: string) {
		try {
			await deleteLedgerPreset(id);
			presets = presets.filter((p) => p.id !== id);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete preset';
		}
	}

	async function applyPreset(preset: LedgerPreset) {
		try {
			const newEntry = await addLedgerEntry({
				date: new Date().toISOString(),
				type: preset.type,
				description: preset.description,
				amount: preset.amount,
				tag: preset.tag
			});
			entries = [newEntry, ...entries];
			currentPage = 1;
			showAddModal = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to add entry';
		}
	}

	// Pagination
	let currentPage = $state(1);
	const itemsPerPage = 5;

	let totalPages = $derived(Math.max(1, Math.ceil(entries.length / itemsPerPage)));
	let paginatedEntries = $derived(
		entries.slice((currentPage - 1) * itemsPerPage, currentPage * itemsPerPage)
	);

	$effect(() => {
		if (currentPage > totalPages) {
			currentPage = Math.max(1, totalPages);
		}
	});

	// Computed summaries (filtered by netRange — affects only the Net card)
	let netRangeEntries = $derived.by(() => {
		const days = netRangeDays[netRange];
		if (days === null) return entries;
		const cutoff = Date.now() - days * 24 * 60 * 60 * 1000;
		return entries.filter((e) => new Date(e.date).getTime() >= cutoff);
	});

	let expenseTags = $derived.by(() => {
		const tags: Record<string, number> = {};
		netRangeEntries
			.filter((e) => e.type === 'expense')
			.forEach((e) => {
				tags[e.tag] = (tags[e.tag] || 0) + e.amount;
			});
		return Object.entries(tags).map(([tag, total]) => ({ tag, total }));
	});

	let markupTags = $derived.by(() => {
		const tags: Record<string, number> = {};
		netRangeEntries
			.filter((e) => e.type === 'markup')
			.forEach((e) => {
				tags[e.tag] = (tags[e.tag] || 0) + e.amount;
			});
		return Object.entries(tags).map(([tag, total]) => ({ tag, total }));
	});

	let totalExpenses = $derived(
		netRangeEntries.filter((e) => e.type === 'expense').reduce((sum, e) => sum + e.amount, 0)
	);

	let totalMarkup = $derived(
		netRangeEntries.filter((e) => e.type === 'markup').reduce((sum, e) => sum + e.amount, 0)
	);

	let netLedger = $derived(totalMarkup - totalExpenses);

	let showAddModal = $state(false);
	let showPresets = $state(false);
	let showPresetForm = $state(false);
	let showLedgerSources = $state(false);

	// Inventory ledger state
	let inventoryItems = $state<InventoryItem[]>([]);
	let inventoryLoading = $state(true);
	let inventoryError = $state<string | null>(null);
	let showInventoryFormModal = $state(false);
	let inventoryEditTarget = $state<InventoryItem | null>(null);
	let inventorySellTarget = $state<InventoryItem | null>(null);
	let inventorySellPrefilledPrice = $state<number | null>(null);

	let inventoryTtTotal = $derived(inventoryItems.reduce((sum, i) => sum + i.ttValue, 0));
	let inventoryPaidTotal = $derived(
		inventoryItems.reduce((sum, i) => sum + i.ttValue + i.markupPaid, 0)
	);

	$effect(() => {
		void loadInventory();
	});

	// Guide-mode demoApi: lets the analytics surface drive the Add Entry modal
	// and the inventory Sell flow programmatically for the looped animations.
	// injectDemoSaleEntry / clearDemoSaleEntry mutate local entries state only
	// (no /demo/ writes; the demo router is read-only) so the synthetic row
	// vanishes the moment guide-mode flips off.
	onMount(() => {
		registerDemoApi('analytics-ledger', {
			openAddEntryModal: () => (showAddModal = true),
			closeAddEntryModal: () => (showAddModal = false),
			openInventorySellModal: (itemName: string, prefilledPrice?: number) => {
				const target = inventoryItems.find((i) => i.name === itemName);
				if (!target) return;
				inventorySellPrefilledPrice = prefilledPrice ?? null;
				inventorySellTarget = target;
			},
			closeInventorySellModal: () => {
				inventorySellTarget = null;
				inventorySellPrefilledPrice = null;
			},
			injectDemoSaleEntry: (itemName: string, gain: number) => {
				const syntheticEntry: LedgerEntry = {
					id: 'demo-inventory-sale',
					date: new Date().toISOString(),
					type: 'markup',
					description: `Sold ${itemName} at +${gain.toFixed(0)} PED over basis`,
					amount: gain,
					tag: 'inventory_sale'
				};
				entries = [syntheticEntry, ...entries.filter((e) => e.id !== syntheticEntry.id)];
				currentPage = 1;
			},
			clearDemoSaleEntry: () => {
				entries = entries.filter((e) => e.id !== 'demo-inventory-sale');
			}
		});
		return () => unregisterDemoApi('analytics-ledger');
	});

	async function loadInventory() {
		inventoryLoading = true;
		inventoryError = null;
		try {
			inventoryItems = await getInventoryItems();
		} catch (e) {
			inventoryError = e instanceof Error ? e.message : 'Failed to load inventory ledger';
		} finally {
			inventoryLoading = false;
		}
	}

	function openInventoryAdd() {
		inventoryEditTarget = null;
		showInventoryFormModal = true;
	}

	function openInventoryEdit(item: InventoryItem) {
		inventoryEditTarget = item;
		showInventoryFormModal = true;
	}

	function openInventorySell(item: InventoryItem) {
		inventorySellTarget = item;
	}

	function handleInventorySaved(saved: InventoryItem) {
		const idx = inventoryItems.findIndex((i) => i.id === saved.id);
		if (idx >= 0) {
			inventoryItems = inventoryItems.map((i) => (i.id === saved.id ? saved : i));
		} else {
			inventoryItems = [saved, ...inventoryItems];
		}
	}

	function handleInventorySold(result: InventorySellResult) {
		inventoryItems = inventoryItems.filter((i) => i.id !== result.soldItem.id);
		inventorySellTarget = null;
	}

	async function handleInventoryDelete(item: InventoryItem) {
		try {
			await deleteInventoryItem(item.id);
			inventoryItems = inventoryItems.filter((i) => i.id !== item.id);
		} catch (e) {
			inventoryError = e instanceof Error ? e.message : 'Failed to delete item';
		}
	}
</script>

{#if loading}
	<p class="text-sm text-text-secondary">Loading ledger...</p>
{:else if error}
	<p class="text-sm text-error">{error}</p>
{:else}
	<div class="space-y-6" data-guide-anchor="analytics-ledger-area">
		<!-- Strip + table grouped so guide-mode can cutout just the main ledger area
		     (excluding the inventory section below). Inner space-y-6 preserves
		     the prior vertical rhythm. -->
		<div class="space-y-6" data-guide-anchor="analytics-ledger-main-area">
		<!-- Net ledger impact -->
		<Card class="p-4">
			<div class="flex items-center justify-between gap-4 flex-wrap">
				<button
					type="button"
					class="flex items-center gap-3 group cursor-pointer"
					aria-expanded={showLedgerSources}
					onclick={() => (showLedgerSources = !showLedgerSources)}
				>
					<span class="eyebrow group-hover:text-text transition-colors">
						Net Ledger Impact
					</span>
					<span
						class="text-sm font-semibold tabular-nums {netLedger >= 0
							? 'text-positive'
							: 'text-negative'}"
					>
						{netLedger >= 0 ? '+' : ''}{formatPed(netLedger)} PED
					</span>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 20 20"
						fill="currentColor"
						class="h-4 w-4 text-text-tertiary transition-transform duration-[var(--duration-base)] {showLedgerSources ? 'rotate-180' : ''}"
					>
						<path
							fill-rule="evenodd"
							d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
							clip-rule="evenodd"
						/>
					</svg>
				</button>

				<div class="flex items-center gap-2 flex-shrink-0">
					<SegmentedControl
						options={netRanges.map((r) => ({ id: r, label: r }))}
						active={netRange}
						onchange={(id) => (netRange = id as NetRange)}
					/>
					<span data-guide-anchor="ledger-add-entry-btn" class="inline-flex">
						<Button size="sm" onclick={() => (showAddModal = true)}>Add Entry</Button>
					</span>
				</div>
			</div>

			{#if showLedgerSources}
				<div class="mt-4 pt-4 border-t border-border/50 grid grid-cols-1 md:grid-cols-2 gap-6">
					<div>
						<h3 class="eyebrow mb-3">
							Expense Sources
						</h3>
						{#if expenseTags.length === 0}
							<p class="text-xs text-text-tertiary">No expenses recorded</p>
						{:else}
							<div class="space-y-2">
								{#each expenseTags as { tag, total }}
									<div class="flex items-center justify-between text-sm">
										<span class="text-text-secondary">{tagLabels[tag] || tag}</span>
										<span class="text-negative tabular-nums font-medium">
											{formatPed(total)} PED
										</span>
									</div>
								{/each}
								<Divider class="my-1" />
								<div class="flex items-center justify-between text-sm font-medium">
									<span class="text-text">Total Expenses</span>
									<span class="text-negative tabular-nums">{formatPed(totalExpenses)} PED</span>
								</div>
							</div>
						{/if}
					</div>
					<div>
						<h3 class="eyebrow mb-3">
							Markup Sources
						</h3>
						{#if markupTags.length === 0}
							<p class="text-xs text-text-tertiary">No markup recorded</p>
						{:else}
							<div class="space-y-2">
								{#each markupTags as { tag, total }}
									<div class="flex items-center justify-between text-sm">
										<span class="text-text-secondary">{tagLabels[tag] || tag}</span>
										<span class="text-positive tabular-nums font-medium">
											{formatPed(total)} PED
										</span>
									</div>
								{/each}
								<Divider class="my-1" />
								<div class="flex items-center justify-between text-sm font-medium">
									<span class="text-text">Total Markup</span>
									<span class="text-positive tabular-nums">{formatPed(totalMarkup)} PED</span>
								</div>
							</div>
						{/if}
					</div>
				</div>
			{/if}
		</Card>

		<!-- Entry table -->
		<div>
			{#if entries.length === 0}
				<Card class="p-8">
					<p class="text-center text-text-tertiary text-sm">
						Record confirmed sales, equipment purchases, quest rewards, and other economic flows not
						captured by automatic tracking. Only confirmed values, no estimates.
					</p>
				</Card>
			{:else}
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border">
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-left">Date</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-left">
								Description
							</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Amount</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-left">Tag</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right w-10"></th>
						</tr>
					</thead>
					<tbody>
						{#each paginatedEntries as entry}
							<tr
								data-guide-anchor="ledger-entry-row"
								data-entry-id={entry.id}
								class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors duration-[var(--duration-fast)]"
							>
								<td class="py-2.5 px-3 text-text-secondary tabular-nums">
									{formatLedgerDate(entry.date)}
								</td>
								<td class="py-2.5 px-3 text-text">{entry.description}</td>
								<td
									class="py-2.5 px-3 text-right tabular-nums font-medium {entry.type === 'markup'
										? 'text-positive'
										: 'text-negative'}"
								>
									{entry.type === 'markup' ? '+' : '-'}{formatPed(entry.amount)}
								</td>
								<td class="py-2.5 px-3">
									<Badge variant={entry.type === 'markup' ? 'positive' : 'negative'}>
										{tagLabels[entry.tag] || entry.tag}
									</Badge>
								</td>
								<td class="py-2.5 px-3 text-right">
									<button
										type="button"
										class="icon-button-row"
										onclick={() => deleteEntry(entry.id)}
										aria-label="Delete entry"
									>
										<svg
											xmlns="http://www.w3.org/2000/svg"
											viewBox="0 0 20 20"
											fill="currentColor"
											class="h-4 w-4"
										>
											<path
												d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
											/>
										</svg>
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>

				{#if totalPages > 1}
					<div class="flex items-center justify-between mt-4">
						<span class="text-xs text-text-tertiary">
							Showing {(currentPage - 1) * itemsPerPage + 1} to {Math.min(currentPage * itemsPerPage, entries.length)} of {entries.length} entries
						</span>
						<div class="flex gap-1">
							<Button
								size="sm"
								variant="ghost"
								disabled={currentPage === 1}
								onclick={() => currentPage--}
							>
								Previous
							</Button>
							<Button
								size="sm"
								variant="ghost"
								disabled={currentPage === totalPages}
								onclick={() => currentPage++}
							>
								Next
							</Button>
						</div>
					</div>
				{/if}
			{/if}
		</div>
		</div>

		<Divider />

		<!-- Inventory Ledger -->
		<div data-guide-anchor="analytics-ledger-inventory-area">
			{#if inventoryLoading}
				<p class="text-sm text-text-secondary">Loading inventory ledger...</p>
			{:else if inventoryError}
				<p class="text-sm text-error">{inventoryError}</p>
			{:else}
				<Card class="p-4 mb-3">
					<div class="flex items-center justify-between gap-4 flex-wrap">
						<div class="flex items-center gap-6 flex-wrap">
							<div class="flex items-center gap-3">
								<span class="eyebrow">
									Inventory TT Value
								</span>
								<span class="text-sm font-semibold tabular-nums text-text">
									{formatPed(inventoryTtTotal)} PED
								</span>
							</div>
							<div class="flex items-center gap-3">
								<span class="eyebrow">
									Value After Paid Markup
								</span>
								<span class="text-sm font-semibold tabular-nums text-text">
									{formatPed(inventoryPaidTotal)} PED
								</span>
							</div>
						</div>
						<Button size="sm" onclick={openInventoryAdd}>Add Item</Button>
					</div>
				</Card>

				{#if inventoryItems.length === 0}
					<Card class="p-6">
						<p class="text-center text-text-tertiary text-sm">
							Log unlimited weapons, estates, deeds, or other persistent items you own.
							Their cost basis is held here; only the realised gain or loss on sale
							flows into the Ledger.
						</p>
					</Card>
				{:else}
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-border">
								<th class="py-2 px-3 text-xs font-medium text-text-secondary text-left">Name</th>
								<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">TT</th>
								<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Markup</th>
								<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Cost Basis</th>
								<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Actions</th>
							</tr>
						</thead>
						<tbody>
							{#each inventoryItems as item (item.id)}
								<tr
									class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors duration-[var(--duration-fast)]"
								>
									<td class="py-2.5 px-3">
										<div class="text-text">{item.name}</div>
										{#if item.notes}
											<div class="text-xs text-text-tertiary truncate mt-0.5">
												{item.notes}
											</div>
										{/if}
									</td>
									<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
										{formatPed(item.ttValue)}
									</td>
									<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
										{formatPed(item.markupPaid)}
									</td>
									<td class="py-2.5 px-3 text-right tabular-nums font-medium text-text">
										{formatPed(item.ttValue + item.markupPaid)}
									</td>
									<td class="py-2.5 px-3">
										<div class="flex items-center justify-end gap-1.5">
											<Button size="sm" variant="ghost" onclick={() => openInventoryEdit(item)}>
												Edit
											</Button>
											<span
												data-guide-anchor="inventory-sell-btn"
												data-item-name={item.name}
												class="inline-flex"
											>
												<Button size="sm" onclick={() => openInventorySell(item)}>Sell</Button>
											</span>
											<button
												type="button"
												class="icon-button-row"
												onclick={() => handleInventoryDelete(item)}
												aria-label={`Delete ${item.name}`}
												title="Delete (no ledger entry)"
											>
												<svg
													xmlns="http://www.w3.org/2000/svg"
													viewBox="0 0 20 20"
													fill="currentColor"
													class="h-4 w-4"
												>
													<path
														d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
													/>
												</svg>
											</button>
										</div>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			{/if}
		</div>

	</div>
{/if}

<!-- Inventory item modals -->
<InventoryItemFormModal
	bind:open={showInventoryFormModal}
	item={inventoryEditTarget}
	onsaved={handleInventorySaved}
/>
<SellInventoryItemModal
	item={inventorySellTarget}
	prefilledSalePrice={inventorySellPrefilledPrice}
	onsold={handleInventorySold}
	oncancel={() => {
		inventorySellTarget = null;
		inventorySellPrefilledPrice = null;
	}}
/>

<!-- Add Entry Modal -->
<Modal bind:open={showAddModal} title="Add Entry" class="max-w-lg">
	<div class="space-y-5">
		<!-- Type toggle -->
		<div>
			<span class="eyebrow mb-1.5 block">Type</span>
			<SegmentedControl
				size="md"
				options={[
					{ id: 'expense', label: 'Expense' },
					{ id: 'markup', label: 'Markup' }
				]}
				active={entryType}
				onchange={(id) => (entryType = id as LedgerEntryType)}
			/>
		</div>

		<!-- Amount -->
		<div>
			<label class="block eyebrow mb-1.5" for="ledger-amount">
				Amount (PED)
			</label>
			<Input
				id="ledger-amount"
				type="number"
				bind:value={entryAmount}
				placeholder="0.00"
				step="0.01"
				min="0"
			/>
		</div>

		<!-- Description -->
		<div>
			<label class="block eyebrow mb-1.5" for="ledger-desc">
				Description
			</label>
			<Input
				id="ledger-desc"
				type="text"
				bind:value={entryDescription}
				placeholder="What was this for?"
			/>
		</div>

		<!-- Tag -->
		<div class="relative">
			<label class="block eyebrow mb-1.5" for="ledger-tag">
				Tag
			</label>
			<Input
				id="ledger-tag"
				bind:value={entryTag}
				type="text"
				placeholder={entryType === 'expense' ? 'equipment' : 'item_sale'}
				onfocus={() => (tagInputFocused = true)}
				onblur={() => {
					setTimeout(() => {
						tagInputFocused = false;
					}, 100);
				}}
			/>
			{#if ledgerTagSuggestions.length > 0}
				<div
					class="absolute top-full left-0 right-0 z-10 mt-1 overflow-hidden rounded-md border border-border bg-surface-raised shadow-lg"
				>
					{#each ledgerTagSuggestions as suggestion}
						<button
							type="button"
							class="block w-full px-3 py-2 text-left text-sm text-text-secondary transition-colors hover:bg-surface-hover hover:text-text cursor-pointer"
							onmousedown={(event) => event.preventDefault()}
							onclick={() => applyTagSuggestion(suggestion)}
						>
							{suggestion}
						</button>
					{/each}
				</div>
			{/if}
		</div>

		<!-- Quick Entries sub-section -->
		<div class="pt-4 border-t border-border/50">
			<h3 class="eyebrow">
				Quick Entries
			</h3>

			<div class="mt-3 flex flex-wrap items-center gap-2">
					{#each presets as preset (preset.id)}
						{@const isMarkup = preset.type === 'markup'}
						<span
							class="group/badge inline-flex items-center gap-1.5 rounded-sm pl-2 pr-1 py-0.5 text-xs font-medium {isMarkup
								? 'bg-positive-muted/40 text-positive'
								: 'bg-negative-muted/40 text-negative'}"
						>
							<button
								type="button"
								class="inline-flex items-center gap-1.5 cursor-pointer"
								title="Add entry from this preset"
								onclick={() => applyPreset(preset)}
							>
								<span>{preset.name}</span>
								<span class="tabular-nums opacity-80">
									{isMarkup ? '+' : '-'}{formatPed(preset.amount)}
								</span>
							</button>
							<button
								type="button"
								class="rounded-sm p-0.5 opacity-0 group-hover/badge:opacity-60 hover:!opacity-100 hover:bg-surface-hover/50 transition-opacity cursor-pointer"
								aria-label="Delete preset"
								title="Delete preset"
								onclick={(e) => {
									e.stopPropagation();
									removePreset(preset.id);
								}}
							>
								<svg
									xmlns="http://www.w3.org/2000/svg"
									viewBox="0 0 20 20"
									fill="currentColor"
									class="h-3 w-3"
								>
									<path
										d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
									/>
								</svg>
							</button>
						</span>
					{/each}

					<div class="ml-auto">
						<Button
							size="sm"
							variant={showPresetForm ? 'secondary' : 'ghost'}
							onclick={() => (showPresetForm = !showPresetForm)}
						>
							{showPresetForm ? 'Cancel' : 'New Quick Entry'}
						</Button>
					</div>
				</div>

				{#if showPresetForm}
					<div class="mt-3 pt-3 border-t border-border/30 space-y-3">
						<div>
							<label class="block eyebrow mb-1.5" for="preset-name">
								Name
							</label>
							<Input
								id="preset-name"
								type="text"
								bind:value={presetName}
								placeholder="e.g. L weapon"
							/>
						</div>

						<div>
							<span class="eyebrow mb-1.5 block">Type</span>
							<SegmentedControl
								size="md"
								options={[
									{ id: 'expense', label: 'Expense' },
									{ id: 'markup', label: 'Markup' }
								]}
								active={presetType}
								onchange={(id) => (presetType = id as LedgerEntryType)}
							/>
						</div>

						<div>
							<label class="block eyebrow mb-1.5" for="preset-amount">
								Amount (PED)
							</label>
							<Input
								id="preset-amount"
								type="number"
								bind:value={presetAmount}
								placeholder="0.00"
								step="0.01"
								min="0"
							/>
						</div>

						<div>
							<label class="block eyebrow mb-1.5" for="preset-desc">
								Description
							</label>
							<Input
								id="preset-desc"
								type="text"
								bind:value={presetDescription}
								placeholder="What was this for?"
							/>
						</div>

						<div class="relative">
							<label class="block eyebrow mb-1.5" for="preset-tag">
								Tag
							</label>
							<Input
								id="preset-tag"
								bind:value={presetTag}
								type="text"
								placeholder={presetType === 'expense' ? 'equipment' : 'item_sale'}
								onfocus={() => (presetTagInputFocused = true)}
								onblur={() => {
									setTimeout(() => {
										presetTagInputFocused = false;
									}, 100);
								}}
							/>
							{#if presetTagSuggestions.length > 0}
								<div
									class="absolute top-full left-0 right-0 z-10 mt-1 overflow-hidden rounded-md border border-border bg-surface-raised shadow-lg"
								>
									{#each presetTagSuggestions as suggestion}
										<button
											type="button"
											class="block w-full px-3 py-2 text-left text-sm text-text-secondary transition-colors hover:bg-surface-hover hover:text-text cursor-pointer"
											onmousedown={(event) => event.preventDefault()}
											onclick={() => applyPresetTagSuggestion(suggestion)}
										>
											{suggestion}
										</button>
									{/each}
								</div>
							{/if}
						</div>

						<div class="flex justify-end">
							<Button size="sm" onclick={savePreset}>Save Preset</Button>
						</div>
					</div>
				{/if}
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="ghost" onclick={() => (showAddModal = false)}>Cancel</Button>
			<Button onclick={addEntry}>Add Entry</Button>
		</div>
	</div>
</Modal>
