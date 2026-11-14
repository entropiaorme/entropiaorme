<script lang="ts">
	import type { InventoryItem } from '$lib/types/analytics';
	import { addEquipmentHolding, updateEquipmentHolding } from '$lib/api/inventory';
	import { formatPed } from '$lib/utils/format';
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import { figuresLockedReason, perUnit } from './assetPosition';
	import MarkupPaidField from './MarkupPaidField.svelte';

	let {
		open = $bindable(false),
		item = null,
		onsaved,
	}: {
		open?: boolean;
		item?: InventoryItem | null;
		onsaved: (item: InventoryItem) => void;
	} = $props();

	let name = $state('');
	let quantity = $state(1);
	let ttValue = $state(0);
	let markupPed = $state(0);
	let notes = $state('');
	let saving = $state(false);
	let error = $state<string | null>(null);
	let initialised = $state(false);
	/** Bumped each time the form opens, so the markup field starts again. */
	let opened = $state(0);

	$effect(() => {
		if (open && !initialised) {
			name = item?.name ?? '';
			quantity = item?.quantity ?? 1;
			ttValue = item?.ttValue ?? 0;
			markupPed = item?.markupPaid ?? 0;
			notes = item?.notes ?? '';
			error = null;
			opened += 1;
			initialised = true;
		}
		if (!open) {
			initialised = false;
		}
	});

	/** Why the figures are read-only here, for an asset whose cost now comes
	 * from its purchases or is fixed by what has already left. */
	const locked = $derived(item ? figuresLockedReason(item) : null);
	/** An asset a conversion made keeps the count it was made as. */
	const quantityFixed = $derived(item !== null && !item.divisible);
	const costBasis = $derived(ttValue + markupPed);
	const stack = $derived(quantity > 0 && Math.abs(quantity - 1) > 1e-9);
	const canSave = $derived(name.trim().length > 0 && ttValue > 0 && quantity > 0);

	async function save() {
		if (!canSave || saving) return;
		saving = true;
		error = null;
		try {
			const saved = item
				? await updateEquipmentHolding(item.id, {
						name: name.trim(),
						notes: notes.trim() || null,
						...(locked
							? {}
							: {
									...(quantityFixed ? {} : { quantity }),
									tt_value: ttValue,
									markup_paid: markupPed,
								}),
					})
				: await addEquipmentHolding({
						name: name.trim(),
						quantity,
						tt_value: ttValue,
						markup_paid: markupPed,
						notes: notes.trim() || null,
					});
			onsaved(saved);
			open = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to save inventory item';
		} finally {
			saving = false;
		}
	}
</script>

<Modal bind:open title={item ? 'Edit asset' : 'Add asset'}>
	<div class="space-y-4">
		<div>
			<label for="inventory-name" class="text-xs text-text-secondary mb-1 block">Name</label>
			<Input
				id="inventory-name"
				type="text"
				bind:value={name}
				placeholder="e.g. Mod FAP, Treasure Island deed"
			/>
		</div>

		{#if locked}
			<p class="text-xs text-text-tertiary">{locked}</p>
		{:else}
			<div class="grid grid-cols-[6rem_minmax(0,1fr)] gap-3">
				<div>
					<label for="inventory-quantity" class="text-xs text-text-secondary mb-1 block">
						Quantity
					</label>
					<Input
						id="inventory-quantity"
						type="number"
						bind:value={quantity}
						step="1"
						min="1"
						align="right"
						disabled={quantityFixed}
					/>
				</div>
				<div>
					<label for="inventory-tt" class="text-xs text-text-secondary mb-1 block">
						TT value (PED{stack ? ', all units' : ''})
					</label>
					<Input
						id="inventory-tt"
						type="number"
						bind:value={ttValue}
						step="0.01"
						min="0"
						placeholder="0.00"
					/>
				</div>
			</div>

			<MarkupPaidField
				{ttValue}
				bind:markupPed
				resetKey={opened}
				label={stack ? 'MU paid, all units' : 'MU paid'}
			/>
		{/if}

		<div>
			<label for="inventory-notes" class="text-xs text-text-secondary mb-1 block">
				Notes <span class="text-text-tertiary">(optional)</span>
			</label>
			<textarea
				id="inventory-notes"
				bind:value={notes}
				rows="2"
				placeholder="Anything worth remembering"
				class="w-full px-3 py-2 text-sm bg-surface text-text rounded-md border border-border
					placeholder:text-text-tertiary resize-none
					focus:outline-none focus:border-accent focus:ring-1 focus:ring-accent"
			></textarea>
		</div>

		<div class="flex items-baseline justify-between border-t border-border/50 pt-3">
			<span class="eyebrow text-text-tertiary">Total cost</span>
			<span class="text-right">
				<span class="block text-sm font-semibold tabular-nums text-text">
					{formatPed(costBasis)} PED
				</span>
				{#if stack}
					<span class="block text-xs tabular-nums text-text-tertiary">
						{formatPed(perUnit(costBasis, quantity) ?? 0)} each,
						{formatPed(perUnit(markupPed, quantity) ?? 0)} of it markup
					</span>
				{/if}
			</span>
		</div>

		{#if error}
			<p class="text-xs text-error">{error}</p>
		{/if}

		<div class="flex items-center justify-end gap-2 pt-2">
			<Button variant="ghost" onclick={() => (open = false)} disabled={saving}>Cancel</Button>
			<Button onclick={save} loading={saving} disabled={!canSave}>
				{item ? 'Save' : 'Add'}
			</Button>
		</div>
	</div>
</Modal>
