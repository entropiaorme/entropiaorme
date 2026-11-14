<script lang="ts">
	/**
	 * Buying more of an asset, or correcting a purchase already recorded.
	 *
	 * The form shows what the purchase does to the position before it is
	 * saved: how many are held and what each has cost on average. Buying more
	 * is the only thing that moves that average, so this is where the player
	 * sees it move.
	 *
	 * A position of an item with no TT (strongboxes, tokens) is bought by count
	 * and price alone: there is no TT to enter, and every PED of it is markup.
	 */
	import type { AssetPurchase, AssetPurchaseInput, InventoryItem } from '$lib/api';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { formatPed, formatQuantity, isZeroTt, todayDate } from '$lib/utils/format';
	import { averageAfter, currentAverage, perUnit } from './assetPosition';
	import MarkupPaidField from './MarkupPaidField.svelte';

	let {
		item,
		purchase = null,
		onsubmit,
		oncancel,
	}: {
		/** The asset bought into; null closes the form. */
		item: InventoryItem | null;
		/** The purchase being corrected, or null for a new one. */
		purchase?: AssetPurchase | null;
		onsubmit: (input: AssetPurchaseInput) => Promise<void>;
		oncancel: () => void;
	} = $props();

	let quantity = $state<number | null>(null);
	let ttValue = $state(0);
	let markupPed = $state(0);
	let acquiredAt = $state('');
	let saving = $state(false);
	let error = $state<string | null>(null);
	let modalOpen = $state(false);
	let initialisedFor = $state<string | null>(null);
	let opened = $state(0);

	const key = $derived(item ? `${item.id}:${purchase?.id ?? 'new'}` : null);

	$effect(() => {
		if (item && key !== initialisedFor) {
			quantity = purchase?.quantity ?? null;
			// A new purchase starts at the TT each unit already carries, so the
			// player usually only types how many and what they paid.
			ttValue = purchase?.ttValue ?? 0;
			markupPed = purchase?.markupPaid ?? 0;
			acquiredAt = purchase?.acquiredAt ?? todayDate();
			error = null;
			opened += 1;
			initialisedFor = key;
			modalOpen = true;
		}
		if (!item) {
			initialisedFor = null;
			modalOpen = false;
		}
	});

	// Until the player types a TT of their own, a new purchase's TT follows
	// its quantity at the position's TT per unit.
	let ttTyped = $state(false);
	$effect(() => {
		if (!item || purchase || ttTyped) return;
		const unitTt = perUnit(item.ttValue, item.quantity) ?? 0;
		ttValue = Math.round((quantity ?? 0) * unitTt * 10000) / 10000;
	});
	$effect(() => {
		void key;
		ttTyped = false;
	});

	$effect(() => {
		if (item && !modalOpen) oncancel();
	});

	/** A position with no TT stays one: its purchases carry none to enter. */
	const noTt = $derived(
		item !== null && isZeroTt(item.ttValue) && isZeroTt(purchase?.ttValue ?? 0),
	);
	const valid = $derived(
		quantity !== null &&
			quantity > 0 &&
			Number.isFinite(ttValue) &&
			ttValue >= 0 &&
			Number.isFinite(markupPed),
	);
	const before = $derived(item ? currentAverage(item) : null);
	const after = $derived(
		item && valid && quantity !== null
			? averageAfter(item, { quantity, ttValue, markupPaid: markupPed }, purchase)
			: null,
	);

	async function save() {
		if (!item || !valid || quantity === null || saving) return;
		saving = true;
		error = null;
		try {
			await onsubmit({ quantity, ttValue, markupPaid: markupPed, acquiredAt });
			modalOpen = false;
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Failed to save the purchase';
		} finally {
			saving = false;
		}
	}
</script>

<Modal
	bind:open={modalOpen}
	title={purchase ? 'Edit purchase' : `Add purchase${item ? ` of ${item.name}` : ''}`}
>
	{#if item}
		<div class="space-y-4">
			<div
				class="grid gap-3 {noTt
					? 'grid-cols-[6rem_minmax(0,1fr)]'
					: 'grid-cols-[6rem_minmax(0,1fr)_minmax(0,1fr)]'}"
			>
				<div>
					<label for="purchase-quantity" class="mb-1 block text-xs text-text-secondary">
						Quantity
					</label>
					<Input
						id="purchase-quantity"
						type="number"
						bind:value={quantity}
						step="1"
						min="1"
						align="right"
						placeholder="0"
					/>
				</div>
				{#if !noTt}
					<div>
						<label for="purchase-tt" class="mb-1 block text-xs text-text-secondary">
							TT value (PED)
						</label>
						<Input
							id="purchase-tt"
							type="number"
							bind:value={
								() => ttValue,
								(next) => {
									ttValue = Number(next ?? 0);
									ttTyped = true;
								}
							}
							step="0.01"
							min="0"
							placeholder="0.00"
						/>
					</div>
				{/if}
				<div>
					<label for="purchase-date" class="mb-1 block text-xs text-text-secondary">Bought</label>
					<Input id="purchase-date" type="date" bind:value={acquiredAt} />
				</div>
			</div>

			<MarkupPaidField
				{ttValue}
				{quantity}
				bind:markupPed
				initialMode="ped"
				resetKey={opened}
				label={noTt ? 'Paid, all units' : 'MU paid, all units'}
			/>

			{#if before}
				<dl
					class="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] items-baseline gap-x-3 gap-y-1.5
						border-t border-border/50 pt-3 text-sm tabular-nums"
					aria-label="What this purchase does to the position"
				>
					<dt class="text-xs text-text-secondary">Held</dt>
					<dd class="text-right text-text-secondary">{formatQuantity(before.quantity)}</dd>
					<dd class="text-text-tertiary" aria-hidden="true">&rarr;</dd>
					<dd class="text-right font-medium text-text">
						{after ? formatQuantity(after.quantity) : formatQuantity(before.quantity)}
					</dd>

					{#if !noTt}
						<dt class="text-xs text-text-secondary">MU per unit</dt>
						<dd class="text-right text-text-secondary">
							{formatPed(before.markupPerUnit ?? 0)}
						</dd>
						<dd class="text-text-tertiary" aria-hidden="true">&rarr;</dd>
						<dd class="text-right font-medium text-text">
							{formatPed((after ?? before).markupPerUnit ?? 0)}
						</dd>
					{/if}

					<dt class="text-xs text-text-secondary">Cost per unit</dt>
					<dd class="text-right text-text-secondary">{formatPed(before.costPerUnit ?? 0)}</dd>
					<dd class="text-text-tertiary" aria-hidden="true">&rarr;</dd>
					<dd class="text-right font-medium text-text">
						{formatPed((after ?? before).costPerUnit ?? 0)}
					</dd>
				</dl>
			{/if}

			{#if error}
				<p class="text-xs text-error">{error}</p>
			{/if}

			<div class="flex items-center justify-end gap-2 pt-2">
				<Button variant="ghost" onclick={() => (modalOpen = false)} disabled={saving}>
					Cancel
				</Button>
				<Button onclick={save} loading={saving} disabled={!valid}>
					{purchase ? 'Save' : 'Add purchase'}
				</Button>
			</div>
		</div>
	{/if}
</Modal>
