<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import type { OverviewModel } from './overviewModel.svelte';
	import { formatAge, formatSalesPed, HORIZONS } from './overviewModel.svelte';

	let { model }: { model: OverviewModel } = $props();

	// Observation ages are week-scale; the load time is a fine "now".
	const nowEpoch = Date.now() / 1000;

	function formatMarkup(markupPct: number | null): string {
		return markupPct === null ? NO_DATA : `${markupPct.toFixed(2)}%`;
	}

	function formatDay(epoch: number): string {
		return new Date(epoch * 1000).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
		});
	}

	function horizonLabel(id: string): string {
		return HORIZONS.find((horizon) => horizon.id === id)?.label ?? id;
	}

	const columns = 'grid grid-cols-[minmax(0,1fr)_6.5rem_7rem_5rem] items-baseline gap-3';
</script>

<!-- Fills the space its parent leaves and scrolls the item rows inside
     it; the search and column headings stay put. The minimum height keeps
     the list usable when the paste area above grows tall. -->
<section class="flex min-h-[14rem] flex-1 flex-col gap-3" aria-label="Market observations">
	{#if model.error}
		<ErrorNotice message={model.error} />
	{:else if !model.loading && model.items.length === 0}
		<p class="py-8 text-center text-sm text-text-tertiary">
			No market observations yet. Paste a market-ledger export above.
		</p>
	{:else}
		<div class="flex shrink-0 items-center justify-between gap-3">
			<SearchInput bind:value={model.search} placeholder="Find an item" class="max-w-56" />
			<span class="text-xs text-text-tertiary tabular-nums">
				{model.items.length}
				{model.items.length === 1 ? 'item' : 'items'}
			</span>
		</div>

		<div class="{columns} shrink-0 border-b border-border/60 px-2.5 pb-2 pr-3.5 text-text-tertiary">
			<span class="eyebrow">Item</span>
			<span class="eyebrow text-right">Markup</span>
			<span class="eyebrow text-right">Volume</span>
			<span class="eyebrow text-right">Observed</span>
		</div>

		{#if model.visibleItems.length === 0}
			<p class="py-6 text-center text-sm text-text-tertiary">
				{model.loading ? 'Loading market data' : 'No items match the search'}
			</p>
		{/if}

		<ul class="flex min-h-0 flex-1 flex-col overflow-y-auto pr-1">
			{#each model.visibleItems as item (item.row.itemName)}
				{@const expanded = model.expandedItem === item.row.itemName}
				{@const detailId = `market-item-${item.row.itemName.replace(/\W+/g, '-')}`}
				<li class="border-b border-border/30 last:border-b-0">
					<button
						type="button"
						class="{columns} w-full rounded-md px-2.5 py-2 text-left text-sm transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover/30 focus:outline-none focus-visible:[box-shadow:var(--shadow-glow)]"
						aria-expanded={expanded}
						aria-controls={detailId}
						onclick={() => void model.toggle(item.row.itemName)}
					>
						<span class="min-w-0 truncate text-text">
							{item.row.itemName}
							{#if item.row.tier > 0}
								<span class="ml-1.5 text-xs text-text-tertiary">T{item.row.tier}</span>
							{/if}
						</span>
						<span class="text-right tabular-nums text-text">
							{formatMarkup(item.resolved.markupPct)}
							{#if item.resolved.markupPct !== null && item.resolved.horizon !== 'week'}
								<span class="block text-[11px] leading-tight text-text-tertiary">
									{horizonLabel(item.resolved.horizon).toLowerCase()}
								</span>
							{/if}
						</span>
						<span class="text-right tabular-nums text-text-secondary">
							{item.resolved.salesPed > 0 ? formatSalesPed(item.resolved.salesPed) : NO_DATA}
						</span>
						<span class="text-right text-text-tertiary">
							{formatAge(item.row.observedAt, nowEpoch)}
						</span>
					</button>

					{#if expanded}
						<div id={detailId} class="space-y-4 px-2.5 pb-4 pt-1">
							<div
								class="grid grid-cols-[auto_repeat(5,minmax(3.5rem,1fr))] items-baseline gap-x-3 gap-y-1.5"
							>
								<span></span>
								{#each HORIZONS as horizon (horizon.id)}
									<span class="eyebrow text-right">{horizon.label}</span>
								{/each}
								<span class="eyebrow">MU</span>
								{#each HORIZONS as horizon (horizon.id)}
									<span class="text-right text-sm tabular-nums text-text">
										{formatMarkup(item.row[horizon.id].markupPct)}
									</span>
								{/each}
								<span class="eyebrow">Volume</span>
								{#each HORIZONS as horizon (horizon.id)}
									<span class="text-right text-sm tabular-nums text-text-secondary">
										{item.row[horizon.id].salesPed > 0
											? formatSalesPed(item.row[horizon.id].salesPed)
											: NO_DATA}
									</span>
								{/each}
							</div>

							<div>
								<p class="eyebrow mb-1.5 text-text-tertiary">
									{horizonLabel(item.resolved.horizon)} history
								</p>
								{#if model.historyError}
									<ErrorNotice message={model.historyError} />
								{:else if model.historyLoading}
									<p class="text-xs text-text-tertiary">Loading history</p>
								{:else}
									<ul class="space-y-1">
										{#each model.history as point (point.observedAt)}
											<li class="grid grid-cols-[8rem_6.5rem_7rem] gap-3 text-xs tabular-nums">
												<span class="text-text-secondary">{formatDay(point.observedAt)}</span>
												<span class="text-right text-text">{formatMarkup(point.markupPct)}</span>
												<span class="text-right text-text-tertiary">
													{point.salesPed > 0 ? formatSalesPed(point.salesPed) : NO_DATA}
												</span>
											</li>
										{/each}
									</ul>
								{/if}
							</div>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>
