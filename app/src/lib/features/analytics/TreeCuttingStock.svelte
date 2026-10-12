<script lang="ts">
	import type { Snippet } from 'svelte';
	import { filterByQuery } from '$lib/view/search';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import ExpandingActionButton from '$lib/components/ExpandingActionButton.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { tick } from 'svelte';
	import type { TreeCuttingStock } from './treeCuttingModel.svelte';
	import { shrapnelConversionTip, shrapnelMarkupLabel } from './marketConfidence';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';

	let {
		stock,
		onsell,
		onconvert,
		onremove,
		onshrapnelconvert,
		heading,
		controlsLabel,
		actions,
		controls,
		actionLayout = 'activity',
		emptyMessage = 'No tracked stock is currently held.',
		alwaysSearch = false,
		fillAvailable = false,
		// The one activity-specific line in the panel, so the Hunting tab can
		// host the same surface over its own loot without forking the layout.
		sourceDescription = 'Loot recorded from tree cutting, minus stock you have sold, converted, or removed.',
	}: {
		stock: TreeCuttingStock[];
		onsell: (item: TreeCuttingStock) => void;
		onconvert: (item: TreeCuttingStock) => void;
		onremove: (item: TreeCuttingStock) => void;
		onshrapnelconvert: (item: TreeCuttingStock) => void;
		heading?: Snippet;
		controlsLabel?: Snippet;
		/** Page-level actions for the strip's right-hand slot. */
		actions?: Snippet;
		controls?: Snippet;
		actionLayout?: 'activity' | 'inventory';
		emptyMessage?: string;
		alwaysSearch?: boolean;
		fillAvailable?: boolean;
		sourceDescription?: string;
	} = $props();

	// A hunting loot table runs to hundreds of distinct items where a
	// harvesting one holds a handful, so the panel scales itself: past the
	// threshold a search appears and emptied lines fold behind a quiet
	// disclosure. Below it, nothing changes: every line stays visible,
	// emptied ones dimmed, exactly as this panel has always read.
	const SEARCH_THRESHOLD = 8;
	let query = $state('');
	let showEmptied = $state(false);
	const longList = $derived(stock.length > SEARCH_THRESHOLD);
	const searchable = $derived(alwaysSearch || longList);
	const matches = $derived(filterByQuery(stock, query, (item) => item.itemName));
	// A live query overrides the emptied fold: a deliberate search is a
	// deliberate request for that item, sold out or not. The fold's count
	// reads against the whole list so the two can never disagree.
	const emptiedCount = $derived(stock.filter((item) => item.heldQty <= 0).length);
	const visibleStock = $derived(
		longList && !showEmptied && query.trim() === ''
			? matches.filter((item) => item.heldQty > 0)
			: matches,
	);
	let stockList = $state<HTMLUListElement>();
	let hasMoreBelow = $state(false);

	function updateScrollContinuation() {
		if (!stockList) {
			hasMoreBelow = false;
			return;
		}
		hasMoreBelow = stockList.scrollHeight - stockList.clientHeight - stockList.scrollTop > 2;
	}

	$effect(() => {
		void visibleStock;
		const list = stockList;
		if (!list) return;
		void tick().then(updateScrollContinuation);
		const observer =
			typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(updateScrollContinuation);
		observer?.observe(list);
		window.addEventListener('resize', updateScrollContinuation);
		return () => {
			observer?.disconnect();
			window.removeEventListener('resize', updateScrollContinuation);
		};
	});

	function formatVolume(value: number): string {
		if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
		if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
		return formatPed(value);
	}
	const marketPeriod = (horizon: string) =>
		horizon === 'week' || horizon === 'month' || horizon === 'year'
			? `last ${horizon}`
			: `over the last ${horizon}`;
	const marketShare = (value: number) => {
		const percent = value * 100;
		return percent < 0.1 ? 'less than 0.1%' : `${percent.toFixed(1)}%`;
	};
	const confidenceTitle = (tier: TreeCuttingStock['tier']) => {
		if (tier === 'liquid') return 'High markup confidence: This markup should be practical to realise';
		if (tier === 'middling') {
			return 'Medium markup confidence: It may be difficult to realise this markup';
		}
		return 'Low markup confidence: Do not rely on realising this markup';
	};
	const markupLabel = (item: TreeCuttingStock) => {
		if (item.effectiveMarkupPct == null) return 'No markup available';
		if (item.markupBasis === 'shrapnel_conversion') {
			return shrapnelMarkupLabel(item.markupPct, item.effectiveMarkupPct);
		}
		if (item.floored && item.markupPct !== null) {
			return `Observed markup ${formatPercent(item.markupPct / 100)}; projections use ${formatPercent(item.effectiveMarkupPct / 100)} Nanocube markup`;
		}
		if (item.markupPct == null) {
			return `Projections use ${formatPercent(item.effectiveMarkupPct / 100)} Nanocube markup`;
		}
		return `Markup ${formatPercent(item.effectiveMarkupPct / 100)}`;
	};

	function confidenceTip(item: TreeCuttingStock): {
		title: string;
		subtitle: string;
		example?: string;
		note?: string;
	} {
		if (item.markupBasis === 'shrapnel_conversion' && item.effectiveMarkupPct !== null) {
			return shrapnelConversionTip(item.markupPct, item.effectiveMarkupPct);
		}
		if (item.markupPct == null || !item.opportunity) {
			return {
				title: confidenceTitle(item.tier),
				subtitle: 'No market MU is available for this item.',
			};
		}

		const horizon = item.markupHorizon;
		const salesPed = item.salesPed;
		let lead = 'No recent sales data is available for this item.';
		if (horizon && salesPed !== null) {
			if (horizon === 'week') {
				lead = `${formatPed(salesPed)} PED TT sold last week at ${formatPercent(item.markupPct / 100)} MU.`;
			} else {
			const weeklyReading =
				item.weeklySalesPed == null || item.weeklySalesPed <= 0
					? 'No sales in the last week.'
					: `${formatPed(item.weeklySalesPed)} PED TT sold last week.`;
			lead = `${weeklyReading} The current ${formatPercent(item.markupPct / 100)} MU comes from ${formatPed(salesPed)} PED TT sold ${marketPeriod(horizon)}.`;
			}
		}

		const batchTt = item.opportunity.efficientBatchTt;
		const batchShare = item.opportunity.efficientBatchMarketShare;
		const batchMarkup =
			batchTt === null ? null : batchTt * Math.max(0, item.markupPct / 100 - 1);
		const example =
			batchTt !== null && batchMarkup !== null && batchShare !== null && horizon
				? `For example: A ${formatPed(batchTt)} PED TT sale at this MU would produce about ${formatPed(batchMarkup)} PED of markup. The minimum auction fee is 0.5 PED, or 10% of that markup. That sale would be ${marketShare(batchShare)} of the TT sold ${marketPeriod(horizon)}.`
				: undefined;
		return {
			title: confidenceTitle(item.tier),
			subtitle: lead,
			example,
			note: example
				? undefined
				: 'The recorded MU does not provide enough markup to calculate a sale after fees.',
		};
	}
