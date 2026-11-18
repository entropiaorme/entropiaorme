<script lang="ts">
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import AccountRecordHint from '$lib/features/inventory/AccountRecordHint.svelte';
	import HeldAmountField from '$lib/features/inventory/HeldAmountField.svelte';
	import { type AmountMode, pedForUnits } from '$lib/features/inventory/heldAmount';
	import { formatPed } from '$lib/utils/format';
	import { stockHasNoTt, type TreeCuttingStock } from './treeCuttingModel.svelte';

	let {
		item,
		mode,
		onconfirm,
		oncancel,
	}: {
		item: TreeCuttingStock | null;
		mode: 'remove' | 'shrapnel';
		onconfirm: (itemName: string, quantity: number) => Promise<void>;
		oncancel: () => void;
	} = $props();

	// Stock is held and thought of in PED of TT, so the amount opens in PED;
	// a count is one toggle away for when the units are what is known. An item
	// with no TT has no PED to enter, so it is entered by count.
	let units = $state<number | null>(null);
	let entry = $state<AmountMode>('ped');
	let saving = $state(false);
	let error = $state<string | null>(null);
	let modalOpen = $state(false);
	let initialisedFor = $state<string | null>(null);
	const unitTt = $derived(
		item && !stockHasNoTt(item) && item.heldQty > 0 ? item.heldTt / item.heldQty : 0,
	);
	const tooMany = $derived(!!item && units !== null && units > item.heldQty + 1e-9);
	const valid = $derived(units !== null && units > 0 && !tooMany);

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

	async function submit() {
		if (!item || saving || !valid || units === null) return;
		saving = true;
		error = null;
		try {
			await onconfirm(item.itemName, units);
			modalOpen = false;
		} catch (e) {
			error = e instanceof Error ? e.message : `Failed to ${mode === 'remove' ? 'remove' : 'convert'} the stock`;
		} finally {
			saving = false;
		}
	}
</script>

{#if item}
	<Modal
		bind:open={modalOpen}
		class="max-w-xs"
		title={mode === 'remove' ? `Remove ${item.itemName}` : 'Convert Shrapnel'}
	>
		<div class="space-y-5">
			<div class="space-y-1.5">
				<HeldAmountField
					bind:units
					bind:mode={entry}
					{unitTt}
					held={item.heldQty}
					subject={mode === 'remove' ? 'to remove' : 'to convert'}
					describedby={mode === 'shrapnel' ? 'shrapnel-account-record' : undefined}
				/>
				{#if tooMany}
					<p class="text-xs text-warning">More than the {item.itemName} in stock.</p>
				{:else if mode === 'shrapnel' && units !== null && units > 0}
					<p class="text-xs tabular-nums text-text-tertiary">
						{formatPed(pedForUnits(units, unitTt) * 1.01)} PED ammo after conversion
					</p>
				{/if}
				{#if mode === 'shrapnel'}
					<AccountRecordHint
						kind="conversion"
						itemName={item.itemName}
						id="shrapnel-account-record"
					/>
				{/if}
			</div>

			{#if mode === 'remove'}
				<p class="text-xs leading-relaxed text-text-tertiary">
					This only removes it from current stock. The loot and its historical TT stay recorded.
				</p>
			{/if}
			{#if error}<p class="text-xs text-error">{error}</p>{/if}
			<div class="flex items-center justify-end gap-2">
				<Button variant="ghost" onclick={oncancel} disabled={saving}>Cancel</Button>
				<Button onclick={submit} loading={saving} disabled={!valid}>
					{mode === 'remove' ? 'Remove' : 'Convert'}
				</Button>
			</div>
		</div>
	</Modal>
{/if}
