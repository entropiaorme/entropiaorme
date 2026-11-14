<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import type { EquipmentListingInput, EquipmentTradeInput, InventoryItem } from '$lib/api';
	import { formatPed, formatQuantity, isZeroTt, todayDate } from '$lib/utils/format';
	import { isStack, perUnit, shareOf } from './assetPosition';

	let {
		item,
		onlist,
		ontrade,
		oncancel,
	}: {
		item: InventoryItem | null;
		onlist: (input: EquipmentListingInput) => Promise<void>;
		ontrade: (input: EquipmentTradeInput) => Promise<void>;
		oncancel: () => void;
	} = $props();

	let method = $state<'auction' | 'trade'>('auction');
	let startingBid = $state(0);
	let buyout = $state<number | null>(null);
	let listingFee = $state(0.5);
	let soldFor = $state(0);
	/** How many of the asset's units go; every one of them by default. */
	let quantity = $state<number | null>(null);
	let occurredAt = $state('');
	let saving = $state(false);
	let error = $state<string | null>(null);
	let modalOpen = $state(false);
	let initialisedFor = $state<string | null>(null);

	/** Part of the asset can go only when it is a stack that divides. */
	const partial = $derived(item !== null && item.divisible && isStack(item));
	const selling = $derived(item ? Math.min(quantity ?? item.quantity, item.quantity) : 0);
	const share = $derived(item ? shareOf(item, selling) : { ttValue: 0, markupPaid: 0 });
	const tooMany = $derived(item !== null && quantity !== null && quantity > item.quantity + 1e-9);
	const quantityValid = $derived(quantity === null || (quantity > 0 && !tooMany));
	const costBasis = $derived(share.ttValue + share.markupPaid);
	/** An item with no TT is worth its markup alone: its cost is all markup,
	 * so it is read by how many go and what each cost. */
	const noTt = $derived(item !== null && isZeroTt(item.ttValue));
	const expectedPrice = $derived(method === 'trade' ? soldFor : (buyout ?? startingBid));
	const expectedResult = $derived(
		expectedPrice - costBasis - (method === 'auction' ? listingFee : 0),
	);

	$effect(() => {
		if (item && initialisedFor !== item.id) {
			method = 'auction';
			startingBid = 0;
			buyout = null;
			listingFee = 0.5;
			soldFor = 0;
			quantity = null;
			occurredAt = todayDate();
			error = null;
			initialisedFor = item.id;
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
		if (!item || saving || !quantityValid) return;
		saving = true;
		error = null;
		// Every unit is sent as none named, which the backend takes as all.
		const units = partial ? quantity : null;
		try {
			if (method === 'auction') {
				await onlist({
					itemId: item.id,
					quantity: units,
					startingBid,
					buyout,
					listingFee,
					listedAt: occurredAt || null,
				});
			} else {
				await ontrade({
					itemId: item.id,
					quantity: units,
					soldFor,
					soldAt: occurredAt || null,
				});
			}
			modalOpen = false;
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Failed to record the sale';
		} finally {
			saving = false;
		}
	}
</script>

{#if item}
	<Modal bind:open={modalOpen} title={`Sell ${item.name}`}>
		<div class="space-y-4">
			<SegmentedControl
				options={[{ id: 'auction', label: 'Auction' }, { id: 'trade', label: 'Trade' }]}
				active={method}
				onchange={(id) => (method = id as 'auction' | 'trade')}
			/>

			{#if partial}
				<div class="flex items-end gap-3">
					<label class="block w-28 space-y-1">
						<span class="eyebrow text-text-tertiary">Quantity</span>
						<Input
							type="number"
							min="1"
							step="1"
							align="right"
							placeholder={formatQuantity(item.quantity)}
							aria-label="Quantity to sell"
							bind:value={quantity}
						/>
					</label>
					<button
						type="button"
						class="pb-2 text-xs tabular-nums hover:text-text {tooMany
							? 'text-warning'
							: 'text-text-tertiary'}"
						onclick={() => (quantity = null)}
					>
						of {formatQuantity(item.quantity)} held
					</button>
				</div>
			{/if}

			<div class="space-y-1.5 rounded-md border border-border/50 bg-surface/50 px-3 py-2 text-sm">
				{#if noTt}
					<div class="flex items-baseline justify-between">
						<span class="font-medium text-text">Total cost</span>
						<span class="text-right tabular-nums">
							<span class="block font-semibold text-text">{formatPed(costBasis)} PED</span>
							{#if selling > 1 + 1e-9}
								<span class="block text-xs text-text-tertiary">
									{formatQuantity(selling)} at {formatPed(perUnit(costBasis, selling) ?? 0)} each
								</span>
							{/if}
						</span>
					</div>
				{:else}
					<div class="flex items-center justify-between">
						<span class="text-text-secondary">TT value</span>
						<span class="tabular-nums text-text">{formatPed(share.ttValue)} PED</span>
					</div>
					<div class="flex items-center justify-between">
						<span class="text-text-secondary">MU paid</span>
						<span class="tabular-nums text-text">{formatPed(share.markupPaid)} PED</span>
					</div>
					<div class="flex items-center justify-between border-t border-border/50 pt-1.5">
						<span class="font-medium text-text">Total cost</span>
						<span class="tabular-nums font-semibold text-text">{formatPed(costBasis)} PED</span>
					</div>
				{/if}
				{#if expectedPrice > 0}
					<div class="flex items-center justify-between border-t border-border/50 pt-1.5">
						<span class="font-medium text-text">
							{method === 'auction' ? 'Result if it clears' : 'Realised result'}
						</span>
						<span class="tabular-nums font-semibold {expectedResult >= 0 ? 'text-positive' : 'text-negative'}">
							{expectedResult >= 0 ? '+' : ''}{formatPed(expectedResult)} PED
						</span>
					</div>
				{/if}
			</div>

			{#if method === 'auction'}
				<div class="grid grid-cols-2 gap-3">
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Starting bid (PED)</span>
						<Input type="number" min="0" step="0.01" bind:value={startingBid} />
					</label>
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Buyout (PED, optional)</span>
						<Input type="number" min="0" step="0.01" bind:value={buyout} />
					</label>
				</div>
				<div class="grid grid-cols-2 gap-3">
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Listing fee (PED)</span>
						<Input type="number" min="0" step="0.01" bind:value={listingFee} />
					</label>
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Listed on</span>
						<Input type="date" bind:value={occurredAt} />
					</label>
				</div>
				<p class="text-xs leading-relaxed text-text-tertiary">
					{selling < item.quantity - 1e-9
						? 'These units leave Inventory now at the average cost; the rest keep it.'
						: 'The asset leaves Inventory now.'}
					Its fee is charged immediately and remains spent if the auction expires; the original
					total cost is retained for confirmation and undo.
				</p>
			{:else}
				<div class="grid grid-cols-2 gap-3">
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Sold for (PED)</span>
						<Input type="number" min="0" step="0.01" bind:value={soldFor} />
					</label>
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Sold on</span>
						<Input type="date" bind:value={occurredAt} />
					</label>
				</div>
			{/if}

			{#if error}<p class="text-xs text-error">{error}</p>{/if}
			<div class="flex items-center justify-end gap-2 pt-2">
				<Button variant="ghost" onclick={oncancel} disabled={saving}>Cancel</Button>
				<Button onclick={confirm} loading={saving} disabled={!quantityValid}>
					{method === 'auction' ? 'List on auction' : 'Record trade'}
				</Button>
			</div>
		</div>
	</Modal>
{/if}
