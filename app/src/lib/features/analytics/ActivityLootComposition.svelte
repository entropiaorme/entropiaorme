<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { filterByQuery } from '$lib/view/search';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import { confidenceTip, confidenceTitle, markupLabel } from './marketConfidence';
	import type { TreeCuttingItem } from './treeCuttingModel.svelte';

	let {
		items,
		marketAvailable,
		emptyLabel,
		disclosure,
	}: {
		items: TreeCuttingItem[];
		marketAvailable: boolean;
		emptyLabel: string;
		disclosure?: 'session' | 'activity';
	} = $props();

	const SEARCH_THRESHOLD = 8;
	let query = $state('');
	let expanded = $state(false);
	const searchable = $derived(!disclosure && (items.length > SEARCH_THRESHOLD || query !== ''));
	const visible = $derived(disclosure ? items : filterByQuery(items, query, (item) => item.name));
	const disclosureId = $derived(disclosure ? `${disclosure}-loot-composition` : undefined);
	$effect(() => {
		void items;
		expanded = false;
	});
</script>

{#snippet confidenceBody(item: TreeCuttingItem)}
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

{#if items.length > 0}
	<div class="mt-5 {disclosure ? '' : 'border-t border-border/50 pt-4'}">
		{#if disclosure}
			<button
				type="button"
				class="group flex w-full cursor-pointer items-center justify-center gap-2 rounded-md py-1.5
					text-xs font-medium uppercase tracking-wider text-text-tertiary transition-colors
					duration-[var(--duration-fast)] hover:bg-surface-hover/30 hover:text-text
					focus:outline-none focus:[box-shadow:var(--shadow-glow)]"
				aria-expanded={expanded}
				aria-controls={disclosureId}
				onclick={() => (expanded = !expanded)}
			>
				<span class="text-sm transition-transform" aria-hidden="true">{expanded ? '↑' : '↓'}</span>
				<span>{expanded ? `Hide ${disclosure} loot` : `Show ${disclosure} loot`}</span>
				<span class="text-sm transition-transform" aria-hidden="true">{expanded ? '↑' : '↓'}</span>
			</button>
		{/if}

		{#if !disclosure || expanded}
			<div id={disclosureId} class={disclosure ? 'mt-3' : ''}>
				{#if searchable}
					<div class="px-2.5 pb-3">
						<SearchInput bind:value={query} placeholder="Find an item" aria-label="Find an item" />
					</div>
				{/if}
				<div
					class="flex items-center gap-3 px-2.5 pb-1 text-text-tertiary"
					data-testid={disclosure ? `${disclosure}-loot-header` : undefined}
				>
					<span class="eyebrow flex-1 min-w-0">Item</span>
					<span class="eyebrow w-20 text-right shrink-0">TT</span>
					<span class="eyebrow w-14 text-right shrink-0">Share</span>
					<span class="eyebrow w-20 text-right shrink-0">Markup</span>
					<span class="eyebrow w-12 text-center shrink-0">Conf</span>
				</div>

				<ul
					class="flex flex-col gap-1 {disclosure ? 'max-h-[24rem] overflow-y-auto pr-1' : ''}"
					data-testid={disclosure ? `${disclosure}-loot-list` : undefined}
				>
			{#each visible as item (item.name)}
				{@const valuationAvailable = marketAvailable || item.markupBasis === 'shrapnel_conversion'}
				<li
					class="flex items-center gap-3 rounded-md border border-transparent px-2.5 py-2
						hover:border-border/40 hover:bg-surface-hover/30
						transition-[background-color,border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]"
				>
					<span class="flex-1 min-w-0 truncate text-sm font-medium tracking-tight text-text">
						{item.name}
					</span>
					<span class="w-20 shrink-0 text-right text-sm tabular-nums font-medium text-text">
						{formatPed(item.ttValue)}
					</span>
					<span class="w-14 shrink-0 text-right text-sm tabular-nums font-semibold tracking-tight text-accent">
						{item.sharePct.toFixed(1)}%
					</span>

					<div class="w-20 shrink-0 flex items-center justify-end">
						{#if !valuationAvailable}
							<span class="text-sm text-text-tertiary">{NO_DATA}</span>
						{:else}
							<span
								class="inline-flex h-5 flex-col items-end justify-center tabular-nums"
								aria-label={markupLabel(item)}
							>
								{#if (item.floored || item.markupBasis === 'shrapnel_conversion') && item.ownMarkupPct !== null}
									<span class="text-[9px] leading-[9px] text-text-tertiary line-through">
										{formatPercent(item.ownMarkupPct / 100)}
									</span>
									<span class="text-xs leading-[11px] text-text-secondary">
										{formatPercent(item.effectiveMarkupPct / 100)}
									</span>
								{:else}
									<span class="text-sm leading-5 text-text-secondary">
										{formatPercent(item.effectiveMarkupPct / 100)}
									</span>
								{/if}
							</span>
						{/if}
					</div>

					<div class="w-12 shrink-0 flex items-center justify-center">
						{#if !valuationAvailable}
							<span class="text-sm text-text-tertiary">{NO_DATA}</span>
						{:else if item.markupBasis === 'shrapnel_conversion'}
							<InfoTip align="right" width="w-96" label="Fixed Shrapnel conversion value">
								{@render confidenceBody(item)}
							</InfoTip>
						{:else}
							<InfoTip align="right" width="w-96" label={confidenceTitle(item.tier)}>
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
						{/if}
					</div>
				</li>
			{/each}
			{#if visible.length === 0}
				<li class="px-2.5 py-3 text-center text-xs text-text-tertiary">
					No loot item matches that search.
				</li>
			{/if}
				</ul>
			</div>
		{/if}
	</div>
{:else}
	<p class="mt-4 px-2.5 text-xs text-text-tertiary">{emptyLabel}</p>
{/if}
