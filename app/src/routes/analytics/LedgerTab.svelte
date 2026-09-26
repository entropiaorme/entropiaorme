<script lang="ts">
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import Divider from '$lib/components/Divider.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import {
		createLedgerModel,
		type NetRange,
		netRanges,
		PAGE_SIZE,
		tagLabels
	} from '$lib/features/analytics/ledgerModel.svelte';
	import type { LedgerEntryType } from '$lib/types/analytics';
	import { formatLedgerDate, formatPed } from '$lib/utils/format';

	const model = createLedgerModel();
	const table = model.table;

	$effect(() => {
		void model.loadAll();
	});
</script>

{#if model.loading}
	<p class="text-sm text-text-secondary">Loading ledger...</p>
{:else}
	<div class="space-y-6" data-testid="analytics-ledger-area">
		<ErrorNotice message={model.error} />
		<!-- Strip + table grouped. -->
		<div class="space-y-6">
		<!-- Net ledger impact -->
		<Card class="p-4">
			<div class="flex items-center justify-between gap-4 flex-wrap">
				<button
					type="button"
					class="flex items-center gap-3 group cursor-pointer"
					aria-expanded={model.showLedgerSources}
					onclick={() => (model.showLedgerSources = !model.showLedgerSources)}
				>
					<span class="eyebrow group-hover:text-text transition-colors">
						Net Ledger Impact
					</span>
					<span
						class="text-sm font-semibold tabular-nums {model.netLedger >= 0
							? 'text-positive'
							: 'text-negative'}"
					>
						{model.netLedger >= 0 ? '+' : ''}{formatPed(model.netLedger)} PED
					</span>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 20 20"
						fill="currentColor"
						class="h-4 w-4 text-text-tertiary transition-transform duration-[var(--duration-base)] {model.showLedgerSources ? 'rotate-180' : ''}"
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
						active={model.netRange}
						onchange={(id) => (model.netRange = id as NetRange)}
					/>
					<span class="inline-flex">
						<Button size="sm" onclick={() => (model.showAddModal = true)}>Add Entry</Button>
					</span>
				</div>
			</div>

			{#if model.showLedgerSources}
				<div class="mt-4 pt-4 border-t border-border/50 grid grid-cols-1 md:grid-cols-2 gap-6">
					<div>
						<h3 class="eyebrow mb-3">
							Expense Sources
						</h3>
						{#if model.expenseTags.length === 0}
							<p class="text-xs text-text-tertiary">No expenses recorded</p>
						{:else}
							<div class="space-y-2">
								{#each model.expenseTags as { tag, total }}
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
									<span class="text-negative tabular-nums">{formatPed(model.totalExpenses)} PED</span>
								</div>
							</div>
						{/if}
					</div>
					<div>
						<h3 class="eyebrow mb-3">
							Markup Sources
						</h3>
						{#if model.markupTags.length === 0}
							<p class="text-xs text-text-tertiary">No markup recorded</p>
						{:else}
							<div class="space-y-2">
								{#each model.markupTags as { tag, total }}
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
									<span class="text-positive tabular-nums">{formatPed(model.totalMarkup)} PED</span>
								</div>
							</div>
						{/if}
					</div>
				</div>
			{/if}
		</Card>

		<!-- Entry table -->
		<div>
			{#if model.entries.length === 0}
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
						{#each table.pageRows as entry}
							<tr
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
										onclick={() => model.deleteEntry(entry.id)}
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

				{#if model.totalPages > 1}
					<div class="flex items-center justify-between mt-4">
						<span class="text-xs text-text-tertiary">
							Showing {table.page * PAGE_SIZE + 1} to {Math.min((table.page + 1) * PAGE_SIZE, model.total)} of {model.total} entries
						</span>
						<div class="flex gap-1">
							<Button
								size="sm"
								variant="ghost"
								disabled={table.page === 0}
								onclick={() => model.prevPage()}
							>
								Previous
							</Button>
							<Button
								size="sm"
								variant="ghost"
								disabled={table.page >= model.totalPages - 1 || model.loadingMore}
								onclick={() => model.nextPage()}
							>
								Next
							</Button>
						</div>
					</div>
				{/if}
			{/if}
		</div>
		</div>
	</div>
{/if}

<!-- Add Entry Modal -->
<Modal bind:open={model.showAddModal} title="Add Entry" class="max-w-lg">
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
				active={model.entryType}
				onchange={(id) => (model.entryType = id as LedgerEntryType)}
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
				bind:value={model.entryAmount}
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
				bind:value={model.entryDescription}
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
				bind:value={model.entryTag}
				type="text"
				placeholder={model.entryType === 'expense' ? 'equipment' : 'item_sale'}
				onfocus={() => (model.tagInputFocused = true)}
				onblur={() => {
					setTimeout(() => {
						model.tagInputFocused = false;
					}, 100);
				}}
			/>
			{#if model.ledgerTagSuggestions.length > 0}
				<div
					class="absolute top-full left-0 right-0 z-10 mt-1 overflow-hidden rounded-md border border-border bg-surface-raised shadow-lg"
				>
					{#each model.ledgerTagSuggestions as suggestion}
						<button
							type="button"
							class="block w-full px-3 py-2 text-left text-sm text-text-secondary transition-colors hover:bg-surface-hover hover:text-text cursor-pointer"
							onmousedown={(event) => event.preventDefault()}
							onclick={() => model.applyTagSuggestion(suggestion)}
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
					{#each model.presets as preset (preset.id)}
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
								onclick={() => model.applyPreset(preset)}
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
									model.removePreset(preset.id);
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
							variant={model.showPresetForm ? 'secondary' : 'ghost'}
							onclick={() => (model.showPresetForm = !model.showPresetForm)}
						>
							{model.showPresetForm ? 'Cancel' : 'New Quick Entry'}
						</Button>
					</div>
				</div>

				{#if model.showPresetForm}
					<div class="mt-3 pt-3 border-t border-border/30 space-y-3">
						<div>
							<label class="block eyebrow mb-1.5" for="preset-name">
								Name
							</label>
							<Input
								id="preset-name"
								type="text"
								bind:value={model.presetName}
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
								active={model.presetType}
								onchange={(id) => (model.presetType = id as LedgerEntryType)}
							/>
						</div>

						<div>
							<label class="block eyebrow mb-1.5" for="preset-amount">
								Amount (PED)
							</label>
							<Input
								id="preset-amount"
								type="number"
								bind:value={model.presetAmount}
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
								bind:value={model.presetDescription}
								placeholder="What was this for?"
							/>
						</div>

						<div class="relative">
							<label class="block eyebrow mb-1.5" for="preset-tag">
								Tag
							</label>
							<Input
								id="preset-tag"
								bind:value={model.presetTag}
								type="text"
								placeholder={model.presetType === 'expense' ? 'equipment' : 'item_sale'}
								onfocus={() => (model.presetTagInputFocused = true)}
								onblur={() => {
									setTimeout(() => {
										model.presetTagInputFocused = false;
									}, 100);
								}}
							/>
							{#if model.presetTagSuggestions.length > 0}
								<div
									class="absolute top-full left-0 right-0 z-10 mt-1 overflow-hidden rounded-md border border-border bg-surface-raised shadow-lg"
								>
									{#each model.presetTagSuggestions as suggestion}
										<button
											type="button"
											class="block w-full px-3 py-2 text-left text-sm text-text-secondary transition-colors hover:bg-surface-hover hover:text-text cursor-pointer"
											onmousedown={(event) => event.preventDefault()}
											onclick={() => model.applyPresetTagSuggestion(suggestion)}
										>
											{suggestion}
										</button>
									{/each}
								</div>
							{/if}
						</div>

						<div class="flex justify-end">
							<Button size="sm" onclick={() => model.savePreset()}>Save Preset</Button>
						</div>
					</div>
				{/if}
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="ghost" onclick={() => (model.showAddModal = false)}>Cancel</Button>
			<Button onclick={() => model.addEntry()}>Add Entry</Button>
		</div>
	</div>
</Modal>
