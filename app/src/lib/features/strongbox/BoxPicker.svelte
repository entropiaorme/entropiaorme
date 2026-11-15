<script lang="ts">
	/**
	 * Which box this run opens: one of the box types held, or one bought for
	 * the run that the inventory has never held.
	 */
	import Input from '$lib/components/Input.svelte';
	import { formatQuantity } from '$lib/utils/format';
	import type { BoxHolding } from './openingDraft';

	let {
		holdings,
		selected,
		onchoose,
	}: {
		holdings: BoxHolding[];
		selected: string | null;
		onchoose: (name: string) => void;
	} = $props();

	const isHeld = $derived(
		holdings.some((row) => row.name.toLocaleLowerCase() === (selected ?? '').toLocaleLowerCase()),
	);
	let typing = $state(false);
	let typed = $state('');
	$effect(() => {
		if (holdings.length === 0) typing = true;
	});

	function describe(holding: BoxHolding): string {
		const parts: string[] = [];
		if (holding.lootQty > 0) parts.push(`${formatQuantity(holding.lootQty)} looted`);
		const assets = holding.assets.reduce((sum, asset) => sum + asset.quantity, 0);
		if (assets > 0) parts.push(`${formatQuantity(assets)} held as an asset`);
		return parts.join(' · ');
	}
</script>

<div class="space-y-4">
	{#if holdings.length > 0}
		<div role="radiogroup" aria-label="Box to open" class="divide-y divide-border/50 border-y border-border/50">
			{#each holdings as holding (holding.name)}
				{@const active = holding.name === selected}
				<button
					type="button"
					role="radio"
					aria-checked={active}
					class="group relative flex w-full items-center gap-4 px-4 py-3.5 text-left transition-colors duration-[var(--duration-base)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-accent/60 {active
						? 'bg-accent-faint'
						: 'hover:bg-surface-hover/40'}"
					onclick={() => {
						typing = false;
						onchoose(holding.name);
					}}
				>
					<span
						class="absolute inset-y-0 left-0 w-0.5 transition-colors {active ? 'bg-accent' : 'bg-transparent'}"
						aria-hidden="true"
					></span>
					<span
						class="flex h-4 w-4 shrink-0 items-center justify-center rounded-full border transition-colors {active
							? 'border-accent'
							: 'border-border-bright group-hover:border-text-tertiary'}"
						aria-hidden="true"
					>
						{#if active}<span class="h-2 w-2 rounded-full bg-accent"></span>{/if}
					</span>
					<span class="min-w-0 flex-1">
						<span class="block truncate text-sm font-medium {active ? 'text-text' : 'text-text-secondary group-hover:text-text'}">
							{holding.name}
						</span>
						<span class="block text-xs text-text-tertiary">{describe(holding)}</span>
					</span>
					<span
						class="shrink-0 text-2xl font-light tabular-nums {active ? 'text-text' : 'text-text-tertiary'}"
					>
						{formatQuantity(holding.total)}
					</span>
				</button>
			{/each}
		</div>
	{:else}
		<p class="text-sm text-text-secondary">No strongboxes are held. Name the box you are opening.</p>
	{/if}

	{#if typing}
		<div class="flex max-w-md items-center gap-2">
			<Input
				aria-label="Box name"
				placeholder="Halloween Strongbox"
				bind:value={typed}
				oninput={() => onchoose(typed)}
			/>
		</div>
		{#if typed.trim() && !/strongbox\s*$/i.test(typed)}
			<p class="text-xs text-text-tertiary">Boxes are usually named as the game does: "… Strongbox".</p>
		{/if}
	{:else}
		<button
			type="button"
			class="text-xs font-medium text-accent hover:text-text"
			onclick={() => {
				typing = true;
				if (!isHeld) typed = selected ?? '';
			}}
		>
			Opening boxes you bought that are not in your inventory?
		</button>
	{/if}
</div>
