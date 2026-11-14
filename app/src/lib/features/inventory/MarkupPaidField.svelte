<script lang="ts">
	/**
	 * Markup paid, entered either as the game's percentage of TT or as PED
	 * above TT, with the other form read out beside it. The PED figure is the
	 * one bound out: it is what is recorded. A percentage is the same for one
	 * unit as for a stack, so it suits a price read off the auction; PED suits
	 * a total paid in a trade.
	 */
	import { untrack } from 'svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { formatPed, NO_DATA } from '$lib/utils/format';

	type MarkupMode = 'percent' | 'ped';

	let {
		ttValue,
		markupPed = $bindable(0),
		initialMode = 'percent',
		resetKey,
		label = 'MU paid',
	}: {
		/** The TT the markup is paid over, in total. */
		ttValue: number;
		markupPed?: number;
		initialMode?: MarkupMode;
		/** Changes whenever the form opens on something new, so the field
		 * starts again from the bound figure. */
		resetKey: unknown;
		label?: string;
	} = $props();

	let mode = $state<MarkupMode>('percent');
	let markupPercent = $state(100);

	$effect(() => {
		void resetKey;
		untrack(() => {
			mode = initialMode;
			markupPercent = ttValue > 0 ? 100 + (markupPed / ttValue) * 100 : 100;
		});
	});

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
	<div class="mb-1 flex items-center justify-between">
		<span class="text-xs text-text-secondary">{label}</span>
		<SegmentedControl
			options={[
				{ id: 'percent', label: '%', disabled: ttValue <= 0 },
				{ id: 'ped', label: 'PED' },
			]}
			active={mode}
			onchange={(id) => (mode = id as MarkupMode)}
		/>
	</div>

	<div class="grid grid-cols-2 gap-3">
		{#if mode === 'percent'}
			<Input
				type="number"
				bind:value={markupPercent}
				step="0.1"
				min="100"
				placeholder="100"
				aria-label={`${label} as a percentage of TT`}
				disabled={ttValue <= 0}
			/>
		{:else}
			<Input
				type="number"
				bind:value={markupPed}
				step="0.01"
				min="0"
				placeholder="0.00"
				aria-label={`${label} in PED`}
			/>
		{/if}
		<div
			class="flex h-9 items-center justify-end rounded-md border border-border/50 bg-surface/50
				px-3 text-sm tabular-nums text-text-secondary"
		>
			{#if ttValue <= 0}
				{NO_DATA}
			{:else if mode === 'percent'}
				{formatPed(markupPed)} PED
			{:else}
				{markupPercent.toFixed(2)}%
			{/if}
		</div>
	</div>
</div>
