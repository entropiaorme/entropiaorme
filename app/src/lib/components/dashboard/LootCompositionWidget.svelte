<script lang="ts">
	import MarkupConfidenceControl from '$lib/features/analytics/MarkupConfidenceControl.svelte';
	import { markupLabel } from '$lib/features/analytics/marketConfidence';
	import {
		type ConfidenceMode,
		isShrapnel,
	} from '$lib/features/analytics/treeCuttingModel.svelte';
	import type { LiveLootValuation } from '$lib/features/dashboard/liveLootValuation.svelte';
	import {
		type CompositionRow,
		composeLoot,
		segmentKeyOf,
	} from '$lib/features/dashboard/lootComposition';
	import ManualMarkupEditor from '$lib/features/market/ManualMarkupEditor.svelte';
	import NanocubeMarkButton from '$lib/features/market/NanocubeMarkButton.svelte';
	import { liveMarkupSettings, setLiveConfidence } from '$lib/liveMarkupSettings.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';

	let {
		sessionId,
		valuation,
	}: {
		sessionId: string | null;
		/** The session's live loot valuation, shared with the stat tiles. */
		valuation: LiveLootValuation | null;
	} = $props();

	let hovered = $state<string | null>(null);
	let expanded = $state(false);
	let actionError = $state<string | null>(null);

	const view = $derived(valuation?.view ?? null);
	const composition = $derived(
		composeLoot(valuation?.loot ?? [], view?.items ?? null, expanded),
	);
	const marketByItem = $derived(
		new Map((valuation?.market?.items ?? []).map((item) => [item.itemName, item])),
	);
	const loading = $derived(sessionId !== null && (valuation?.loading ?? true));

	function isLit(row: CompositionRow): boolean {
		return hovered !== null && (hovered === row.key || hovered === segmentKeyOf(row));
	}

	function markupClass(row: CompositionRow): string {
		const basis = row.valued?.markupBasis;
		if (basis === 'manual') return 'text-accent';
		if (basis === 'nanocube' || basis === 'nanocube_mark') return 'text-text-tertiary';
		return 'text-text';
	}

	/** Actions stay visible on a row the player has already set. */
	function pinned(row: CompositionRow): boolean {
		const market = marketByItem.get(row.name);
		return Boolean(market?.manual || market?.nanocubeMarked);
	}

	function changed() {
		actionError = null;
		valuation?.marketChanged();
	}
</script>