</script>

{#snippet confidenceBody(item: TreeCuttingStock)}
	{@const tip = confidenceTip(item)}
	<p class="text-xs font-semibold leading-relaxed text-text">{tip.title}</p>
	<p class="mt-1 text-xs leading-relaxed text-text-secondary">{tip.subtitle}</p>
	{#if tip.example}
		<p class="mt-2 text-xs leading-relaxed text-text-secondary">{tip.example}</p>
	{/if}
	{#if tip.note}
		<p class="mt-2 text-xs leading-relaxed text-text-tertiary">{tip.note}</p>
	{/if}
{/snippet}

{#snippet markupBreakdown(
	readings: { horizon: string; markupPct: number | null; salesPed: number }[],
)}
	<div class="grid grid-cols-[auto_repeat(4,minmax(2.25rem,1fr))] items-center gap-x-3 gap-y-1.5">
		<span></span>
		{#each readings as reading (reading.horizon)}
			<span class="eyebrow text-right">{reading.horizon}</span>
		{/each}

		<span class="eyebrow">MU</span>
		{#each readings as reading (reading.horizon)}
			<span class="text-right text-sm tabular-nums text-text">
				{reading.markupPct !== null ? formatPercent(reading.markupPct / 100) : NO_DATA}
			</span>
		{/each}

		<span class="eyebrow">Volume</span>
		{#each readings as reading (reading.horizon)}
			<span class="text-right text-sm tabular-nums text-text-secondary">
				{reading.salesPed > 0 ? formatVolume(reading.salesPed) : NO_DATA}
			</span>
		{/each}
	</div>
{/snippet}

<div class={fillAvailable ? 'flex h-full min-h-0 flex-col' : ''}>
	{#if controlsLabel}
		<div
			class="grid shrink-0 grid-cols-[minmax(0,1fr)_auto] items-start gap-x-5 pb-5"
			data-testid="stock-utility-strip"
		>
			<div class="flex min-w-0 flex-col items-start gap-3">
				{#if heading}{@render heading()}{/if}
				<div class="flex flex-wrap items-center gap-x-3 gap-y-2">
					{#if searchable}
						<SearchInput
							class="w-full sm:w-64"
							bind:value={query}
							placeholder="Find an item"
							aria-label="Find an item"
						/>
					{/if}
					<!-- Beside the search rather than opposite it: both narrow what
						the list shows, so they belong to the same gesture. -->
					<div class="flex items-center gap-1.5">
						{@render controlsLabel()}
						{#if controls}{@render controls()}{/if}
					</div>
				</div>
			</div>
			<div class="flex h-full items-end justify-end">
				{#if actions}{@render actions()}{/if}
			</div>
		</div>
	{:else}
		<div
			class="flex shrink-0 flex-wrap items-center gap-x-5 gap-y-2 pb-2"
			data-testid="stock-utility-strip"
		>
			<div class="flex items-center gap-2">
				{#if heading}
					{@render heading()}
				{:else}
					<h3 class="text-sm font-semibold tracking-tight text-text">Your Current Stock</h3>
				{/if}
				<InfoTip align="right" label="What current stock means">
					<div class="space-y-2 text-xs leading-relaxed text-text-secondary">
						<p class="font-semibold text-text">Your Current Stock: Loot you still hold</p>
						<p>{sourceDescription}</p>
						<p>
							Stock TT is its Trade Terminal value. Markup becomes realised when a sale is
							confirmed or Shrapnel is deliberately converted.
						</p>
					</div>
				</InfoTip>
			</div>

			{#if searchable}
				<SearchInput
					class="w-full sm:w-64"
					bind:value={query}
					placeholder="Find an item"
					aria-label="Find an item"
				/>
			{/if}

			{#if controls}<div class="ml-auto">{@render controls()}</div>{/if}
		</div>
	{/if}

	<div
		class="flex shrink-0 items-center gap-3 px-2.5 text-text-tertiary
			{actionLayout === 'inventory' ? 'border-b border-border py-2' : 'pb-1'}"
	>
		<span class="eyebrow flex-1 min-w-0">Item</span>
		<span class="eyebrow w-24 text-right shrink-0">TT</span>
		{#if actionLayout === 'inventory'}
			<span class="eyebrow w-24 text-right shrink-0">Packet TT</span>
		{/if}
		<span class="eyebrow w-20 text-right shrink-0">MU</span>
		<span class="eyebrow w-12 text-center shrink-0">Conf</span>
		<span class="eyebrow {actionLayout === 'inventory' ? 'w-[5.25rem]' : 'w-[7.125rem]'} shrink-0 text-right">Actions</span>
	</div>

	<div class="relative {fillAvailable ? 'min-h-0 flex-1' : ''}">
	<ul
		bind:this={stockList}
		class="flex flex-col gap-1 overflow-y-auto {fillAvailable ? 'h-full' : 'max-h-[24rem]'}"
		data-testid="stock-scroll-list"
		onscroll={updateScrollContinuation}
	>
		{#each visibleStock as item (item.itemName)}
			<!-- An emptied line stays, dimmed: the item is still one this
				activity produces, and its market reading is worth keeping
				legible for the next time there is stock to sell. -->
			{@const empty = item.heldQty <= 0}
			<li class="flex items-center gap-3 rounded-md px-2.5 py-2">
				<span
					class="flex-1 min-w-0 text-sm font-medium truncate tracking-tight
						{empty ? 'text-text-tertiary' : 'text-text'}"
				>
					{item.itemName}
				</span>

				<span
					class="w-24 text-right shrink-0 text-sm tabular-nums font-medium
						{empty ? 'text-text-tertiary' : 'text-text'}"
				>
					{formatPed(item.heldTt)}
					{#if item.listedQty > 0}
						<span
							class="block text-[0.625rem] font-normal text-text-tertiary tabular-nums"
							title="Out on an open auction; returns to stock if the listing expires"
						>
							{item.listedQty} listed
						</span>
					{/if}
				</span>

				{#if actionLayout === 'inventory'}
					{@const packetTt = item.recommendedPacketTt}
					<span
						aria-label={packetTt !== null && item.heldTt >= packetTt
							? `${formatPed(packetTt)} packet ready`
							: packetTt !== null
								? `${formatPed(packetTt)} recommended packet`
								: 'Recommended packet unavailable'}
						title={packetTt !== null && item.heldTt >= packetTt ? 'Packet ready' : undefined}
						class="w-24 text-right shrink-0 text-sm tabular-nums font-medium
							{packetTt !== null && item.heldTt >= packetTt
								? 'text-positive'
								: packetTt !== null
									? 'text-text'
									: 'text-text-tertiary'}"
					>
						{packetTt !== null ? formatPed(packetTt) : NO_DATA}
					</span>
				{/if}

				<div class="w-20 shrink-0 flex items-center justify-end">
					{#if item.effectiveMarkupPct !== null && item.markupPct !== null}
						{@const observedMarkup = item.markupPct}
						{@const appliedMarkup = item.effectiveMarkupPct}
						<InfoTip align="right" width="w-96" label={markupLabel(item)}>
							{#snippet trigger()}
								<span
									class="inline-flex h-5 flex-col items-end justify-center tabular-nums
										border-b border-dotted border-border/70"
								>
									{#if item.floored || item.markupBasis === 'shrapnel_conversion'}
										<span class="text-[9px] leading-[9px] text-text-tertiary line-through">
											{formatPercent(observedMarkup / 100)}
										</span>
										<span class="text-xs leading-[11px] text-text-secondary">
											{formatPercent(appliedMarkup / 100)}
										</span>
									{:else}
										<span class="text-sm leading-5 text-text-secondary">
											{formatPercent(appliedMarkup / 100)}
										</span>
									{/if}
								</span>
							{/snippet}
							{@render markupBreakdown(item.readings)}
						</InfoTip>
					{:else if item.effectiveMarkupPct !== null}
						{@const appliedMarkup = item.effectiveMarkupPct}
						<span
							class="inline-flex h-5 items-center text-sm tabular-nums text-text-secondary"
							aria-label={markupLabel(item)}
						>
							{formatPercent(appliedMarkup / 100)}
						</span>
					{:else}
						<span class="text-sm text-text-tertiary">{NO_DATA}</span>
					{/if}
				</div>

				<div class="w-12 shrink-0 flex items-center justify-center">
					{#if item.markupBasis === 'shrapnel_conversion'}
						<InfoTip align="right" width="w-96" label="Fixed Shrapnel conversion value">
							{@render confidenceBody(item)}
						</InfoTip>
					{:else if item.tier}
						<InfoTip
							align="right"
							width="w-96"
							label={confidenceTitle(item.tier)}
						>
							{#snippet trigger()}
								{#if item.tier === 'liquid'}
									<span class="text-positive" aria-label="High volume">✓</span>
								{:else if item.tier === 'middling'}
									<span class="text-warning" aria-label="Medium volume">⚠</span>
								{:else}
									<span class="text-error font-semibold" aria-label="Low volume">!</span>
								{/if}
							{/snippet}
							{@render confidenceBody(item)}
						</InfoTip>
					{:else}
						<span class="text-sm text-text-tertiary">{NO_DATA}</span>
					{/if}
				</div>

				<div
					class="shrink-0 flex items-center justify-end gap-1.5 {actionLayout === 'inventory'
						? 'min-w-[5.25rem]'
						: ''}"
				>
					{#if actionLayout === 'inventory'}
						{#if item.itemName === 'Shrapnel'}
							<ExpandingActionButton
								letter="C"
								label="Convert 101%"
								onclick={() => onshrapnelconvert(item)}
								disabled={item.heldQty <= 0}
								title={item.heldQty <= 0 ? 'Nothing held to convert' : ''}
							/>
						{:else if item.itemName !== 'Nanocube' && item.itemName !== 'Universal Ammo'}
							<ExpandingActionButton
								letter="N"
								label="Nanocubes"
								onclick={() => onconvert(item)}
								disabled={item.heldQty <= 0}
								title={item.heldQty <= 0 ? 'Nothing held to convert' : ''}
							/>
						{:else}
							<span class="h-6 w-6 shrink-0" aria-hidden="true"></span>
						{/if}
						<ExpandingActionButton
							letter="S"
							label="Sell"
							onclick={() => onsell(item)}
							disabled={item.heldQty <= 0}
							title={item.heldQty <= 0 ? 'Nothing held to sell' : ''}
						/>
						<ExpandingActionButton
							letter="X"
							label="Remove"
							onclick={() => onremove(item)}
							disabled={item.heldQty <= 0}
							title={item.heldQty <= 0 ? 'Nothing held to remove' : ''}
						/>
					{:else}
						{#if item.itemName === 'Shrapnel'}
							<ExpandingActionButton
								letter="C"
								label="Convert"
								onclick={() => onshrapnelconvert(item)}
								disabled={item.heldQty <= 0}
								title={item.heldQty <= 0 ? 'Nothing held to convert' : ''}
							/>
						{:else}
							<span class="h-6 w-6 shrink-0" aria-hidden="true"></span>
						{/if}
						<ExpandingActionButton
							letter="N"
							label="Nanocube"
							onclick={() => onconvert(item)}
							disabled={item.heldQty <= 0}
							title={item.heldQty <= 0 ? 'Nothing held to convert' : ''}
						/>
						<ExpandingActionButton
							letter="S"
							label="Sell"
							onclick={() => onsell(item)}
							disabled={item.heldQty <= 0}
							title={item.heldQty <= 0 ? 'Nothing held to sell' : ''}
						/>
						<ExpandingActionButton
							letter="X"
							label="Remove"
							onclick={() => onremove(item)}
							disabled={item.heldQty <= 0}
							title={item.heldQty <= 0 ? 'Nothing held to remove' : ''}
						/>
					{/if}
				</div>
			</li>
		{/each}
		{#if visibleStock.length === 0 && query.trim() !== ''}
			<li class="px-2.5 py-3 text-center text-xs text-text-tertiary">
				No stock item matches that search.
			</li>
		{:else if visibleStock.length === 0}
			<li class="px-2.5 py-10 text-center text-sm text-text-tertiary">
				{emptyMessage}
			</li>
		{/if}
	</ul>

		<div
			class="pointer-events-none absolute inset-x-0 bottom-0 flex h-14 items-end justify-center
				bg-gradient-to-t from-base via-base/75 to-transparent pb-1.5
				transition-opacity duration-[var(--duration-base)] ease-[var(--ease-out)]
				{hasMoreBelow ? 'opacity-100' : 'opacity-0'}"
			data-testid="stock-scroll-continuation"
			aria-hidden="true"
		>
			<svg
				class="h-5 w-5 text-text-tertiary/80 drop-shadow-sm"
				viewBox="0 0 20 20"
				fill="none"
				stroke="currentColor"
				stroke-width="1.35"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				<path d="m5 6.5 5 4 5-4" />
				<path d="m5 11 5 4 5-4" opacity="0.6" />
			</svg>
		</div>
	</div>

	{#if longList && emptiedCount > 0 && query.trim() === ''}
		<button
			type="button"
			class="mt-1 px-2.5 text-xs text-text-tertiary cursor-pointer
				transition-colors duration-[var(--duration-fast)] hover:text-text"
			onclick={() => (showEmptied = !showEmptied)}
		>
			{showEmptied
				? 'Hide emptied items'
				: `Show ${emptiedCount} emptied ${emptiedCount === 1 ? 'item' : 'items'}`}
		</button>
	{/if}
</div>
