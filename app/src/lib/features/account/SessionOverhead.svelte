<script lang="ts">
	// What the session cost beyond performing its activity, as the account
	// readings measured it: travel, field logistics, and what the app cannot
	// place yet, each opening onto the items behind it. Overhead is in the
	// session's Net and never in Cycled or its return rate, so the group ends
	// with the Net and its parts. Every figure is the backend's.
	//
	// What the app could not place is open from the start, since each of its
	// items asks the player to place it once; so is the category an item was
	// just placed in, so the player sees where it went.
	import type { CostBreakdown } from '$lib/api/commands.gen';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { formatPed } from '$lib/utils/format';
	import CostLineRow from './CostLineRow.svelte';
	import { COST_REASONS } from './costBasis';
	import {
		type OverheadBucket,
		type OverheadCategory,
		overheadCategories,
		overheadWithheld,
	} from './costLines';
	import type { CostPlacementsModel } from './costPlacementsModel.svelte';

	let {
		breakdown,
		lootTt,
		cycled,
		net,
		placements = null,
	}: {
		breakdown: CostBreakdown;
		lootTt: number;
		cycled: number;
		net: number;
		placements?: CostPlacementsModel | null;
	} = $props();

	const categories = $derived(overheadCategories(breakdown));
	const baseId = `session-overhead-${crypto.randomUUID()}`;
	let closed = $state<Partial<Record<OverheadBucket, boolean>>>({});
	function isOpen(bucket: OverheadBucket, lines: { item: string }[]): boolean {
		const settled = placements?.lastPlacement?.item ?? null;
		const openFirst =
			bucket === 'unclassified' || (settled !== null && lines.some((line) => line.item === settled));
		return !(closed[bucket] ?? !openFirst);
	}

	// A category, or the whole overhead, that bills nothing says why rather
	// than reading as a measured nothing: withheld with a reason (on hover
	// or focus), or holding only items to place.
	const NOTHING_BILLED = 'nothing billed; items to place';
	const billsNothing = $derived(breakdown.overheadCost === 0);
	const toPlace = $derived(categories.some((category) => category.lines.length > 0));
</script>

{#snippet withheld(category: OverheadCategory)}
	{' '}<span class="text-text-tertiary" data-testid="overhead-reason-{category.bucket}"
		><InfoTip label="not billed" align="left" width="w-64">
			{#snippet trigger()}<span class="underline decoration-dotted underline-offset-2 hover:text-text-secondary"
					>not billed</span
				>{/snippet}
			<span class="block text-xs leading-relaxed text-text-secondary"
				>{category.reason ? COST_REASONS[category.reason] : ''}</span
			>
		</InfoTip></span
	>
{/snippet}

{#if categories.length > 0}
	<div data-testid="session-overhead" aria-labelledby="{baseId}-heading" role="group">
		<div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
			<h3 id="{baseId}-heading" class="eyebrow">Overhead</h3>
			{#if billsNothing}
				<span class="text-xs text-text-tertiary" data-testid="overhead-nothing-billed">
					{toPlace ? 'Nothing billed; items to place' : 'Nothing billed'}
				</span>
			{:else}
				<span class="text-sm tabular-nums text-text">{formatPed(breakdown.overheadCost)}</span>
				<span class="text-xs text-text-tertiary">beyond Cycled</span>
			{/if}
		</div>

		<div class="mt-2 space-y-1 pl-1 text-xs">
			{#each categories as category (category.bucket)}
				{@const open = isOpen(category.bucket, category.lines)}
				{@const panelId = `${baseId}-${category.bucket}`}
				<div data-testid="overhead-{category.bucket}">
					{#if category.lines.length > 0}
						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-sm text-text-secondary hover:text-text
								transition-colors duration-[var(--duration-fast)]
								focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent"
							aria-expanded={open}
							aria-controls={open ? panelId : undefined}
							onclick={() => (closed = { ...closed, [category.bucket]: open })}
						>
							<svg
								viewBox="0 0 20 20"
								fill="currentColor"
								class="h-3 w-3 transition-transform duration-[var(--duration-fast)] {open ? 'rotate-90' : ''}"
								aria-hidden="true"
							>
								<path
									fill-rule="evenodd"
									d="M7.21 14.77a.75.75 0 01.02-1.06L11.17 10 7.23 6.29a.75.75 0 111.04-1.08l4.5 4.25a.75.75 0 010 1.08l-4.5 4.25a.75.75 0 01-1.06-.02z"
									clip-rule="evenodd"
								/>
							</svg>
							{category.label}:
							{#if overheadWithheld(category) === 'toPlace'}
								<span class="text-text-tertiary">{NOTHING_BILLED}</span>
							{:else if overheadWithheld(category) === null}
								<span class="tabular-nums text-text">{formatPed(category.amount)}</span>
							{/if}
						</button>{#if category.reason}{@render withheld(category)}{/if}
						{#if open}
							<div id={panelId} class="ml-4 mt-1 border-l border-border/40 pl-3">
								{#each category.lines as line, index (`${index}-${line.item}`)}
									<CostLineRow
										{line}
										{placements}
										unplaced={category.bucket === 'unclassified'}
									/>
								{/each}
							</div>
						{/if}
					{:else}
						<span class="pl-4 text-text-secondary">
							{category.label}:{#if category.reason}{@render withheld(category)}{:else}
								<span class="tabular-nums text-text">{formatPed(category.amount)}</span>{/if}
						</span>
					{/if}
				</div>
			{/each}
		</div>

		{#if !billsNothing}
			<!-- The Net's parts, each as the backend rounds it: listed, never
			     claimed to sum exactly. -->
			<p class="mt-2 pl-1 text-xs text-text-tertiary" data-testid="overhead-arithmetic">
				Net <span class="tabular-nums">{formatPed(net)}</span>: Loot TT
				<span class="tabular-nums">{formatPed(lootTt)}</span>, Cycled
				<span class="tabular-nums">{formatPed(cycled)}</span>, Overhead
				<span class="tabular-nums">{formatPed(breakdown.overheadCost)}</span>
			</p>
		{/if}
	</div>
{/if}
