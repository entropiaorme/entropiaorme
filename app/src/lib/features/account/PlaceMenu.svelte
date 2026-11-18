<script lang="ts">
	// Where an item's use costs, chosen once for every session: a compact
	// menu whose trigger reads "Place" for an item not yet placed, or the
	// placement made, with the choice to change or clear it. While the
	// choice is being saved the trigger says so, and every other trigger
	// waits too (`waiting`), so a second choice is never taken and dropped.
	// The trigger carries the item's name so a surface can hand focus back
	// to it once the figures it moved have been read again.
	import type { CostPlacementCategory } from '$lib/api/commands.gen';
	import Menu from '$lib/components/Menu.svelte';
	import { isCurrentChoice, PLACE_CHOICES, PLACEMENT_LABELS, type PlaceChoice } from './costLines';

	let {
		item,
		current = null,
		pending = false,
		waiting = false,
		onplace,
		onclear,
	}: {
		item: string;
		/** Where the player placed the item; `null` when they have not. */
		current?: CostPlacementCategory | null;
		/** Whether a placement of this item is being saved. */
		pending?: boolean;
		/** Whether another item's placement is being saved. */
		waiting?: boolean;
		onplace: (category: CostPlacementCategory) => void;
		onclear: () => void;
	} = $props();

	const label = $derived(
		pending ? 'Placing…' : current ? PLACEMENT_LABELS[current] : 'Place',
	);
	const GROUPS: { group: PlaceChoice['group']; heading: string | null }[] = [
		{ group: 'cycled', heading: 'Cycled' },
		{ group: 'overhead', heading: 'Overhead' },
		{ group: 'none', heading: null },
	];
</script>

<Menu overlay align="right" panelClass="w-52 p-1" class="inline-block shrink-0">
	{#snippet trigger({ open, toggle, keydown })}
		<button
			type="button"
			class="-mx-1 inline-flex cursor-pointer items-baseline gap-1 rounded px-1 text-xs
				transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover hover:text-text
				focus:outline-none focus-visible:[box-shadow:var(--shadow-glow)]
				disabled:cursor-default disabled:opacity-60
				{current ? 'text-text-secondary' : 'text-accent'}"
			aria-haspopup="menu"
			aria-expanded={open}
			aria-busy={pending}
			aria-label={current
				? `${item}: placed as ${PLACEMENT_LABELS[current]}; change or clear`
				: `Place ${item}`}
			data-place-item={item}
			disabled={pending || waiting}
			onclick={toggle}
			onkeydown={keydown}
		>
			{label}
			{#if !pending}
				<svg
					class="h-2.5 w-2.5 self-center transition-transform {open ? 'rotate-180' : ''}"
					viewBox="0 0 20 20"
					fill="currentColor"
					aria-hidden="true"
				>
					<path
						fill-rule="evenodd"
						d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
						clip-rule="evenodd"
					/>
				</svg>
			{/if}
		</button>
	{/snippet}

	{#snippet children({ close })}
		{#each GROUPS as { group, heading } (group)}
			{#if heading}
				<p class="eyebrow px-2 pb-1 pt-1.5">{heading}</p>
			{:else}
				<div class="mx-2 my-1 border-t border-border/50" role="separator"></div>
			{/if}
			{#each PLACE_CHOICES.filter((choice) => choice.group === group) as choice (choice.category)}
				{@const chosen = isCurrentChoice(choice, current)}
				<button
					type="button"
					role="menuitemradio"
					aria-checked={chosen}
					class="flex w-full cursor-pointer items-center rounded px-2 py-1.5 text-left text-xs
						transition-colors duration-[var(--duration-fast)]
						{chosen ? 'bg-accent/10 text-accent' : 'text-text-secondary hover:bg-surface-hover hover:text-text'}"
					onclick={() => {
						close();
						if (!chosen) onplace(choice.category);
					}}
				>
					{choice.label}
				</button>
			{/each}
		{/each}
		{#if current}
			<div class="mx-2 my-1 border-t border-border/50" role="separator"></div>
			<button
				type="button"
				role="menuitem"
				class="flex w-full cursor-pointer items-center rounded px-2 py-1.5 text-left text-xs text-text-secondary
					transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover hover:text-text"
				onclick={() => {
					close();
					onclear();
				}}
			>
				Clear placement
			</button>
		{/if}
		<p class="px-2 pb-1 pt-1.5 text-[11px] leading-snug text-text-tertiary">
			Remembered for every session
		</p>
	{/snippet}
</Menu>
