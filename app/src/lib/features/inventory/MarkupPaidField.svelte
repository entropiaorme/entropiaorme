<script lang="ts">
	/**
	 * Markup paid, entered either as the game's percentage of TT or as PED
	 * above TT, with the other form read out beside it. The PED figure is the
	 * one bound out: it is what is recorded. A percentage is the same for one
	 * unit as for a stack, so it suits a price read off the auction; PED suits
	 * a total paid in a trade.
	 *
	 * An item with no TT is worth its markup alone, so there is no percentage
	 * to speak of: the field is PED only, and says what each unit cost.
	 */
	import { untrack } from 'svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { formatPed, isZeroTt } from '$lib/utils/format';

	type MarkupMode = 'percent' | 'ped';

	let {
		ttValue,
		markupPed = $bindable(0),
		initialMode = 'percent',
		resetKey,
		label = 'MU paid',
		quantity = 1,
	}: {
		/** The TT the markup is paid over, in total. */
		ttValue: number;
		/** How many units the markup is paid for, so a figure with no TT to
		 * read against can be read per unit instead. */
		quantity?: number | null;
		markupPed?: number;
		initialMode?: MarkupMode;
		/** Changes whenever the form opens on something new, so the field
		 * starts again from the bound figure. */
		resetKey: unknown;
		label?: string;
	} = $props();

	let mode = $state<MarkupMode>('percent');
	let markupPercent = $state(100);
	const noTt = $derived(isZeroTt(ttValue));
	const units = $derived(quantity && quantity > 0 ? quantity : 1);

	$effect(() => {
		void resetKey;
		untrack(() => {
			mode = initialMode;
			markupPercent = ttValue > 0 ? 100 + (markupPed / ttValue) * 100 : 100;
		});
	});

	// With no TT the figure can only be read in PED, so it is shown as PED
	// whatever the chosen form; the figure that is saved is then the one in
	// view. Typing it there makes PED the chosen form, so a TT entered after
	// does not overwrite it with a percentage nobody chose.
	const shown = $derived<MarkupMode>(noTt ? 'ped' : mode);

	$effect(() => {
		if (mode === 'percent' && ttValue > 0) {
			markupPed = Math.round(ttValue * (markupPercent - 100)) / 100;
		}
	});

	$effect(() => {
		if (mode === 'ped' && ttValue > 0) {
			markupPercent = Math.round((100 + (markupPed / ttValue) * 100) * 100) / 100;
		}
	});
</script>

<div>
	<div class="mb-1 flex h-6 items-center justify-between">
		<span class="text-xs text-text-secondary">{label}</span>
		{#if noTt}
			<span class="text-xs text-text-tertiary">No TT, so all of it is markup</span>
		{:else}
			<SegmentedControl
				options={[
					{ id: 'percent', label: '%' },
					{ id: 'ped', label: 'PED' },
				]}
				active={mode}
				onchange={(id) => (mode = id as MarkupMode)}
			/>
		{/if}
	</div>

	<div class="grid gap-3 {noTt && units === 1 ? 'grid-cols-1' : 'grid-cols-2'}">
		{#if shown === 'percent'}
			<Input
				type="number"
				bind:value={markupPercent}
				step="0.1"
				min="100"
				placeholder="100"
				aria-label={`${label} as a percentage of TT`}
			/>
		{:else}
			<Input
				type="number"
				bind:value={
					() => markupPed,
					(next) => {
						markupPed = next;
						if (noTt) mode = 'ped';
					}
				}
				step="0.01"
				min="0"
				placeholder="0.00"
				aria-label={`${label} in PED`}
			/>
		{/if}
		{#if !(noTt && units === 1)}
			<div
				class="flex h-9 items-center justify-end rounded-md border border-border/50 bg-surface/50
					px-3 text-sm tabular-nums text-text-secondary"
			>
				{#if noTt}
					{formatPed((markupPed ?? 0) / units)} PED each
				{:else if shown === 'percent'}
					{formatPed(markupPed)} PED
				{:else}
					{markupPercent.toFixed(2)}%
				{/if}
			</div>
		{/if}
	</div>
</div>