<div class="flex-1 min-h-0 flex flex-col gap-4">
	{#if !sessionId}
		<!-- Idle: no active session -->
		<div class="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
			<div class="relative h-12 w-12 rounded-full border border-border/60 flex items-center justify-center
				before:absolute before:inset-[-6px] before:rounded-full before:border before:border-accent/15
				after:absolute after:inset-[-14px] after:rounded-full after:border after:border-accent/[0.06]">
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor"
					class="h-5 w-5 text-text-tertiary">
					<path fill-rule="evenodd" d="M10 1.5a.75.75 0 0 1 .75.75V4.5a.75.75 0 0 1-1.5 0V2.25A.75.75 0 0 1 10 1.5ZM5.404 4.343a.75.75 0 0 1 1.06 0l1.591 1.59a.75.75 0 1 1-1.06 1.061l-1.591-1.59a.75.75 0 0 1 0-1.061ZM14.596 4.343a.75.75 0 0 1 0 1.06l-1.591 1.591a.75.75 0 1 1-1.06-1.06l1.59-1.591a.75.75 0 0 1 1.061 0ZM2.25 9.25a.75.75 0 0 0 0 1.5H4.5a.75.75 0 0 0 0-1.5H2.25ZM15.5 10a.75.75 0 0 1 .75-.75h2.25a.75.75 0 0 1 0 1.5H16.25A.75.75 0 0 1 15.5 10ZM10 6a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z" clip-rule="evenodd" />
				</svg>
			</div>
			<div class="flex flex-col gap-1 max-w-xs">
				<p class="text-sm text-text-secondary tracking-tight">No active session</p>
				<p class="text-xs text-text-tertiary leading-relaxed">
					Start tracking to watch your loot composition in real time.
				</p>
			</div>
		</div>
	{:else if loading}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-xs text-text-tertiary animate-pulse tracking-wider uppercase">Loading…</p>
		</div>
	{:else if composition.segments.length === 0}
		<!-- Active but no loot yet -->
		<div class="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
			<span class="signal-dot positive animate-pulse"></span>
			<div class="flex flex-col gap-1 max-w-xs">
				<p class="text-sm text-text-secondary tracking-tight">Tracking: no loot yet</p>
				<p class="text-xs text-text-tertiary leading-relaxed">
					Items will appear here as they enter your session ledger.
				</p>
			</div>
		</div>
	{:else}
		<!-- Valuation summary and the confidence the live figures trust -->
		<div class="flex items-center justify-between gap-3 mt-1">
			<p class="text-xs text-text-secondary tabular-nums" data-testid="composition-loot-mu">
				{#if view?.lootMuFactor != null}
					Loot MU <span class="font-semibold text-text">{formatPercent(view.lootMuFactor)}</span>
					<span class="text-text-tertiary">· {formatPed(view.marketValuePed)} PED at market</span>
				{:else if valuation?.marketFailed}
					<span class="text-text-tertiary">Market markup unavailable</span>
				{:else}
					<span class="text-text-tertiary">Valuing at market…</span>
				{/if}
			</p>
			<MarkupConfidenceControl
				active={liveMarkupSettings.confidence}
				onchange={(mode: ConfidenceMode) => void setLiveConfidence(mode)}
			/>
		</div>

		<!-- Composition bar -->
		<div class="relative h-2.5 rounded-full overflow-hidden bg-base/60 border border-border/60 flex">
			{#each composition.segments as seg (seg.key)}
				<button
					type="button"
					class="relative h-full transition-[opacity,filter] duration-[var(--duration-base)] ease-[var(--ease-out)]
						first:rounded-l-full last:rounded-r-full
						{hovered && hovered !== seg.key ? 'opacity-35' : ''}
						{hovered === seg.key ? '[filter:brightness(1.15)]' : ''}"
					style="width: {seg.sharePct}%; background-color: {seg.color};"
					aria-label="{seg.name}: {seg.sharePct.toFixed(1)}% of loot TT"
					onmouseenter={() => (hovered = seg.key)}
					onmouseleave={() => (hovered = null)}
					onfocus={() => (hovered = seg.key)}
					onblur={() => (hovered = null)}
				></button>
			{/each}
		</div>

		{#if actionError}
			<p class="text-xs text-error -mb-1" role="alert">{actionError}</p>
		{/if}

		<!-- Column headers -->
		<div class="flex items-center gap-3 px-2.5 -mb-1">
			<div class="flex items-center gap-2.5 shrink-0">
				<span class="w-4"></span>
				<span class="w-2.5"></span>
			</div>
			<span class="eyebrow flex-1 min-w-0">Item</span>
			<span class="hidden xl:block w-16 shrink-0"></span>
			<span class="eyebrow w-16 text-right shrink-0">TT</span>
			<span class="eyebrow w-16 text-right shrink-0">MU</span>
			<span class="eyebrow w-12 text-right shrink-0">Share</span>
			<span class="w-14 shrink-0"></span>
		</div>

		<!-- Ranked list -->
		<div class="flex-1 min-h-0 overflow-y-auto -mr-2 pr-2">
			<ul class="flex flex-col gap-1" data-testid="loot-composition-rows">
				{#each composition.rows as row (row.key)}
					<li>
						<div
							class="group relative flex items-center gap-3 rounded-md px-2.5 py-2
								transition-[background-color,border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
								border border-transparent
								{isLit(row)
									? 'bg-surface-hover/50 border-border/50'
									: 'hover:bg-surface-hover/30 hover:border-border/40'}"
							onmouseenter={() => (hovered = row.key)}
							onmouseleave={() => (hovered = null)}
							role="presentation"
						>
							<!-- Rank + swatch -->
							<div class="flex items-center gap-2.5 shrink-0">
								<span class="text-[10.5px] font-medium tabular-nums text-text-tertiary tracking-[0.12em] w-4 text-right">
									{row.isOther ? '' : row.rank}
								</span>
								<span
									class="block h-2.5 w-2.5 rounded-sm shrink-0
										transition-[box-shadow] duration-[var(--duration-base)] ease-[var(--ease-out)]"
									style="background-color: {row.color};
										box-shadow: 0 0 0 1px color-mix(in oklab, {row.color} 40%, transparent),
											{isLit(row)
												? `0 0 10px color-mix(in oklab, ${row.color} 60%, transparent)`
												: 'none'};"
								></span>
							</div>

							<!-- Name + qty (the pool expands into its items) -->
							<div class="flex-1 min-w-0 flex items-center gap-2">
								{#if row.isOther}
									<button
										type="button"
										class="text-sm italic text-text-tertiary hover:text-text-secondary truncate tracking-tight"
										aria-expanded={expanded}
										onclick={() => (expanded = true)}
									>
										{row.name}
									</button>
								{:else}
									<span class="text-sm font-medium truncate tracking-tight text-text">{row.name}</span>
									<span class="text-xs text-text-tertiary tabular-nums shrink-0">×{row.quantity}</span>
								{/if}
							</div>

							<!-- Mini share bar -->
							<div class="hidden xl:block w-16 h-1 rounded-full bg-base/60 overflow-hidden shrink-0">
								<div
									class="h-full rounded-full transition-[width] duration-[var(--duration-slow)] ease-[var(--ease-out)]"
									style="width: {row.sharePct}%; background-color: {row.color};"
								></div>
							</div>

							<!-- TT value -->
							<span class="text-sm tabular-nums font-medium text-text shrink-0 w-16 text-right">
								{formatPed(row.ttValue)}
							</span>

							<!-- Applied markup, with where it came from -->
							<span
								class="text-sm tabular-nums shrink-0 w-16 text-right {markupClass(row)}"
								title={row.valued ? markupLabel(row.valued) : undefined}
								data-testid="composition-markup"
							>
								{#if row.valued}
									{#if row.valued.markupBasis === 'nanocube_mark'}
										<span class="line-through decoration-text-tertiary/70">NC</span>
									{/if}
									{formatPercent(row.valued.effectiveMarkupPct / 100)}{#if row.valued.markupBasis === 'manual'}<span
											class="ml-0.5 align-super text-[9px]"
											aria-hidden="true">✎</span
										>{/if}
									<span class="sr-only">, {markupLabel(row.valued)}</span>
								{:else}
									<span class="text-text-tertiary">{row.isOther ? '' : NO_DATA}</span>
								{/if}
							</span>

							<!-- Share % -->
							<span class="text-sm tabular-nums font-semibold shrink-0 w-12 text-right tracking-tight
								{row.isOther ? 'text-text-tertiary' : 'text-accent'}">
								{row.sharePct.toFixed(1)}%
							</span>

							<!-- Row actions: correct the markup, or scratch it to Nanocube -->
							<div class="flex w-14 shrink-0 items-center justify-end gap-0.5
								transition-opacity duration-[var(--duration-fast)]
								{pinned(row) ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-within:opacity-100'}">
								{#if !row.isOther && !isShrapnel(row.name)}
									<NanocubeMarkButton
										itemName={row.name}
										marked={marketByItem.get(row.name)?.nanocubeMarked ?? false}
										onChanged={changed}
										onError={(message) => (actionError = message)}
									/>
									<ManualMarkupEditor
										itemName={row.name}
										quantity={row.quantity}
										ttValue={row.ttValue}
										market={marketByItem.get(row.name)}
										applied={row.valued
											? { markupPct: row.valued.effectiveMarkupPct, basis: row.valued.markupBasis }
											: null}
										onChanged={changed}
									/>
								{/if}
							</div>
						</div>
					</li>
				{/each}
				{#if expanded && composition.pooled > 0}
					<li>
						<button
							type="button"
							class="w-full px-2.5 py-1.5 text-left text-xs text-text-tertiary hover:text-text-secondary"
							aria-expanded={expanded}
							onclick={() => (expanded = false)}
						>
							Show the top {composition.rows.length - composition.pooled} only
						</button>
					</li>
				{/if}
			</ul>
		</div>
	{/if}
</div>
