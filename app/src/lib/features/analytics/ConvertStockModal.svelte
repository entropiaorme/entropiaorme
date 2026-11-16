<script lang="ts">
	/**
	 * Recycle held stock into Nanocubes.
	 *
	 * A transformation, not a sale: TT is preserved exactly (100 PED of wood
	 * becomes 100 PED of Nanocubes, with no refiner cost), no markup is
	 * realised, and the ledger is untouched. The consumed stock's activity
	 * composition rides forward into the Nanocubes, so selling them later
	 * still attributes back to the tiers that grew the wood.
	 *
	 * Because the conversion is 1:1 there is only ever one decision here: how
	 * much. The modal is built around that single field, with the position it
	 * is drawn from sitting under it as both the context for the number and the
	 * way back to all of it.
	 */
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import HeldAmountField from '$lib/features/inventory/HeldAmountField.svelte';
	import type { AmountMode } from '$lib/features/inventory/heldAmount';
	import type { TreeCuttingStock } from './treeCuttingModel.svelte';

	let {
		item,
		onconvert,
		oncancel,
	}: {
		item: TreeCuttingStock | null;
		onconvert: (sourceItem: string, quantity: number) => Promise<void>;
		oncancel: () => void;
	} = $props();

	// The player holds and thinks in PED, and the conversion is 1:1 in TT, so
	// the amount opens in PED; a count is one toggle away. Units are what the
	// command takes, and the field works them out from the PED entered.
	let units = $state<number | null>(null);
	let entry = $state<AmountMode>('ped');
	let converting = $state(false);
	let error = $state<string | null>(null);

	let modalOpen = $state(false);
	let initialisedFor = $state<string | null>(null);

	const unitTt = $derived(item && item.heldQty > 0 ? item.heldTt / item.heldQty : 0);
	// A sale may run past tracked stock, because the player can hold units the
	// app never recorded. A conversion may not: what it produces is credited to
	// the activities that grew the source, so converting more than is tracked
	// would credit them with Nanocubes they did not produce.
	const overStock = $derived(!!item && units !== null && units > item.heldQty + 1e-9);
	const valid = $derived(units !== null && units > 0 && unitTt > 0 && !overStock);

	$effect(() => {
		if (item && initialisedFor !== item.itemName) {
			units = item.heldQty;
			entry = 'ped';
			error = null;
			initialisedFor = item.itemName;
			modalOpen = true;
		}
		if (!item) {
			initialisedFor = null;
			modalOpen = false;
		}
	});

	$effect(() => {
		if (item && !modalOpen) oncancel();
	});

	async function confirm() {
		if (!item || converting || !valid || units === null) return;
		converting = true;
		error = null;
		try {
			await onconvert(item.itemName, units);
			modalOpen = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to convert the stock';
		} finally {
			converting = false;
		}
	}
</script>

{#if item}
	<Modal bind:open={modalOpen} class="max-w-xs" title={`Convert ${item.itemName}`}>
		<div class="space-y-5">
			<!-- The amount carries no label of its own. The title says what is
				being converted and the line beneath says how much there is; a
				caption above it would only say it again. -->
			<div class="space-y-1.5">
				<HeldAmountField
					bind:units
					bind:mode={entry}
					{unitTt}
					held={item.heldQty}
					subject="to convert"
				/>
				{#if overStock}
					<p class="text-xs text-warning">More than the {item.itemName} in stock.</p>
				{/if}
			</div>

			{#if error}
				<p class="text-xs text-error">{error}</p>
			{/if}

			<div class="flex items-center justify-end gap-2">
				<Button variant="ghost" onclick={oncancel} disabled={converting}>Cancel</Button>
				<Button
					onclick={confirm}
					loading={converting}
					disabled={!valid}
				>
					Convert
				</Button>
			</div>
		</div>
	</Modal>
{/if}
