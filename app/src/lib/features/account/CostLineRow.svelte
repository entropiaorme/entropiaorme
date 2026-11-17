<script lang="ts">
	// One measured cost line: the item, what it measured (the units used or
	// the TT lost), and its figure, or why it has none. A consumable billed
	// at TT says quietly that its markup is left out; a ledger entry pegged
	// to the session that may record the same cost by hand is named, and
	// nothing is changed. Given the placements, an item the app could not
	// place offers "Place", and an item the player placed shows where, with
	// the choice to change or clear it.
	import type { CostPlacementCategory, MeasuredCostLine } from '$lib/api/commands.gen';
	import { lineFigure, lineQuantity, peggedLedgerNote } from './costLines';
	import type { CostPlacementsModel } from './costPlacementsModel.svelte';
	import PlaceMenu from './PlaceMenu.svelte';

	let {
		line,
		placements = null,
		unplaced = false,
	}: {
		line: MeasuredCostLine;
		placements?: CostPlacementsModel | null;
		/** The line is one the app could not place: it offers "Place". */
		unplaced?: boolean;
	} = $props();

	const figure = $derived(lineFigure(line));
	const placedAs = $derived(placements?.placedAs(line.item) ?? null);
	const offersPlace = $derived(!!placements && (unplaced || placedAs !== null));
	const failure = $derived(
		placements?.failure?.item === line.item ? placements.failure.message : null,
	);

	function place(category: CostPlacementCategory) {
		void placements?.place(line.item, category);
	}

	function clear() {
		void placements?.clear(line.item);
	}
</script>

<div class="py-0.5" data-testid="cost-line">
	<div class="flex items-baseline justify-between gap-4">
		<span class="min-w-0 break-words text-text-secondary">
			<span class="text-text">{line.item}</span>
			<span class="text-text-tertiary">{lineQuantity(line)}{#if line.markupNotIncluded}; markup not included{/if}</span>
		</span>
		<span class="flex shrink-0 items-baseline gap-3">
			{#if offersPlace && placements}
				<PlaceMenu
					item={line.item}
					current={placedAs}
					pending={placements.pending === line.item}
					waiting={placements.pending !== null && placements.pending !== line.item}
					onplace={place}
					onclear={clear}
				/>
			{/if}
			<span
				class="tabular-nums {line.amount === null ? 'text-text-tertiary' : 'text-text'}"
				title={figure.title ?? undefined}>{figure.text}</span
			>
		</span>
	</div>
	{#if line.peggedLedgerEntryId}
		<p class="mt-0.5 text-[11px] leading-snug text-warning" data-testid="cost-line-ledger">
			{peggedLedgerNote(line.peggedLedgerEntryDescription)}
		</p>
	{/if}
	{#if failure}
		<p class="mt-0.5 text-[11px] leading-snug text-negative" role="alert" data-testid="place-error">
			{failure}
		</p>
	{/if}
</div>
