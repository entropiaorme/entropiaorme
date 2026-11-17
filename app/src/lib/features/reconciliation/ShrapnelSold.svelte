<script lang="ts">
	/**
	 * How much of the missing Shrapnel was sold at TT rather than converted
	 * into Universal Ammo; nothing entered means none. Sent when the field
	 * is left or Enter is pressed, and only when it changed.
	 */
	import HeldAmountField from '$lib/features/inventory/HeldAmountField.svelte';
	import type { Line } from './reconciliation';

	let {
		line,
		error = null,
		onchange,
	}: {
		line: Line;
		/** Why the last amount was refused, said beneath the field. */
		error?: string | null;
		onchange: (quantity: number | null) => void | Promise<void>;
	} = $props();

	const saved = $derived(line.disposition.kind === 'shrapnelConverted' ? line.disposition.soldAtTt : null);
	const missing = $derived(Math.abs(line.difference));
	const unitTt = $derived(line.unitTt.kind === 'unknown' ? 0 : line.unitTt.ped);

	let units = $state<number | null>(null);
	// The field follows the draft whenever the draft's amount moves.
	$effect.pre(() => {
		units = saved;
	});

	function commit() {
		const next = units !== null && units > 0 ? units : null;
		if (next === saved) return;
		void onchange(next);
	}

	function onfocusout(event: FocusEvent) {
		const to = event.relatedTarget;
		if (to instanceof Node && (event.currentTarget as HTMLElement).contains(to)) return;
		commit();
	}
</script>

<form
	class="flex flex-wrap items-center justify-end gap-2"
	{onfocusout}
	onsubmit={(event) => {
		event.preventDefault();
		commit();
	}}
>
	<span class="text-xs text-text-tertiary">of which sold at TT</span>
	<HeldAmountField
		bind:units
		{unitTt}
		held={missing}
		subject="of the missing Shrapnel sold at TT"
		variant="inline"
		class="w-32"
		describedby={error ? 'shrapnel-sold-error' : undefined}
	/>
	{#if error}
		<p id="shrapnel-sold-error" class="basis-full text-right text-xs text-error" role="alert">{error}</p>
	{/if}
</form>
