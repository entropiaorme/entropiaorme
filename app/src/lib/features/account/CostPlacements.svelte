<script lang="ts">
	// Every item the measured sessions could not place, in one list, so the
	// player can place each once without opening the sessions it was in:
	// the item, how many sessions, the TT it lost wearing and the units of it
	// that left, and the same Place menu the session detail offers. Until
	// placed, what an item lost wearing costs as unclassified overhead, and
	// units that left cost nothing. The placements made are listed below,
	// each with the choice to change or clear it.
	import { onMount } from 'svelte';
	import type { CostPlacementCategory } from '$lib/api/commands.gen';
	import { ErrorNotice, Skeleton } from '$lib/components';
	import { awaitingSummary } from './costLines';
	import { createCostPlacementsModel } from './costPlacementsModel.svelte';
	import PlaceMenu from './PlaceMenu.svelte';

	let {
		connected,
	}: {
		/** Whether the player has connected the account: the list shows then,
		 * and otherwise only while it holds anything, so a placement made
		 * stays reachable after disconnecting. */
		connected: boolean;
	} = $props();

	let root = $state<HTMLElement | null>(null);
	const model = createCostPlacementsModel({ focusFallback: () => root });
	onMount(() => {
		const detach = model.connect();
		void model.refresh();
		return detach;
	});

	const shown = $derived(connected || model.holdsAny);

	function place(item: string, category: CostPlacementCategory) {
		void model.place(item, category);
	}

	function clear(item: string) {
		void model.clear(item);
	}
</script>

{#snippet failureFor(item: string)}
	{#if model.failure?.item === item}
		<p class="mt-0.5 text-[11px] leading-snug text-negative" role="alert" data-testid="place-error">
			{model.failure.message}
		</p>
	{/if}
{/snippet}

{#if shown}
	<div
		bind:this={root}
		tabindex="-1"
		class="border-t border-border/40 py-5 focus:outline-none"
		data-testid="cost-placements"
		aria-labelledby="cost-placements-heading"
		role="group"
	>
		<h3 id="cost-placements-heading" class="text-sm text-text">Items to place</h3>
		<p class="mt-0.5 max-w-xl text-xs text-text-tertiary">
			Items your readings show wearing, or leaving your inventory, that the app cannot place. Until
			you place one, what it lost wearing costs as unclassified overhead, and units that left cost
			nothing.
		</p>

		<div class="mt-3" aria-busy={!model.answered && !model.error}>
			{#if model.placements}
				{@const { awaiting, placed } = model.placements}
				{#if awaiting.length === 0}
					<p class="text-xs text-text-tertiary" data-testid="nothing-to-place">Nothing to place</p>
				{:else}
					<ul class="divide-y divide-border/30 text-xs" data-testid="awaiting-placement">
						{#each awaiting as entry (entry.item)}
							<li class="py-1.5">
								<div class="flex items-baseline justify-between gap-4">
									<span class="min-w-0 break-words text-text">{entry.item}</span>
									<span class="flex shrink-0 items-baseline gap-4">
										<span class="tabular-nums text-text-tertiary">{awaitingSummary(entry)}</span>
										<PlaceMenu
											item={entry.item}
											pending={model.pending === entry.item}
											waiting={model.pending !== null && model.pending !== entry.item}
											onplace={(category) => place(entry.item, category)}
											onclear={() => clear(entry.item)}
										/>
									</span>
								</div>
								{@render failureFor(entry.item)}
							</li>
						{/each}
					</ul>
				{/if}

				{#if placed.length > 0}
					<p class="eyebrow mb-1 mt-4">Placed</p>
					<ul class="divide-y divide-border/30 text-xs" data-testid="placed-items">
						{#each placed as entry (entry.item)}
							<li class="py-1.5">
								<div class="flex items-baseline justify-between gap-4">
									<span class="min-w-0 break-words text-text-secondary">{entry.item}</span>
									<PlaceMenu
										item={entry.item}
										current={entry.category}
										pending={model.pending === entry.item}
										waiting={model.pending !== null && model.pending !== entry.item}
										onplace={(category) => place(entry.item, category)}
										onclear={() => clear(entry.item)}
									/>
								</div>
								{@render failureFor(entry.item)}
							</li>
						{/each}
					</ul>
				{/if}
			{:else if model.error}
				<ErrorNotice message={model.error} />
			{:else}
				<Skeleton class="h-3 w-48" />
				<Skeleton class="mt-2 h-3 w-72" />
			{/if}
		</div>
		<p class="sr-only" role="status" aria-live="polite" data-testid="placement-status">
			{model.announcement}
		</p>
	</div>
{/if}
